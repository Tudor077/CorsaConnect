//! 6DoF head pose from 2D landmarks: a small Levenberg-Marquardt PnP solver
//! over a fixed 18-point rigid face model (jaw corners, chin, nose, eye
//! corners — the stable bony parts, no mouth or brows).
//!
//! Camera frame: x right, y down, z forward (into the scene). The face model
//! is expressed in the same frame for a head at rest looking straight into
//! the camera, nose tip at the origin.

/// Landmark indices (66-point layout) matched to [MODEL3D] row by row.
pub const CONTOUR_IDX: [usize; 18] = [
    0, 1, 8, 15, 16, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 39, 42, 45,
];

/// Rigid face model. Derived from the widely used AITrack/OpenSeeFace contour
/// model, re-expressed in our camera-aligned right-handed frame (their x axis
/// is mirrored). Units: ~16 cm per unit.
pub const MODEL3D: [[f64; 3]; 18] = [
    [-0.45517698, -0.30089578, 0.76442945], // 0  jaw, subject's right (image left)
    [-0.44899884, -0.16699584, 0.765143],   // 1
    [0.0, 0.621079, 0.28729478],            // 8  chin
    [0.44899884, -0.16699584, 0.765143],    // 15
    [0.45517698, -0.30089578, 0.76442945],  // 16 jaw, subject's left
    [0.0, -0.2933326, 0.1375821],           // 27 nose bridge top
    [0.0, -0.1948287, 0.06915811],          // 28
    [0.0, -0.10384402, 0.00915182],         // 29
    [0.0, 0.0, 0.0],                        // 30 nose tip
    [-0.08062635, 0.04127607, 0.13416104],  // 31 nostril right
    [-0.04643935, 0.05767522, 0.10299063],  // 32
    [0.0, 0.06875312, 0.09054535],          // 33
    [0.04643935, 0.05767522, 0.10299063],   // 34
    [0.08062635, 0.04127607, 0.13416104],   // 35 nostril left
    [-0.31590518, -0.2983375, 0.2851074],   // 36 right eye outer corner
    [-0.13122973, -0.28444737, 0.23423915], // 39 right eye inner corner
    [0.13122973, -0.28444737, 0.23423915],  // 42 left eye inner corner
    [0.31590518, -0.2983375, 0.2851074],    // 45 left eye outer corner
];

/// Approximate size of one model unit, used to report translation in cm.
pub const UNIT_CM: f64 = 16.0;

/// Solved pose: rotation in degrees (positive yaw = looking left, positive
/// pitch = up), translation in cm in the camera frame (x right, y down,
/// z away from camera).
#[derive(Clone, Copy, Default, Debug)]
pub struct Pose {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub struct Solver {
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    /// rvec (axis-angle) + tvec, kept warm between frames.
    state: [f64; 6],
}

impl Solver {
    /// `fov_deg` is the camera's diagonal field of view.
    pub fn new(width: f64, height: f64, fov_deg: f64) -> Solver {
        let diag = (width * width + height * height).sqrt();
        let fov = fov_deg.to_radians();
        let fov_w = fov * width / diag;
        let fov_h = fov * height / diag;
        Solver {
            fx: 0.5 * width / (0.5 * fov_w).tan(),
            fy: 0.5 * height / (0.5 * fov_h).tan(),
            cx: width / 2.0,
            cy: height / 2.0,
            state: [0.0, 0.0, 0.0, 0.0, 0.0, 4.0],
        }
    }

    pub fn reset(&mut self) {
        self.state = [0.0, 0.0, 0.0, 0.0, 0.0, 4.0];
    }

    /// Observed pixel positions matching [CONTOUR_IDX]/[MODEL3D] row order.
    pub fn solve(&mut self, obs: &[[f64; 2]; 18]) -> Pose {
        let mut lambda = 1e-3;
        let mut err = self.residual_norm(&self.state.clone(), obs);

        for _ in 0..12 {
            let (jtj, jtr) = self.normal_equations(&self.state.clone(), obs);
            let mut improved = false;
            for _ in 0..6 {
                let mut damped = jtj;
                for i in 0..6 {
                    damped[i][i] += lambda * (1.0 + jtj[i][i]);
                }
                let Some(delta) = solve6(&damped, &jtr) else {
                    lambda *= 10.0;
                    continue;
                };
                let mut cand = self.state;
                for i in 0..6 {
                    cand[i] -= delta[i];
                }
                // Keep the head in front of the camera.
                cand[5] = cand[5].clamp(0.5, 40.0);
                let cand_err = self.residual_norm(&cand, obs);
                if cand_err < err {
                    self.state = cand;
                    err = cand_err;
                    lambda = (lambda * 0.3).max(1e-6);
                    improved = true;
                    break;
                }
                lambda *= 10.0;
            }
            if !improved {
                break;
            }
        }

        self.to_pose()
    }

    fn project(&self, state: &[f64; 6], p: &[f64; 3]) -> [f64; 2] {
        let r = rotate(&[state[0], state[1], state[2]], p);
        let q = [r[0] + state[3], r[1] + state[4], r[2] + state[5]];
        let z = q[2].max(0.05);
        [self.fx * q[0] / z + self.cx, self.fy * q[1] / z + self.cy]
    }

    fn residual_norm(&self, state: &[f64; 6], obs: &[[f64; 2]; 18]) -> f64 {
        let mut sum = 0.0;
        for (p, o) in MODEL3D.iter().zip(obs) {
            let uv = self.project(state, p);
            let du = uv[0] - o[0];
            let dv = uv[1] - o[1];
            sum += du * du + dv * dv;
        }
        sum
    }

