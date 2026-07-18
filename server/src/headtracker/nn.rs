//! The two neural nets, run on CPU with tract (pure Rust, no runtime DLLs):
//!
//! * Face detector: UltraFace RFB-320 (Linzaer's 1MB face detector, MIT).
//!   Input 320x240 RGB, outputs per-anchor scores + already-decoded boxes.
//! * Landmarks: AITrack/OpenSeeFace `lm_f` (MIT). Input a 224x224 face crop,
//!   outputs 66 landmark heatmaps (28x28) plus two offset map blocks.
//!
//! Both models are embedded in the exe.

use tract_onnx::prelude::*;

static DETECT_ONNX: &[u8] = include_bytes!("../../assets/models/version-RFB-320.onnx");
static LANDMARK_ONNX: &[u8] = include_bytes!("../../assets/models/lm_f.onnx");

type RunModel = TypedRunnableModel<TypedModel>;

pub const LM_COUNT: usize = 66;
const DET_W: usize = 320;
const DET_H: usize = 240;
const LM_SIZE: usize = 224;
const HM: usize = 28; // heatmap side

/// Face box in frame pixels.
#[derive(Clone, Copy, Debug)]
pub struct FaceBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl FaceBox {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    /// Grow by `frac` on every side, clamped to the frame.
    pub fn expanded(&self, frac: f32, fw: usize, fh: usize) -> FaceBox {
        let mx = self.width() * frac;
        let my = self.height() * frac;
        FaceBox {
            x0: (self.x0 - mx).max(0.0),
            y0: (self.y0 - my).max(0.0),
            x1: (self.x1 + mx).min(fw as f32 - 1.0),
            y1: (self.y1 + my).min(fh as f32 - 1.0),
        }
    }
}

pub struct Nets {
    detector: RunModel,
    landmarks: RunModel,
    det_input: Vec<f32>,
    lm_input: Vec<f32>,
}

