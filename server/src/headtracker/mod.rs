//! Webcam head tracking: camera -> face detection -> landmarks -> PnP pose ->
//! One Euro smoothing -> FreeTrack/TrackIR output for games.
//!
//! Runs on its own thread, independent of the phone/controller server, so you
//! can use head tracking with or without the phone connected.

mod filter;
mod nn;
mod pnp;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::freetrack::{FreetrackWriter, HeadPose};
use crate::server::Shared;
use filter::OneEuro;
use nn::{FaceBox, Nets, LM_COUNT};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;

/// Live tuning knobs, shared with the GUI (sliders apply immediately).
#[derive(Clone, Copy)]
pub struct Settings {
    pub camera: u32,
    /// Camera diagonal FOV in degrees; webcams are usually 60-80.
    pub fov: f32,
    /// 0 = raw and jittery, 1 = very smooth but floaty.
    pub smoothing: f32,
    /// Multiplier on yaw/pitch/roll so a comfortable real turn covers the
    /// full in-game range.
    pub rot_gain: f32,
    /// Multiplier on x/y/z movement; 0 disables position.
    pub pos_gain: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            camera: 0,
            fov: 70.0,
            smoothing: 0.5,
            rot_gain: 2.5,
            pos_gain: 1.0,
        }
    }
}

/// Status + preview shared with the GUI.
pub struct HeadShared {
    pub status: Mutex<HeadStatus>,
    /// GUI sets this to re-zero the pose on the current head position.
    pub center: AtomicBool,
    /// GUI sets this while the preview is visible; frames are only copied then.
    pub preview_on: AtomicBool,
    pub preview: Mutex<Option<PreviewFrame>>,
}

impl Default for HeadShared {
    fn default() -> HeadShared {
        HeadShared {
            status: Mutex::new(HeadStatus::default()),
            center: AtomicBool::new(false),
            preview_on: AtomicBool::new(false),
            preview: Mutex::new(None),
        }
    }
}

#[derive(Clone, Default)]
pub struct HeadStatus {
    pub running: bool,
    pub camera_ok: bool,
    pub face: bool,
    pub fps: f32,
    /// TrackIR game id currently reading the pose (0 = none yet).
    pub game_id: i32,
    pub yaw: f32,
    pub pitch: f32,
    pub error: Option<String>,
}

/// Small RGB frame + landmark overlay for the GUI preview.
pub struct PreviewFrame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    /// Landmark positions in preview pixel coords.
    pub points: Vec<(f32, f32)>,
    pub face: Option<(f32, f32, f32, f32)>,
}