    /// J^T J and J^T r with a numerical Jacobian; 18 points x 6 params is
    /// small enough that finite differences cost nothing.
    fn normal_equations(
        &self,
        state: &[f64; 6],
        obs: &[[f64; 2]; 18],
    ) -> ([[f64; 6]; 6], [f64; 6]) {
        const EPS: f64 = 1e-5;
        let mut jtj = [[0.0; 6]; 6];
        let mut jtr = [0.0; 6];

        for (p, o) in MODEL3D.iter().zip(obs) {
            let base = self.project(state, p);
            let res = [base[0] - o[0], base[1] - o[1]];
            let mut jac = [[0.0; 6]; 2];
            for k in 0..6 {
                let mut s = *state;
                s[k] += EPS;
                let bumped = self.project(&s, p);
                jac[0][k] = (bumped[0] - base[0]) / EPS;
                jac[1][k] = (bumped[1] - base[1]) / EPS;
            }
            for i in 0..6 {
                for j in 0..6 {
                    jtj[i][j] += jac[0][i] * jac[0][j] + jac[1][i] * jac[1][j];
                }
                jtr[i] += jac[0][i] * res[0] + jac[1][i] * res[1];
            }
        }
        (jtj, jtr)
    }

    fn to_pose(&self) -> Pose {
        let r = &[self.state[0], self.state[1], self.state[2]];
        // Head forward = toward the camera at rest.
        let f = rotate(r, &[0.0, 0.0, -1.0]);
        // Head up (model y points down).
        let u = rotate(r, &[0.0, -1.0, 0.0]);

        // Positive yaw = subject turns to their left (nose toward camera +x).
        let yaw = f[0].atan2(-f[2]).to_degrees();
        // Positive pitch = looking up (camera -y).
        let pitch = (-f[1]).atan2((f[0] * f[0] + f[2] * f[2]).sqrt()).to_degrees();
        // Positive roll = head tilts toward the subject's right shoulder.
        let roll = u[0].atan2(-u[1]).to_degrees();

        Pose {
            yaw,
            pitch,
            roll,
            x: self.state[3] * UNIT_CM,
            y: self.state[4] * UNIT_CM,
            z: self.state[5] * UNIT_CM,
        }
    }
}

/// Rodrigues rotation of `p` by axis-angle `r`.
fn rotate(r: &[f64; 3], p: &[f64; 3]) -> [f64; 3] {
    let theta = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
    if theta < 1e-12 {
        return *p;
    }
    let k = [r[0] / theta, r[1] / theta, r[2] / theta];
    let (s, c) = theta.sin_cos();
    let kxp = [
        k[1] * p[2] - k[2] * p[1],
        k[2] * p[0] - k[0] * p[2],
        k[0] * p[1] - k[1] * p[0],
    ];
    let kdp = k[0] * p[0] + k[1] * p[1] + k[2] * p[2];
    [
        p[0] * c + kxp[0] * s + k[0] * kdp * (1.0 - c),
        p[1] * c + kxp[1] * s + k[1] * kdp * (1.0 - c),
        p[2] * c + kxp[2] * s + k[2] * kdp * (1.0 - c),
    ]
}

/// Solve a 6x6 linear system via Gaussian elimination with partial pivoting.
fn solve6(a: &[[f64; 6]; 6], b: &[f64; 6]) -> Option<[f64; 6]> {
    let mut m = *a;
    let mut v = *b;
    for col in 0..6 {
        let mut pivot = col;
        for row in (col + 1)..6 {
            if m[row][col].abs() > m[pivot][col].abs() {
                pivot = row;
            }
        }
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        v.swap(col, pivot);
        for row in (col + 1)..6 {
            let f = m[row][col] / m[col][col];
            for k in col..6 {
                m[row][k] -= f * m[col][k];
            }
            v[row] -= f * v[col];
        }
    }
    let mut x = [0.0; 6];
    for row in (0..6).rev() {
        let mut sum = v[row];
        for k in (row + 1)..6 {
            sum -= m[row][k] * x[k];
        }
        x[row] = sum / m[row][row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project the model with a known pose, solve, and expect to recover it.
    #[test]
    fn recovers_synthetic_pose() {
        let mut solver = Solver::new(640.0, 480.0, 60.0);
        let truth = [0.15, -0.25, 0.05, 0.3, -0.2, 4.5];

        let mut obs = [[0.0; 2]; 18];
        for (i, p) in MODEL3D.iter().enumerate() {
            obs[i] = solver.project(&truth, p);
        }
        let pose = solver.solve(&obs);

        // Rebuild the truth pose angles for comparison.
        let f = rotate(&[truth[0], truth[1], truth[2]], &[0.0, 0.0, -1.0]);
        let want_yaw = f[0].atan2(-f[2]).to_degrees();
        let want_pitch = (-f[1]).atan2((f[0] * f[0] + f[2] * f[2]).sqrt()).to_degrees();

        assert!((pose.yaw - want_yaw).abs() < 0.5, "yaw {} vs {}", pose.yaw, want_yaw);
        assert!((pose.pitch - want_pitch).abs() < 0.5);
        assert!((pose.z - truth[5] * UNIT_CM).abs() < 1.0);
    }

    #[test]
    fn straight_ahead_is_zero() {
        let mut solver = Solver::new(640.0, 480.0, 60.0);
        let truth = [0.0, 0.0, 0.0, 0.0, 0.0, 4.0];
        let mut obs = [[0.0; 2]; 18];
        for (i, p) in MODEL3D.iter().enumerate() {
            obs[i] = solver.project(&truth, p);
        }
        let pose = solver.solve(&obs);
        assert!(pose.yaw.abs() < 0.2 && pose.pitch.abs() < 0.2 && pose.roll.abs() < 0.2);
    }
}
