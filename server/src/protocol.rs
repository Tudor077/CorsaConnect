//! Wire formats for the two UDP links.
//!
//! Phone -> server : `InputPacket` (9 bytes). Sent ~60Hz, drives the virtual pad.
//! Server -> phone : `TelemetryPacket` (compact, parsed from BeamNG OutGauge).
//!
//! Everything is little-endian. We keep the input packet tiny and fixed-size so
//! parsing is a few slice reads and latency stays minimal.
//!
//! v2: `buttons` is the raw 16-bit XInput mask chosen by the phone, so the
//! server just forwards it to the virtual pad and the phone owns the mapping.
//! v3: adds `clutch`, a third analog pedal mapped to the right-stick Y axis.
//! v4: telemetry carries `slip` and `impact`, derived from BeamNG MotionSim, so
//!     the wheel can buzz for real slides and crashes.
//! v5: telemetry carries learned `max_rpm` and `redline` so the tach auto-fits
//!     each car.

/// Magic prefix for phone -> server input packets.
pub const INPUT_MAGIC: &[u8; 2] = b"CC";
/// Magic prefix for server -> phone telemetry packets.
pub const TELEMETRY_MAGIC: &[u8; 2] = b"CT";
pub const PROTO_VERSION: u8 = 5;

/// Decoded controller input coming from the phone.
#[derive(Debug, Clone, Copy)]
pub struct InputPacket {
    /// Steering, full range. Maps directly to the left thumbstick X axis.
    pub steer: i16,
    /// Throttle 0..=255, maps to the right trigger.
    pub throttle: u8,
    /// Brake 0..=255, maps to the left trigger.
    pub brake: u8,
    /// Clutch 0..=255, maps to the right-stick Y axis.
    pub clutch: u8,
    /// Raw 16-bit XInput button mask, forwarded straight to the virtual pad.
    pub buttons: u16,
}

impl InputPacket {
    pub const LEN: usize = 10;

    /// Parse a 10-byte input packet. Returns `None` if the buffer is too short
    /// or the magic / version don't match (i.e. it's some other UDP traffic).
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < Self::LEN || &buf[0..2] != INPUT_MAGIC || buf[2] != PROTO_VERSION {
            return None;
        }
        Some(InputPacket {
            buttons: u16::from_le_bytes([buf[3], buf[4]]),
            steer: i16::from_le_bytes([buf[5], buf[6]]),
            throttle: buf[7],
            brake: buf[8],
            clutch: buf[9],
        })
    }
}

/// Telemetry we forward to the phone after parsing OutGauge.
#[derive(Debug, Clone, Copy, Default)]
pub struct TelemetryPacket {
    /// Gear as BeamNG reports it: 0 = reverse, 1 = neutral, 2 = 1st, ...
    pub gear: i8,
    pub speed_kmh: f32,
    pub rpm: f32,
    pub fuel: f32,   // 0..1
    pub turbo: f32,  // bar
    pub engine_temp: f32,
    pub throttle: f32, // 0..1
    pub brake: f32,    // 0..1
    pub slip: f32,     // 0..1 sideways slide, from MotionSim (0 if not enabled)
    pub impact: f32,   // 0..1 crash strength, decaying, from MotionSim
    pub max_rpm: f32,  // learned per car (peak rpm seen); 0 until learned
    pub redline: f32,  // learned per car (shift light, or fraction of max)
}

impl TelemetryPacket {
    /// Serialize into the wire format sent to the phone.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(32);
        b.extend_from_slice(TELEMETRY_MAGIC);
        b.push(PROTO_VERSION);
        b.push(self.gear as u8);
        b.extend_from_slice(&self.speed_kmh.to_le_bytes());
        b.extend_from_slice(&self.rpm.to_le_bytes());
        b.extend_from_slice(&self.fuel.to_le_bytes());
        b.extend_from_slice(&self.turbo.to_le_bytes());
        b.extend_from_slice(&self.engine_temp.to_le_bytes());
        b.extend_from_slice(&self.throttle.to_le_bytes());
        b.extend_from_slice(&self.brake.to_le_bytes());
        b.extend_from_slice(&self.slip.to_le_bytes());
        b.extend_from_slice(&self.impact.to_le_bytes());
        b.extend_from_slice(&self.max_rpm.to_le_bytes());
        b.extend_from_slice(&self.redline.to_le_bytes());
        b
    }
}