/// List available cameras as (index, name) for the GUI picker.
pub fn list_cameras() -> Vec<(u32, String)> {
    match nokhwa::query(nokhwa::utils::ApiBackend::Auto) {
        Ok(cams) => cams
            .into_iter()
            .filter_map(|c| match c.index() {
                CameraIndex::Index(i) => Some((*i, c.human_name())),
                CameraIndex::String(_) => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn set_status(shared: &Shared, f: impl FnOnce(&mut HeadStatus)) {
    f(&mut shared.head.status.lock().unwrap());
}

/// Run head tracking until `stop` is set.
pub fn run(shared: Arc<Shared>, stop: Arc<AtomicBool>, settings: Arc<Mutex<Settings>>) {
    set_status(&shared, |s| {
        *s = HeadStatus {
            running: true,
            ..HeadStatus::default()
        }
    });
    let result = track(&shared, &stop, &settings);
    if let Err(e) = result {
        shared.log(format!("Head tracking stopped: {e}"));
        set_status(&shared, |s| s.error = Some(e));
    }
    set_status(&shared, |s| s.running = false);
    shared.log("Head tracking stopped.");
}

fn track(
    shared: &Arc<Shared>,
    stop: &Arc<AtomicBool>,
    settings: &Arc<Mutex<Settings>>,
) -> Result<(), String> {
    let cfg = *settings.lock().unwrap();

    let mut writer = FreetrackWriter::new(|m| shared.log(m))?;

    shared.log("Loading face tracking models...");
    let t0 = Instant::now();
    let mut nets = Nets::load().map_err(|e| format!("Could not load NN models: {e}"))?;
    shared.log(format!(
        "Models ready in {:.1}s. Opening camera {}...",
        t0.elapsed().as_secs_f32(),
        cfg.camera
    ));

    let mut camera = open_camera(cfg.camera)?;
    let res = camera.resolution();
    let (fw, fh) = (res.width() as usize, res.height() as usize);
    shared.log(format!(
        "Camera open: {}x{} @ {} fps.",
        res.width(),
        res.height(),
        camera.frame_rate()
    ));
    set_status(shared, |s| s.camera_ok = true);

    let mut solver = pnp::Solver::new(fw as f64, fh as f64, cfg.fov as f64);
    let mut filters: Vec<OneEuro> = (0..6).map(|_| OneEuro::new(1.0, 0.1)).collect();

    let mut face: Option<FaceBox> = None;
    let mut points = [[0.0f32; 2]; LM_COUNT];
    let mut low_conf_frames = 0u32;
    let mut center: Option<pnp::Pose> = None;
    let mut last_frame = Instant::now();
    let mut fps = 0.0f32;
    let mut last_pose = HeadPose::default();

    while !stop.load(Ordering::Relaxed) {
        let frame = match camera.frame() {
            Ok(f) => f,
            Err(e) => {
                set_status(shared, |s| s.camera_ok = false);
                return Err(format!("Camera stopped: {e}"));
            }
        };
        let img = frame
            .decode_image::<RgbFormat>()
            .map_err(|e| format!("Could not decode camera frame: {e}"))?;
        let rgb: &[u8] = img.as_raw();

        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f32().clamp(1e-3, 0.5);
        last_frame = now;
        fps = fps * 0.9 + (1.0 / dt) * 0.1;

        let cfg = *settings.lock().unwrap();

        // Redetect when we have no face or the landmarks have been bad a while.
        if face.is_none() || low_conf_frames > 10 {
            face = nets
                .detect_face(rgb, fw, fh)
                .map_err(|e| format!("Face detector failed: {e}"))?
                .map(|b| b.expanded(0.1, fw, fh));
            low_conf_frames = 0;
        }

        let mut have_pose = false;
        if let Some(cur) = face {
            let conf = nets
                .landmarks(rgb, fw, fh, &cur, &mut points)
                .map_err(|e| format!("Landmark model failed: {e}"))?;

            if conf < 0.25 {
                low_conf_frames += 1;
                if low_conf_frames > 10 {
                    // Lost the face: forget the solver's warm state and the
                    // filters' velocity so the redetect starts clean.
                    face = None;
                    solver.reset();
                    filters.iter_mut().for_each(OneEuro::reset);
                }
            } else {
                low_conf_frames = 0;
                // Follow the face: next crop = landmark bounds + margin.
                face = Some(bounds_of(&points).expanded(0.35, fw, fh));

                let mut obs = [[0.0f64; 2]; 18];
                for (o, &idx) in obs.iter_mut().zip(pnp::CONTOUR_IDX.iter()) {
                    o[0] = points[idx][0] as f64;
                    o[1] = points[idx][1] as f64;
                }
                let raw = solver.solve(&obs);

                // Smooth in raw space so centering doesn't add lag.
                let vals = [raw.yaw, raw.pitch, raw.roll, raw.x, raw.y, raw.z];
                let (min_cutoff, beta) = smoothing_params(cfg.smoothing);
                let mut smoothed = [0.0f64; 6];
                for (i, v) in vals.iter().enumerate() {
                    filters[i].set_params(min_cutoff, beta);
                    smoothed[i] = filters[i].filter(*v as f32, dt) as f64;
                }
                let sm = pnp::Pose {
                    yaw: smoothed[0],
                    pitch: smoothed[1],
                    roll: smoothed[2],
                    x: smoothed[3],
                    y: smoothed[4],
                    z: smoothed[5],
                };

                if center.is_none() || shared.head.center.swap(false, Ordering::Relaxed) {
                    center = Some(sm);
                }
                let c = center.unwrap();

                last_pose = HeadPose {
                    yaw: wrap_deg(sm.yaw - c.yaw) as f32 * cfg.rot_gain,
                    pitch: wrap_deg(sm.pitch - c.pitch) as f32 * cfg.rot_gain,
                    roll: wrap_deg(sm.roll - c.roll) as f32 * cfg.rot_gain,
                    x: (sm.x - c.x) as f32 * cfg.pos_gain,
                    y: -(sm.y - c.y) as f32 * cfg.pos_gain, // camera y is down; FreeTrack Y is up
                    z: -(sm.z - c.z) as f32 * cfg.pos_gain, // lean in = closer = positive Z
                };
                have_pose = true;
            }
        }

        // Keep publishing the last pose while the face is briefly lost, so the
        // view holds instead of snapping to center.
        let game_id = writer.write(last_pose);

        set_status(shared, |s| {
            s.camera_ok = true;
            s.face = have_pose;
            s.fps = fps;
            s.game_id = game_id;
            s.yaw = last_pose.yaw;
            s.pitch = last_pose.pitch;
        });

        if shared.head.preview_on.load(Ordering::Relaxed) {
            publish_preview(shared, rgb, fw, fh, have_pose.then_some(&points), &face);
        }
    }
    Ok(())
}

/// Map the 0..1 smoothing slider onto One Euro parameters.
fn smoothing_params(s: f32) -> (f32, f32) {
    let s = s.clamp(0.0, 1.0);
    let min_cutoff = 3.0 + (0.35 - 3.0) * s;
    let beta = 0.4 + (0.015 - 0.4) * s;
    (min_cutoff, beta)
}

fn wrap_deg(v: f64) -> f64 {
    let mut v = v % 360.0;
    if v > 180.0 {
        v -= 360.0;
    } else if v < -180.0 {
        v += 360.0;
    }
    v
}

fn bounds_of(points: &[[f32; 2]; LM_COUNT]) -> FaceBox {
    let mut b = FaceBox {
        x0: f32::MAX,
        y0: f32::MAX,
        x1: f32::MIN,
        y1: f32::MIN,
    };
    for p in points {
        b.x0 = b.x0.min(p[0]);
        b.y0 = b.y0.min(p[1]);
        b.x1 = b.x1.max(p[0]);
        b.y1 = b.y1.max(p[1]);
    }
    b
}

fn open_camera(index: u32) -> Result<Camera, String> {
    // Probe what the camera actually supports, then reopen it directly on the
    // best format. (Letting the driver pick can land on 1 fps still-photo
    // modes, and some cams only stream NV12.)
    let mut formats = {
        let mut probe = Camera::new(
            CameraIndex::Index(index),
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate),
        )
        .map_err(|e| format!("Could not open camera {index}: {e}"))?;
        probe.compatible_camera_formats().unwrap_or_default()
    };
    formats.retain(|f| f.frame_rate() >= 10);
    formats.sort_by_key(|f| {
        let r = f.resolution();
        let res_penalty =
            (r.width() as i32 - 640).abs() + (r.height() as i32 - 480).abs();
        let fps_penalty = (30 - f.frame_rate().min(30) as i32) * 20;
        let fmt_penalty = if f.format() == FrameFormat::MJPEG { 0 } else { 8 };
        res_penalty + fps_penalty + fmt_penalty
    });
    formats.dedup();
    formats.truncate(4);
    if formats.is_empty() {
        return Err(format!("Camera {index} reports no usable video formats."));
    }

    let mut last_err = String::new();
    for fmt in formats {
        let mut cam = match Camera::new(
            CameraIndex::Index(index),
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(fmt)),
        ) {
            Ok(c) => c,
            Err(e) => {
                last_err = format!("{e}");
                continue;
            }
        };
        match cam.open_stream() {
            Ok(()) => {
                // First frames can fail while the pipeline warms up.
                for attempt in 0..20 {
                    match cam.frame() {
                        Ok(_) => return Ok(cam),
                        Err(e) => {
                            last_err = format!("no frames ({e})");
                            if attempt < 19 {
                                std::thread::sleep(std::time::Duration::from_millis(150));
                            }
                        }
                    }
                }
                let _ = cam.stop_stream();
            }
            Err(e) => last_err = format!("{e}"),
        }
    }
    Err(format!(
        "Could not start camera {index}: {last_err}. Is another app using it?"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// List what the webcam claims to support (run with `--ignored`).
    #[test]
    #[ignore]
    fn camera_formats() {
        let cam = Camera::new(
            CameraIndex::Index(0),
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate),
        )
        .expect("open");
        println!("negotiated: {:?}", cam.camera_format());
        let mut cam = cam;
        match cam.compatible_camera_formats() {
            Ok(f) => {
                for fmt in f {
                    println!("  {fmt:?}");
                }
            }
            Err(e) => println!("compatible_camera_formats failed: {e}"),
        }
    }

    /// Live smoke test (needs a webcam; run with `--ignored`): opens the
    /// camera, decodes frames, runs the full pipeline, saves one frame.
    #[test]
    #[ignore]
    fn live_camera_smoke() {
        let mut camera = open_camera(0).expect("camera 0 should open");
        let res = camera.resolution();
        let (fw, fh) = (res.width() as usize, res.height() as usize);
        println!("camera: {}x{} @ {} fps", res.width(), res.height(), camera.frame_rate());

        let mut nets = Nets::load().unwrap();
        let mut solver = pnp::Solver::new(fw as f64, fh as f64, 70.0);
        let mut points = [[0.0f32; 2]; LM_COUNT];

        let mut img = None;
        for i in 0..10 {
            let frame = camera.frame().expect("frame");
            let decoded = frame.decode_image::<RgbFormat>().expect("decode");
            assert_eq!(decoded.width() as usize, fw);
            let rgb: &[u8] = decoded.as_raw();

            let face = nets.detect_face(rgb, fw, fh).unwrap();
            if let Some(b) = face.map(|b| b.expanded(0.1, fw, fh)) {
                let conf = nets.landmarks(rgb, fw, fh, &b, &mut points).unwrap();
                let mut obs = [[0.0f64; 2]; 18];
                for (o, &idx) in obs.iter_mut().zip(pnp::CONTOUR_IDX.iter()) {
                    o[0] = points[idx][0] as f64;
                    o[1] = points[idx][1] as f64;
                }
                let pose = solver.solve(&obs);
                println!(
                    "frame {i}: face conf {conf:.2}  yaw {:+.1} pitch {:+.1} roll {:+.1} z {:.0}cm",
                    pose.yaw, pose.pitch, pose.roll, pose.z
                );
            } else {
                println!("frame {i}: no face");
            }
            img = Some(decoded);
        }

        let out = image::RgbImage::from_raw(fw as u32, fh as u32, img.unwrap().into_raw())
            .unwrap();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/target/camera_smoke.png");
        out.save(path).unwrap();
        println!("saved {path}");
    }
}

/// Downscale the frame 2x and hand it to the GUI with the landmark overlay.
fn publish_preview(
    shared: &Shared,
    rgb: &[u8],
    fw: usize,
    fh: usize,
    points: Option<&[[f32; 2]; LM_COUNT]>,
    face: &Option<FaceBox>,
) {
    let pw = fw / 2;
    let ph = fh / 2;
    let mut small = vec![0u8; pw * ph * 3];
    for y in 0..ph {
        for x in 0..pw {
            let src = ((y * 2) * fw + x * 2) * 3;
            let dst = (y * pw + x) * 3;
            small[dst..dst + 3].copy_from_slice(&rgb[src..src + 3]);
        }
    }
    let frame = PreviewFrame {
        width: pw,
        height: ph,
        rgb: small,
        points: points
            .map(|ps| ps.iter().map(|p| (p[0] / 2.0, p[1] / 2.0)).collect())
            .unwrap_or_default(),
        face: face
            .as_ref()
            .map(|b| (b.x0 / 2.0, b.y0 / 2.0, b.x1 / 2.0, b.y1 / 2.0)),
    };
    *shared.head.preview.lock().unwrap() = Some(frame);
}