impl Nets {
    pub fn load() -> TractResult<Nets> {
        let detector = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(DETECT_ONNX))?
            .with_input_fact(0, f32::fact([1, 3, DET_H, DET_W]).into())?
            .into_optimized()?
            .into_runnable()?;
        let landmarks = tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(LANDMARK_ONNX))?
            .with_input_fact(0, f32::fact([1, 3, LM_SIZE, LM_SIZE]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(Nets {
            detector,
            landmarks,
            det_input: vec![0.0; 3 * DET_W * DET_H],
            lm_input: vec![0.0; 3 * LM_SIZE * LM_SIZE],
        })
    }

    /// Find the most confident face in the frame. `rgb` is packed RGB8;
    /// `gain` is a brightness multiplier (see [auto_gain]) for dim rooms.
    pub fn detect_face(
        &mut self,
        rgb: &[u8],
        fw: usize,
        fh: usize,
        gain: f32,
    ) -> TractResult<Option<FaceBox>> {
        // UltraFace normalization: (px - 127) / 128.
        sample_region_nchw(
            rgb,
            fw,
            fh,
            (0.0, 0.0, fw as f32, fh as f32),
            DET_W,
            DET_H,
            &mut self.det_input,
            |c| ((c * gain).min(255.0) - 127.0) / 128.0,
        );
        let input = Tensor::from_shape(&[1, 3, DET_H, DET_W], &self.det_input)?;
        let out = self.detector.run(tvec!(input.into_tvalue()))?;

        // Identify outputs by shape: scores [1,N,2], boxes [1,N,4].
        let (scores, boxes) = if out[0].shape()[2] == 2 {
            (&out[0], &out[1])
        } else {
            (&out[1], &out[0])
        };
        let scores = scores.to_array_view::<f32>()?;
        let boxes = boxes.to_array_view::<f32>()?;
        let n = scores.shape()[1];

        let mut best: Option<(f32, FaceBox)> = None;
        for i in 0..n {
            let score = scores[[0, i, 1]];
            if score < 0.5 {
                continue;
            }
            if best.map(|(s, _)| score > s).unwrap_or(true) {
                best = Some((
                    score,
                    FaceBox {
                        x0: boxes[[0, i, 0]] * fw as f32,
                        y0: boxes[[0, i, 1]] * fh as f32,
                        x1: boxes[[0, i, 2]] * fw as f32,
                        y1: boxes[[0, i, 3]] * fh as f32,
                    },
                ));
            }
        }
        Ok(best.map(|(_, b)| b))
    }

    /// Landmarks inside `face` (already expanded), in frame pixel coords,
    /// plus the mean heatmap confidence (0..1).
    pub fn landmarks(
        &mut self,
        rgb: &[u8],
        fw: usize,
        fh: usize,
        face: &FaceBox,
        gain: f32,
        out_points: &mut [[f32; 2]; LM_COUNT],
    ) -> TractResult<f32> {
        // ImageNet normalization, RGB.
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        sample_region_nchw_per_channel(
            rgb,
            fw,
            fh,
            (face.x0, face.y0, face.width(), face.height()),
            LM_SIZE,
            LM_SIZE,
            &mut self.lm_input,
            |c, ch| ((c * gain).min(255.0) / 255.0 - MEAN[ch]) / STD[ch],
        );
        let input = Tensor::from_shape(&[1, 3, LM_SIZE, LM_SIZE], &self.lm_input)?;
        let out = self.landmarks.run(tvec!(input.into_tvalue()))?;
        let maps = out[0].to_array_view::<f32>()?; // [1, 198, 28, 28]

        let scale_x = face.width() / (LM_SIZE - 1) as f32;
        let scale_y = face.height() / (LM_SIZE - 1) as f32;
        let res = (LM_SIZE - 1) as f32;

        let mut conf_sum = 0.0;
        for lm in 0..LM_COUNT {
            let mut best = (0usize, 0usize);
            let mut best_v = f32::NEG_INFINITY;
            for r in 0..HM {
                for c in 0..HM {
                    let v = maps[[0, lm, r, c]];
                    if v > best_v {
                        best_v = v;
                        best = (r, c);
                    }
                }
            }
            let (r, c) = best;
            conf_sum += best_v;
            // Offset blocks: channels 66..132 refine the vertical position,
            // 132..198 the horizontal one (OpenSeeFace layout).
            let off_v = res * logit(maps[[0, LM_COUNT + lm, r, c]]);
            let off_h = res * logit(maps[[0, 2 * LM_COUNT + lm, r, c]]);
            let v = res * (r as f32 / (HM - 1) as f32) + off_v;
            let u = res * (c as f32 / (HM - 1) as f32) + off_h;
            out_points[lm] = [face.x0 + u * scale_x, face.y0 + v * scale_y];
        }
        Ok((conf_sum / LM_COUNT as f32).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both embedded models must load under tract and run end to end.
    #[test]
    fn models_load_and_run() {
        let mut nets = Nets::load().expect("models should load under tract");
        let rgb = vec![128u8; 640 * 480 * 3];
        let found = nets
            .detect_face(&rgb, 640, 480, 1.0)
            .expect("detector should run");
        assert!(found.is_none(), "uniform gray frame should contain no face");

        let face = FaceBox {
            x0: 100.0,
            y0: 80.0,
            x1: 420.0,
            y1: 400.0,
        };
        let mut points = [[0.0f32; 2]; LM_COUNT];
        let conf = nets
            .landmarks(&rgb, 640, 480, &face, 1.0, &mut points)
            .expect("landmark model should run");
        assert!(conf.is_finite());
        for p in &points {
            assert!(p[0].is_finite() && p[1].is_finite());
        }
    }

    /// Full pipeline on a real photo (public-domain NASA portrait): the face
    /// must be found, landmarks must land inside it with good confidence, and
    /// the solved pose must be roughly frontal. Also writes an annotated image
    /// to target/ for eyeballing.
    #[test]
    fn astronaut_end_to_end() {
        let img = image::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/test/astronaut.png"
        ))
        .expect("test image")
        .into_rgb8();
        let (fw, fh) = (img.width() as usize, img.height() as usize);
        let rgb = img.as_raw().clone();

        let mut nets = Nets::load().unwrap();
        let face = nets
            .detect_face(&rgb, fw, fh, 1.0)
            .unwrap()
            .expect("the astronaut's face should be detected")
            .expanded(0.1, fw, fh);

        let mut points = [[0.0f32; 2]; LM_COUNT];
        let conf = nets
            .landmarks(&rgb, fw, fh, &face, 1.0, &mut points)
            .unwrap();
        assert!(conf > 0.4, "confidence {conf} too low");
        for p in &points {
            assert!(
                p[0] >= face.x0 - 30.0
                    && p[0] <= face.x1 + 30.0
                    && p[1] >= face.y0 - 30.0
                    && p[1] <= face.y1 + 30.0,
                "landmark {p:?} far outside face {face:?}"
            );
        }

        let mut solver = crate::headtracker::pnp::Solver::new(fw as f64, fh as f64, 60.0);
        let mut obs = [[0.0f64; 2]; 18];
        for (o, &idx) in obs.iter_mut().zip(crate::headtracker::pnp::CONTOUR_IDX.iter()) {
            o[0] = points[idx][0] as f64;
            o[1] = points[idx][1] as f64;
        }
        let pose = solver.solve(&obs);
        // She looks roughly at the camera in this photo.
        assert!(pose.yaw.abs() < 25.0, "yaw {} not frontal", pose.yaw);
        assert!(pose.pitch.abs() < 25.0, "pitch {} not frontal", pose.pitch);
        assert!(pose.z > 0.0, "head must be in front of the camera");

        // Dump the annotated frame for visual inspection.
        let mut out = img;
        for (i, p) in points.iter().enumerate() {
            let color = if CONTOUR_IDX_SET.contains(&i) {
                image::Rgb([0u8, 255, 80])
            } else {
                image::Rgb([255u8, 210, 0])
            };
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let x = (p[0] as i32 + dx).clamp(0, out.width() as i32 - 1) as u32;
                    let y = (p[1] as i32 + dy).clamp(0, out.height() as i32 - 1) as u32;
                    out.put_pixel(x, y, color);
                }
            }
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/target/astronaut_landmarks.png");
        out.save(path).unwrap();
        println!(
            "pose: yaw {:+.1} pitch {:+.1} roll {:+.1}  xyz ({:.1}, {:.1}, {:.1})cm  conf {conf:.2} -> {path}",
            pose.yaw, pose.pitch, pose.roll, pose.x, pose.y, pose.z
        );
    }

    const CONTOUR_IDX_SET: [usize; 18] = crate::headtracker::pnp::CONTOUR_IDX;
}

