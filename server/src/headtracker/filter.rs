//! One Euro filter: adaptive low-pass that stays smooth on slow movements and
//! snappy on fast ones. The standard choice for head/hand tracking.

use std::f32::consts::PI;

pub struct OneEuro {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    x_prev: Option<f32>,
    dx_prev: f32,
}

impl OneEuro {
    pub fn new(min_cutoff: f32, beta: f32) -> OneEuro {
        OneEuro {
            min_cutoff,
            beta,
            d_cutoff: 1.0,
            x_prev: None,
            dx_prev: 0.0,
        }
    }

    pub fn set_params(&mut self, min_cutoff: f32, beta: f32) {
        self.min_cutoff = min_cutoff;
        self.beta = beta;
    }

    pub fn reset(&mut self) {
        self.x_prev = None;
        self.dx_prev = 0.0;
    }

    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        let Some(prev) = self.x_prev else {
            self.x_prev = Some(x);
            return x;
        };
        let dt = dt.clamp(1e-3, 0.5);

        let dx = (x - prev) / dt;
        let dx_hat = lowpass(dx, self.dx_prev, alpha(dt, self.d_cutoff));
        self.dx_prev = dx_hat;

        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let x_hat = lowpass(x, prev, alpha(dt, cutoff));
        self.x_prev = Some(x_hat);
        x_hat
    }
}

fn alpha(dt: f32, cutoff: f32) -> f32 {
    let tau = 1.0 / (2.0 * PI * cutoff);
    dt / (dt + tau)
}

fn lowpass(x: f32, prev: f32, a: f32) -> f32 {
    prev + a * (x - prev)
}
