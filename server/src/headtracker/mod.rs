//! Webcam head tracking: camera -> face detection -> landmarks -> PnP pose ->
//! One Euro smoothing -> FreeTrack/TrackIR output for games.
//!
//! Runs on its own thread, independent of the phone/controller server, so you
//! can use head tracking with or without the phone connected.

mod filter;
mod lowlight;
mod nn;
mod pnp;

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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
    /// Per-axis enable, in [AXIS_NAMES] order: yaw, pitch, roll, x, y, z.
    pub axis_on: [bool; 6],
    /// Per-axis mirror (flips the direction).
    pub axis_mirror: [bool; 6],
    /// Force short manual exposure + hardware gain so the camera keeps full
    /// frame rate in a dark room (the usual cause of tracking lag).
    pub low_light: bool,
}

pub const AXIS_NAMES: [&str; 6] = ["Yaw", "Pitch", "Roll", "Move X", "Move Y", "Move Z"];

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            camera: 0,
            fov: 70.0,
            smoothing: 0.4,
            rot_gain: 2.5,
            pos_gain: 1.0,
            axis_on: [true; 6],
            axis_mirror: [false; 6],
            low_light: true,
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

/// Newest decoded camera frame; older ones are dropped so a slow processing
/// step never builds up queue latency.
struct FrameSlot {
    frame: Mutex<Option<(u64, Vec<u8>)>>,
    ready: Condvar,
}

/// Latest pose sample + velocity, feeding the 100 Hz output thread.
struct OutState {
    pose: HeadPose,
    vel: [f32; 6],
    at: Instant,
}

/// The shortest shutter we try first (1/32 s ~ 30 fps) and the longest we
/// fall back to when the room is too dark to see a face (1/8 s ~ 8 fps,
/// which is no worse than what auto exposure would do).
const EXPOSURE_FAST: i32 = -5;
const EXPOSURE_SLOW: i32 = -3;

fn apply_low_light(shared: &Arc<Shared>, camera_name: &str, on: bool, ev: i32) {
    if on {
        match lowlight::force_low_light(camera_name, ev) {
            Ok(a) if a.exposure => shared.log(format!(
                "Low light boost: manual exposure 1/{} s (keeps the fps up in the dark).",
                1u32 << (-ev).max(0)
            )),
            Ok(_) => shared.log("Low light boost: this camera exposes no exposure control."),
            Err(e) => shared.log(format!("Low light boost failed: {e}")),
        }
    } else {
        match lowlight::restore_auto(camera_name) {
            Ok(()) => shared.log("Camera exposure back on automatic."),
            Err(e) => shared.log(format!("Could not restore auto exposure: {e}")),
        }
    }
}

/// Capture thread: owns the camera, decodes frames, publishes the latest one.
fn capture_loop(
    camera_index: u32,
    slot: Arc<FrameSlot>,
    stop: Arc<AtomicBool>,
    // Reports (width, height, fps, device name) once on success, or the error.
    started: mpsc::Sender<Result<(usize, usize, u32, String), String>>,
    fail: Arc<Mutex<Option<String>>>,
) {
    let mut camera = match open_camera(camera_index) {
        Ok(c) => c,
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };
    let res = camera.resolution();
    let _ = started.send(Ok((
        res.width() as usize,
        res.height() as usize,
        camera.frame_rate(),
        camera.info().human_name(),
    )));

    let mut seq = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let frame = match camera.frame() {
            Ok(f) => f,
            Err(e) => {
                *fail.lock().unwrap() = Some(format!("Camera stopped: {e}"));
                break;
            }
        };
        let Ok(img) = frame.decode_image::<RgbFormat>() else {
            continue;
        };
        seq += 1;
        *slot.frame.lock().unwrap() = Some((seq, img.into_raw()));
        slot.ready.notify_one();
    }
    slot.ready.notify_all();
}

