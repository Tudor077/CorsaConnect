//! Parser for BeamNG's MotionSim ("OutSim") UDP packet.
//!
//! Unlike OutGauge (dashboard data: speed, rpm, pedals), MotionSim streams the
//! car's physics: world velocity, acceleration (gravity excluded) and
//! orientation. That's what lets us detect a real slide (slip angle between
//! where the car points and where it's actually going) and a crash (an
//! acceleration spike far beyond anything braking or cornering can produce).
//!
//! Enable it in BeamNG: Options -> Other -> Protocols -> OutSim / MotionSim,
//! pointed at the PC on port 4445. 76 bytes, little-endian, header "BNG1".

/// Length of a MotionSim packet: 4-byte header + 18 little-endian floats.
pub const MOTIONSIM_LEN: usize = 76;

/// The fields we actually use out of the packet.
pub struct Motion {
    pub vel: [f32; 3], // world velocity (m/s)
    pub acc: [f32; 3], // world acceleration, gravity excluded (m/s^2)
    pub yaw: f32,      // heading angle (rad)
}

/// Parse a MotionSim packet, or `None` if it isn't one.
pub fn parse(buf: &[u8]) -> Option<Motion> {
    if buf.len() < MOTIONSIM_LEN || &buf[0..4] != b"BNG1" {
        return None;
    }
    // Layout after the 4-byte "BNG1" header, all f32:
    //   4  posX  8 posY 12 posZ
    //  16  velX 20 velY 24 velZ
    //  28  accX 32 accY 36 accZ
    //  40  upX  44 upY  48 upZ
    //  52  rollPos 56 pitchPos 60 yawPos
    //  64  rollVel 68 pitchVel 72 yawVel
    let f = |off: usize| f32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
    Some(Motion {
        vel: [f(16), f(20), f(24)],
        acc: [f(28), f(32), f(36)],
        yaw: f(60),
    })
}

use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// Wrap an angle into -PI..PI.
fn wrap(a: f32) -> f32 {
    let mut x = a;
    while x > PI {
        x -= TAU;
    }
    while x < -PI {
        x += TAU;
    }
    x
}

/// Slip detector that calibrates ONCE, then stays fixed.
///
/// MotionSim velocity is in the WORLD frame, so the direction of travel
/// `atan2(velY, velX)` only equals the car's slide direction once you subtract
/// the heading. BeamNG's yaw convention (sign, zero-reference, radians-vs-deg)
/// isn't something we can assume, so we learn it - but learning *continuously*
/// made slip jitter (the lock kept drifting). Instead we accumulate a few
/// seconds of moving samples that include some turning, solve the convention
/// once (circular-mean offset, plus which sign of yaw stays constant), then
/// freeze. After that, slip = how far travel deviates from that locked heading
/// relation = the actual slide. Recalibrate by Stop + Launch.
pub struct SlipEstimator {
    n: u32,
    // Circular accumulators for the two sign hypotheses and for yaw itself.
    sin_a: f32, cos_a: f32, // travel - yaw
    sin_b: f32, cos_b: f32, // travel + yaw
    sin_y: f32, cos_y: f32, // yaw, to confirm the heading actually varied
    locked: bool,
    sign: f32,
    offset: f32,
    degrees: bool,
}

impl SlipEstimator {
    pub fn new() -> Self {
        SlipEstimator {
            n: 0,
            sin_a: 0.0, cos_a: 0.0,
            sin_b: 0.0, cos_b: 0.0,
            sin_y: 0.0, cos_y: 0.0,
            locked: false,
            sign: 1.0,
            offset: 0.0,
            degrees: false,
        }
    }

    /// Feed a packet, get back slip as a 0..1 fraction of a 90-degree slide.
    pub fn update(&mut self, m: &Motion) -> f32 {
        let (vx, vy) = (m.vel[0], m.vel[1]);
        let speed = (vx * vx + vy * vy).sqrt();
        if speed < 2.0 {
            return 0.0;
        }

        // Heading in radians (some builds may report yaw in degrees).
        let mut yaw = m.yaw;
        if yaw.abs() > 6.5 {
            self.degrees = true;
        }
        if self.degrees {
            yaw = yaw.to_radians();
        }

        let travel = vy.atan2(vx);

        if !self.locked {
            let a = wrap(travel - yaw);
            let b = wrap(travel + yaw);
            self.sin_a += a.sin(); self.cos_a += a.cos();
            self.sin_b += b.sin(); self.cos_b += b.cos();
            self.sin_y += yaw.sin(); self.cos_y += yaw.cos();
            self.n += 1;

            let nf = self.n as f32;
            // Resultant length ~1 means the angle was basically constant. We need
            // the heading to have varied (r_y low) so the two sign hypotheses are
            // actually distinguishable.
            let r_y = self.sin_y.hypot(self.cos_y) / nf;
            if self.n >= 180 && r_y < 0.95 {
                let r_a = self.sin_a.hypot(self.cos_a) / nf;
                let r_b = self.sin_b.hypot(self.cos_b) / nf;
                if r_a >= r_b {
                    self.sign = 1.0;
                    self.offset = self.sin_a.atan2(self.cos_a);
                } else {
                    self.sign = -1.0;
                    self.offset = self.sin_b.atan2(self.cos_b);
                }
                self.locked = true;
            }
            return 0.0;
        }

        let err = wrap(travel - self.sign * yaw - self.offset);
        (err.abs() / FRAC_PI_2).min(1.0)
    }
}

/// Horizontal acceleration mapped to a 0..1 impact strength. Hard braking sits
/// near ~1g (10 m/s^2), so anything past a few g is a hit.
pub fn impact_fraction(m: &Motion) -> f32 {
    const IMPACT_MIN: f32 = 30.0; // ~3g: below this it's just driving
    const IMPACT_MAX: f32 = 150.0; // ~15g: a solid crash
    let (ax, ay) = (m.acc[0], m.acc[1]);
    let acc_h = (ax * ax + ay * ay).sqrt();
    ((acc_h - IMPACT_MIN) / (IMPACT_MAX - IMPACT_MIN)).clamp(0.0, 1.0)
}
