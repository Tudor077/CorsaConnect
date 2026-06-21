//! Parser for the BeamNG / Live-For-Speed OutGauge UDP packet.
//!
//! BeamNG sends this on the OutGauge port (default 4444) when the protocol is
//! enabled in Options -> Other -> Protocols. The layout matches LFS OutGauge:
//! 92 bytes, optionally 96 if an extra trailing ID int is configured.
//! All fields little-endian.

use crate::protocol::TelemetryPacket;

/// Minimum size of an OutGauge packet (without the optional trailing ID).
const OUTGAUGE_LEN: usize = 92;

/// Parse an OutGauge packet into our compact telemetry. Returns `None` if the
/// buffer is too short to be an OutGauge packet.
pub fn parse(buf: &[u8]) -> Option<TelemetryPacket> {
    if buf.len() < OUTGAUGE_LEN {
        return None;
    }

    // Offsets per the OutGauge struct layout:
    //   0  u32   Time
    //   4  [u8;4] Car
    //   8  u16   Flags
    //  10  i8    Gear        (0 = reverse, 1 = neutral, 2 = 1st, ...)
    //  11  i8    SpareB / PLID
    //  12  f32   Speed       (m/s)
    //  16  f32   RPM
    //  20  f32   Turbo       (bar)
    //  24  f32   EngTemp     (C)
    //  28  f32   Fuel        (0..1)
    //  32  f32   OilPressure
    //  36  f32   OilTemp
    //  40  u32   DashLights
    //  44  u32   ShowLights
    //  48  f32   Throttle    (0..1)
    //  52  f32   Brake       (0..1)
    //  56  f32   Clutch      (0..1)
    //  60  [u8;16] Display1
    //  76  [u8;16] Display2
    let f32_at = |off: usize| f32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
    let text = |off: usize| {
        let mut d = [0u8; 16];
        d.copy_from_slice(&buf[off..off + 16]);
        d
    };

    let speed_ms = f32_at(12);
    Some(TelemetryPacket {
        gear: buf[10] as i8,
        speed_kmh: speed_ms * 3.6,
        rpm: f32_at(16),
        turbo: f32_at(20),
        engine_temp: f32_at(24),
        fuel: f32_at(28),
        throttle: f32_at(48),
        brake: f32_at(52),
        flags: u16::from_le_bytes([buf[8], buf[9]]),
        show_lights: u32::from_le_bytes(buf[44..48].try_into().unwrap()),
        display1: text(60),
        display2: text(76),
        // Filled in by the relay from MotionSim / learned RPM range.
        ..Default::default()
    })
}