fn track(
    shared: &Arc<Shared>,
    stop: &Arc<AtomicBool>,
    settings: &Arc<Mutex<Settings>>,
) -> Result<(), String> {
    let cfg = *settings.lock().unwrap();

    let writer = FreetrackWriter::new(|m| shared.log(m))?;

    shared.log("Loading face tracking models...");
    let t0 = Instant::now();
    let mut nets = Nets::load().map_err(|e| format!("Could not load NN models: {e}"))?;
    shared.log(format!(
        "Models ready in {:.1}s. Opening camera {}...",
        t0.elapsed().as_secs_f32(),
        cfg.camera
    ));

    // Camera runs on its own thread; we always process the newest frame and
    // silently drop any we're too slow for, so latency can't accumulate.
    let slot = Arc::new(FrameSlot {
        frame: Mutex::new(None),
        ready: Condvar::new(),
    });
    let cam_fail: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let (started_tx, started_rx) = mpsc::channel();
    let capture = {
        let slot = Arc::clone(&slot);
        let stop = Arc::clone(stop);
        let fail = Arc::clone(&cam_fail);
        std::thread::spawn(move || capture_loop(cfg.camera, slot, stop, started_tx, fail))
    };
    let cam_result = started_rx
        .recv()
        .map_err(|_| "Camera thread died while starting".to_string())?;
    let (fw, fh, cam_fps, cam_name) = match cam_result {
        Ok(v) => v,
        Err(e) => {
            let _ = capture.join();
            return Err(e);
        }
    };
    shared.log(format!("Camera open: {fw}x{fh} @ {cam_fps} fps."));
    set_status(shared, |s| s.camera_ok = true);

    // Keep the sensor at a high frame rate in dark rooms (auto exposure would
    // stretch the shutter and collapse the fps, which feels like lag). If the
    // room turns out too dark to track at the fast shutter, the loop below
    // relaxes the exposure one stop at a time until the face comes back.
    let mut low_light_on = cfg.low_light;
    let mut exposure_ev = EXPOSURE_FAST;
    let mut no_face_frames = 0u32;
    if low_light_on {
        apply_low_light(shared, &cam_name, true, exposure_ev);
    }

    // The game-facing pose is published at 100 Hz on its own thread, linearly
    // extrapolated from the last two camera samples, so the in-game view
    // moves smoothly even when the camera delivers far fewer frames.
    let out_state = Arc::new(Mutex::new(OutState {
        pose: HeadPose::default(),
        vel: [0.0; 6],
        at: Instant::now(),
    }));
    let game_id = Arc::new(AtomicI32::new(0));
    let output = {
        let out_state = Arc::clone(&out_state);
        let game_id = Arc::clone(&game_id);
        let stop = Arc::clone(stop);
        let settings = Arc::clone(settings);
        std::thread::spawn(move || {
            let mut writer = writer;
            // The published pose chases the newest camera sample with a
            // critically damped spring (the "smooth damp" used for camera
            // follow in games): C1-continuous, natural ease-in/ease-out,
            // no velocity jumps when a new camera frame lands.
            let mut cur = [0.0f32; 6];
            let mut curv = [0.0f32; 6];
            let mut primed = false;
            let mut last_tick = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let (pose, vel, at) = {
                    let s = out_state.lock().unwrap();
                    (s.pose, s.vel, s.at)
                };
                // Target = latest sample plus a bounded velocity prediction,
                // so the easing doesn't add lag on sustained movement.
                let ahead = at.elapsed().as_secs_f32().min(0.15);
                let target = [
                    pose.yaw + vel[0] * ahead,
                    pose.pitch + vel[1] * ahead,
                    pose.roll + vel[2] * ahead,
                    pose.x + vel[3] * ahead,
                    pose.y + vel[4] * ahead,
                    pose.z + vel[5] * ahead,
                ];
                if !primed {
                    cur = target;
                    primed = true;
                }

                let now = Instant::now();
                let dt = now.duration_since(last_tick).as_secs_f32().clamp(1e-3, 0.1);
                last_tick = now;

                // Response time from the smoothing slider: snappy to floaty.
                let s = settings.lock().unwrap().smoothing.clamp(0.0, 1.0);
                let tau = 0.04 + (0.22 - 0.04) * s;
                let omega = 2.0 / tau;
                let decay = (-omega * dt).exp();
                for i in 0..6 {
                    let x = cur[i] - target[i];
                    let temp = (curv[i] + omega * x) * dt;
                    cur[i] = target[i] + (x + temp) * decay;
                    curv[i] = (curv[i] - omega * temp) * decay;
                }

                let p = HeadPose {
                    yaw: cur[0],
                    pitch: cur[1],
                    roll: cur[2],
                    x: cur[3],
                    y: cur[4],
                    z: cur[5],
                };
                game_id.store(writer.write(p), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(10));
            }
            // Dropping the writer zeroes the pose for the game.
        })
    };

    let mut solver = pnp::Solver::new(fw as f64, fh as f64, cfg.fov as f64);
    let mut filters: Vec<OneEuro> = (0..6).map(|_| OneEuro::new(1.0, 0.1)).collect();

    let mut face: Option<FaceBox> = None;
    let mut points = [[0.0f32; 2]; LM_COUNT];
    let mut low_conf_frames = 0u32;
    let mut center: Option<pnp::Pose> = None;
    let mut last_frame = Instant::now();
    let mut fps = 0.0f32;
    let mut last_pose = HeadPose::default();
    let mut vel = [0.0f32; 6];
    let mut prev_out: Option<(HeadPose, Instant)> = None;

    let mut last_seq = 0u64;
    while !stop.load(Ordering::Relaxed) {
        // Grab the newest frame, waiting briefly if none arrived yet.
        let rgb: Vec<u8> = {
            let guard = slot.frame.lock().unwrap();
            let (guard, _) = slot
                .ready
                .wait_timeout_while(guard, Duration::from_millis(250), |f| {
                    !matches!(f, Some((seq, _)) if *seq > last_seq)
                })
                .unwrap();
            let mut guard = guard;
            match guard.take() {
                Some((seq, rgb)) => {
                    last_seq = seq;
                    rgb
                }
                None => {
                    if let Some(e) = cam_fail.lock().unwrap().take() {
                        set_status(shared, |s| s.camera_ok = false);
                        let _ = capture.join();
                        return Err(e);
                    }
                    continue;
                }
            }
        };
        let rgb: &[u8] = &rgb;

        let now = Instant::now();
        let dt = now.duration_since(last_frame).as_secs_f32().clamp(1e-3, 0.5);
        last_frame = now;
        fps = fps * 0.9 + (1.0 / dt) * 0.1;

        let cfg = *settings.lock().unwrap();
        if cfg.low_light != low_light_on {
            low_light_on = cfg.low_light;
            exposure_ev = EXPOSURE_FAST;
            no_face_frames = 0;
            apply_low_light(shared, &cam_name, low_light_on, exposure_ev);
        }

        // Brightness compensation so tracking still works in a dim room.
        let gain = nn::auto_gain(rgb);

        // Redetect when we have no face or the landmarks have been bad a while.
        if face.is_none() || low_conf_frames > 10 {
            face = nets
                .detect_face(rgb, fw, fh, gain)
                .map_err(|e| format!("Face detector failed: {e}"))?
                .map(|b| b.expanded(0.1, fw, fh));
            low_conf_frames = 0;
        }

        let mut have_pose = false;
        if let Some(cur) = face {
            let conf = nets
                .landmarks(rgb, fw, fh, &cur, gain, &mut points)
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

                // Base signs tuned against BeamNG/ETS2 in live testing; the
                // per-axis toggles let the user disable or mirror each one.
                let axis = |i: usize, v: f32| -> f32 {
                    if !cfg.axis_on[i] {
                        0.0
                    } else if cfg.axis_mirror[i] {
                        -v
                    } else {
                        v
                    }
                };
                last_pose = HeadPose {
                    yaw: axis(0, wrap_deg(sm.yaw - c.yaw) as f32 * cfg.rot_gain),
                    pitch: axis(1, -wrap_deg(sm.pitch - c.pitch) as f32 * cfg.rot_gain),
                    roll: axis(2, wrap_deg(sm.roll - c.roll) as f32 * cfg.rot_gain),
                    x: axis(3, (sm.x - c.x) as f32 * cfg.pos_gain),
                    // Camera y is down; FreeTrack Y is up.
                    y: axis(4, -(sm.y - c.y) as f32 * cfg.pos_gain),
                    z: axis(5, (sm.z - c.z) as f32 * cfg.pos_gain),
                };
                have_pose = true;
            }
        }

        // Too dark to see anyone at this shutter speed? Trade fps for light,
        // one stop at a time (~1.5 s per step).
        if low_light_on && !have_pose {
            no_face_frames += 1;
            if no_face_frames > 45 && exposure_ev < EXPOSURE_SLOW {
                exposure_ev += 1;
                no_face_frames = 0;
                shared.log("No face at this shutter speed - letting more light in.".to_string());
                apply_low_light(shared, &cam_name, true, exposure_ev);
            }
        } else {
            no_face_frames = 0;
        }

        // Feed the output thread: fresh velocity while tracking, zero (hold
        // the last pose) while the face is briefly lost.
        if have_pose {
            if let Some((pp, pt)) = prev_out {
                let dt = now.duration_since(pt).as_secs_f32().clamp(1e-3, 0.5);
                let nv = [
                    (last_pose.yaw - pp.yaw) / dt,
                    (last_pose.pitch - pp.pitch) / dt,
                    (last_pose.roll - pp.roll) / dt,
                    (last_pose.x - pp.x) / dt,
                    (last_pose.y - pp.y) / dt,
                    (last_pose.z - pp.z) / dt,
                ];
                for (v, n) in vel.iter_mut().zip(nv) {
                    *v = *v * 0.5 + n * 0.5;
                }
            }
            prev_out = Some((last_pose, now));
        } else {
            vel = [0.0; 6];
            prev_out = None;
        }
        *out_state.lock().unwrap() = OutState {
            pose: last_pose,
            vel,
            at: now,
        };

        set_status(shared, |s| {
            s.camera_ok = true;
            s.face = have_pose;
            s.fps = fps;
            s.game_id = game_id.load(Ordering::Relaxed);
            s.yaw = last_pose.yaw;
            s.pitch = last_pose.pitch;
        });

        if shared.head.preview_on.load(Ordering::Relaxed) {
            publish_preview(shared, rgb, fw, fh, have_pose.then_some(&points), &face);
        }
    }
    if low_light_on {
        apply_low_light(shared, &cam_name, false, EXPOSURE_FAST);
    }
    let _ = capture.join();
    let _ = output.join();
    Ok(())
}