/// Brightness gain for dim rooms: how much to multiply the frame so its mean
/// luma reaches a comfortable level, capped so noise isn't over-amplified.
pub fn auto_gain(rgb: &[u8]) -> f32 {
    // Green channel, sparse sampling: plenty for a mean.
    let mut sum = 0u64;
    let mut n = 0u64;
    let mut i = 1;
    while i < rgb.len() {
        sum += rgb[i] as u64;
        n += 1;
        i += 48; // every 16th pixel
    }
    if n == 0 {
        return 1.0;
    }
    let mean = sum as f32 / n as f32;
    (110.0 / mean.max(1.0)).clamp(1.0, 4.0)
}

/// Inverse sigmoid scaled the way OpenSeeFace trains its offset maps.
fn logit(p: f32) -> f32 {
    let p = p.clamp(1e-7, 1.0 - 1e-7);
    (p / (1.0 - p)).ln() / 16.0
}

/// Bilinear-sample `region` (x, y, w, h) of an RGB8 frame into an NCHW float
/// buffer of `ow` x `oh`, applying `norm` to each channel value.
fn sample_region_nchw(
    rgb: &[u8],
    fw: usize,
    fh: usize,
    region: (f32, f32, f32, f32),
    ow: usize,
    oh: usize,
    dest: &mut [f32],
    norm: impl Fn(f32) -> f32,
) {
    sample_region_nchw_per_channel(rgb, fw, fh, region, ow, oh, dest, |c, _| norm(c))
}

fn sample_region_nchw_per_channel(
    rgb: &[u8],
    fw: usize,
    fh: usize,
    region: (f32, f32, f32, f32),
    ow: usize,
    oh: usize,
    dest: &mut [f32],
    norm: impl Fn(f32, usize) -> f32,
) {
    let (rx, ry, rw, rh) = region;
    let step_x = rw / (ow - 1) as f32;
    let step_y = rh / (oh - 1) as f32;
    let plane = ow * oh;

    for oy in 0..oh {
        let sy = (ry + oy as f32 * step_y).clamp(0.0, fh as f32 - 1.001);
        let y0 = sy as usize;
        let fy = sy - y0 as f32;
        let y1 = (y0 + 1).min(fh - 1);
        for ox in 0..ow {
            let sx = (rx + ox as f32 * step_x).clamp(0.0, fw as f32 - 1.001);
            let x0 = sx as usize;
            let fx = sx - x0 as f32;
            let x1 = (x0 + 1).min(fw - 1);

            let i00 = (y0 * fw + x0) * 3;
            let i01 = (y0 * fw + x1) * 3;
            let i10 = (y1 * fw + x0) * 3;
            let i11 = (y1 * fw + x1) * 3;
            for ch in 0..3 {
                let top = rgb[i00 + ch] as f32 * (1.0 - fx) + rgb[i01 + ch] as f32 * fx;
                let bot = rgb[i10 + ch] as f32 * (1.0 - fx) + rgb[i11 + ch] as f32 * fx;
                dest[ch * plane + oy * ow + ox] = norm(top * (1.0 - fy) + bot * fy, ch);
            }
        }
    }
}