/// Map the 0..1 smoothing slider onto One Euro parameters. Kept light: this
/// stage only tames landmark jitter per sample - the natural easing between
/// samples comes from the critically damped spring in the output thread,
/// which the same slider also drives.
fn smoothing_params(s: f32) -> (f32, f32) {
    let s = s.clamp(0.0, 1.0);
    let min_cutoff = 4.0 + (1.0 - 4.0) * s;
    let beta = 0.6 + (0.2 - 0.6) * s;
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

        let name = camera.info().human_name();
        match lowlight::force_low_light(&name, EXPOSURE_FAST) {
            Ok(a) => println!("low light boost: exposure {} gain {}", a.exposure, a.gain),
            Err(e) => println!("low light boost failed: {e}"),
        }

        let mut nets = Nets::load().unwrap();
        let mut solver = pnp::Solver::new(fw as f64, fh as f64, 70.0);
        let mut points = [[0.0f32; 2]; LM_COUNT];

        for i in 0..30 {
            let t0 = Instant::now();
            let frame = camera.frame().expect("frame");
            let t_cap = t0.elapsed().as_secs_f32() * 1e3;
            let t0 = Instant::now();
            let decoded = frame.decode_image::<RgbFormat>().expect("decode");
            let t_dec = t0.elapsed().as_secs_f32() * 1e3;
            assert_eq!(decoded.width() as usize, fw);
            let rgb: &[u8] = decoded.as_raw();

            let gain = nn::auto_gain(rgb);
            let t0 = Instant::now();
            let face = nets.detect_face(rgb, fw, fh, gain).unwrap();
            let t_det = t0.elapsed().as_secs_f32() * 1e3;
            if let Some(b) = face.map(|b| b.expanded(0.1, fw, fh)) {
                let t0 = Instant::now();
                let conf = nets.landmarks(rgb, fw, fh, &b, gain, &mut points).unwrap();
                let t_lm = t0.elapsed().as_secs_f32() * 1e3;
                let mut obs = [[0.0f64; 2]; 18];
                for (o, &idx) in obs.iter_mut().zip(pnp::CONTOUR_IDX.iter()) {
                    o[0] = points[idx][0] as f64;
                    o[1] = points[idx][1] as f64;
                }
                let t0 = Instant::now();
                let pose = solver.solve(&obs);
                let t_solve = t0.elapsed().as_secs_f32() * 1e3;
                println!(
                    "frame {i}: cap {t_cap:.0} dec {t_dec:.0} det {t_det:.0} lm {t_lm:.0} solve {t_solve:.1} ms  conf {conf:.2}  yaw {:+.1} pitch {:+.1} roll {:+.1} z {:.0}cm",
                    pose.yaw, pose.pitch, pose.roll, pose.z
                );
            } else {
                println!("frame {i}: no face (cap {t_cap:.0} dec {t_dec:.0} det {t_det:.0} ms, gain {gain:.1})");
            }
        }
        let _ = lowlight::restore_auto(&name);
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
