//! F1 24's UDP telemetry.
//!
//! The game has it built in: Settings > Telemetry Settings > UDP Telemetry On,
//! port 20777, UDP format 2024 (2023 has the same layout for what we read).
//! Every packet starts with a 29-byte header; of the many packet types we use
//! two, both arrays of 22 cars indexed by the header's `playerCarIndex`:
//!
//! - CarTelemetry (id 6): speed, rpm, gear, pedals, rev lights, temperatures.
//! - CarStatus (id 7): fuel, and the engine's exact max and idle rpm - so
//!   unlike BeamNG there's nothing to learn, the tach is right from the start.
//!
//! Layouts from EA's "F1 24 UDP Specification". Everything is little-endian
//! and packed, so offsets are plain arithmetic.

use crate::protocol::TelemetryPacket;

/// Where F1 24 sends by default.
pub const F1_PORT: u16 = 20777;

const HEADER_LEN: usize = 29;
const CARS: usize = 22;
const TELEMETRY_ID: u8 = 6;
const STATUS_ID: u8 = 7;
/// One car in CarTelemetry / CarStatus.
const TELEMETRY_CAR: usize = 60;
const STATUS_CAR: usize = 55;

/// The packet formats with the layout below (the game lets you pick).
const FORMATS: [u16; 2] = [2023, 2024];

/// What we take from CarTelemetry, for the player's car.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarTelemetry {
    pub speed_kmh: f32,
    pub throttle: f32,
    pub brake: f32,
    /// F1's numbering: -1 = R, 0 = N, 1.. = gears.
    pub gear: i8,
    pub rpm: f32,
    pub drs: bool,
    /// The steering wheel's shift lights, 0..100 %.
    pub rev_lights: u8,
    pub engine_temp: f32,
}

/// What we take from CarStatus, for the player's car.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarStatus {
    pub fuel_kg: f32,
    pub fuel_capacity_kg: f32,
    pub max_rpm: f32,
    pub idle_rpm: f32,
    pub pit_limiter: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Packet {
    Telemetry(CarTelemetry),
    Status(CarStatus),
}

/// What the header says about a packet we can't use, for the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Skip {
    /// Not F1 telemetry at all (too short).
    NotF1,
    /// A packet format whose layout we don't know (F1 25's 2025, say).
    Format(u16),
    /// A packet type we don't read.
    Other,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub fn parse(buf: &[u8]) -> Result<Packet, Skip> {
    if buf.len() < HEADER_LEN {
        return Err(Skip::NotF1);
    }
    let format = u16_at(buf, 0);
    if !FORMATS.contains(&format) {
        return Err(Skip::Format(format));
    }
    let id = buf[6];
    let player = buf[27] as usize;
    if player >= CARS {
        return Err(Skip::Other); // spectating: no car of our own
    }
    match id {
        TELEMETRY_ID if buf.len() >= HEADER_LEN + CARS * TELEMETRY_CAR => {
            let c = HEADER_LEN + player * TELEMETRY_CAR;
            // speed u16 | throttle f32 | steer f32 | brake f32 | clutch u8 | gear i8 |
            // rpm u16 | drs u8 | revLights% u8 | revLightsBits u16 | brakesTemp u16x4 |
            // tyreSurface u8x4 | tyreInner u8x4 | engineTemp u16 | ...
            Ok(Packet::Telemetry(CarTelemetry {
                speed_kmh: u16_at(buf, c) as f32,
                throttle: f32_at(buf, c + 2),
                brake: f32_at(buf, c + 10),
                gear: buf[c + 15] as i8,
                rpm: u16_at(buf, c + 16) as f32,
                drs: buf[c + 18] != 0,
                rev_lights: buf[c + 19],
                engine_temp: u16_at(buf, c + 38) as f32,
            }))
        }
        STATUS_ID if buf.len() >= HEADER_LEN + CARS * STATUS_CAR => {
            let c = HEADER_LEN + player * STATUS_CAR;
            // tc u8 | abs u8 | fuelMix u8 | brakeBias u8 | pitLimiter u8 |
            // fuelInTank f32 | fuelCapacity f32 | fuelRemainingLaps f32 |
            // maxRPM u16 | idleRPM u16 | ...
            Ok(Packet::Status(CarStatus {
                pit_limiter: buf[c + 4] != 0,
                fuel_kg: f32_at(buf, c + 5),
                fuel_capacity_kg: f32_at(buf, c + 9),
                max_rpm: u16_at(buf, c + 17) as f32,
                idle_rpm: u16_at(buf, c + 19) as f32,
            }))
        }
        _ => Err(Skip::Other),
    }
}

/// OutGauge-style flags the phone already understands: km/h and bar.
const OG_KM: u16 = 16384;
const OG_BAR: u16 = 32768;
/// OutGauge dash light bits the phone already draws.
const DL_SHIFT: u32 = 1 << 0;
const DL_PITSPEED: u32 = 1 << 3;

/// The phone's telemetry packet from F1's latest numbers.
pub fn to_phone(t: &CarTelemetry, s: Option<&CarStatus>) -> TelemetryPacket {
    let mut p = TelemetryPacket {
        // Ours: 0 = R, 1 = N, 2 = 1st.
        gear: (t.gear.clamp(-1, 8) + 1) as i8,
        speed_kmh: t.speed_kmh,
        rpm: t.rpm,
        engine_temp: t.engine_temp,
        throttle: t.throttle,
        brake: t.brake,
        flags: OG_KM | OG_BAR,
        ..Default::default()
    };
    // The wheel's shift lights fill up to the change point; all lit = shift.
    if t.rev_lights >= 100 {
        p.show_lights |= DL_SHIFT;
    }
    if t.drs {
        p.display2[..3].copy_from_slice(b"DRS");
    }
    if let Some(s) = s {
        if s.fuel_capacity_kg > 0.0 {
            p.fuel = (s.fuel_kg / s.fuel_capacity_kg).clamp(0.0, 1.0);
        }
        if s.max_rpm > 0.0 {
            // Same shape as BeamNG with a known limiter: red from the limiter,
            // and a little headroom past it on the gauge.
            p.redline = s.max_rpm;
            p.max_rpm = s.max_rpm * 1.08;
        }
        if s.pit_limiter {
            p.show_lights |= DL_PITSPEED;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header for `id`, format 2024, with the player in slot `player`.
    fn header(id: u8, player: u8) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_LEN];
        b[0..2].copy_from_slice(&2024u16.to_le_bytes());
        b[2] = 24; // game year
        b[6] = id;
        b[27] = player;
        b
    }

    /// Lay a car out field by field, the way the spec lists them, so the test
    /// checks our offsets against the spec rather than against themselves.
    fn telemetry_car(speed: u16, thr: f32, brk: f32, gear: i8, rpm: u16, drs: u8, lights: u8, temp: u16) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&speed.to_le_bytes());
        c.extend_from_slice(&thr.to_le_bytes());
        c.extend_from_slice(&0.25f32.to_le_bytes()); // steer
        c.extend_from_slice(&brk.to_le_bytes());
        c.push(0); // clutch
        c.push(gear as u8);
        c.extend_from_slice(&rpm.to_le_bytes());
        c.push(drs);
        c.push(lights);
        c.extend_from_slice(&0u16.to_le_bytes()); // rev lights bits
        c.extend_from_slice(&[0u8; 8]); // brakes temperature u16 x4
        c.extend_from_slice(&[0u8; 4]); // tyre surface temperature
        c.extend_from_slice(&[0u8; 4]); // tyre inner temperature
        c.extend_from_slice(&temp.to_le_bytes());
        c.extend_from_slice(&[0u8; 16]); // tyre pressures f32 x4
        c.extend_from_slice(&[0u8; 4]); // surface type
        assert_eq!(c.len(), TELEMETRY_CAR);
        c
    }

    fn status_car(fuel: f32, cap: f32, max: u16, idle: u16, pit: u8) -> Vec<u8> {
        let mut c = vec![0, 0, 0, 50, pit]; // tc, abs, fuel mix, brake bias, pit limiter
        c.extend_from_slice(&fuel.to_le_bytes());
        c.extend_from_slice(&cap.to_le_bytes());
        c.extend_from_slice(&12.0f32.to_le_bytes()); // fuel remaining laps
        c.extend_from_slice(&max.to_le_bytes());
        c.extend_from_slice(&idle.to_le_bytes());
        c.push(8); // max gears
        c.push(1); // drs allowed
        c.extend_from_slice(&0u16.to_le_bytes()); // drs activation distance
        c.extend_from_slice(&[16, 16, 3]); // tyre compound actual / visual, age
        c.push(0); // fia flags
        c.extend_from_slice(&[0u8; 8]); // engine power ICE, MGU-K
        c.extend_from_slice(&[0u8; 4]); // ers store
        c.push(0); // ers deploy mode
        c.extend_from_slice(&[0u8; 12]); // ers harvested K / H, deployed
        c.push(0); // network paused
        assert_eq!(c.len(), STATUS_CAR);
        c
    }

    /// The player's car is read, not car 0, from a full-size packet.
    #[test]
    fn reads_the_players_car_telemetry() {
        let mut b = header(TELEMETRY_ID, 3);
        for i in 0..CARS {
            b.extend(if i == 3 {
                telemetry_car(287, 1.0, 0.0, 7, 11850, 1, 100, 112)
            } else {
                telemetry_car(100, 0.5, 0.5, 3, 9000, 0, 10, 90)
            });
        }
        b.extend_from_slice(&[255, 255, 0]); // mfd panels, suggested gear
        assert_eq!(b.len(), 1352, "F1 24 CarTelemetry is 1352 bytes");
        let Ok(Packet::Telemetry(t)) = parse(&b) else { panic!("not parsed") };
        assert_eq!(
            t,
            CarTelemetry {
                speed_kmh: 287.0, throttle: 1.0, brake: 0.0, gear: 7, rpm: 11850.0,
                drs: true, rev_lights: 100, engine_temp: 112.0,
            }
        );
    }

    #[test]
    fn reads_the_players_car_status() {
        let mut b = header(STATUS_ID, 0);
        b.extend(status_car(42.5, 110.0, 13000, 4000, 1));
        for _ in 1..CARS {
            b.extend(status_car(1.0, 110.0, 1, 1, 0));
        }
        assert_eq!(b.len(), 1239, "F1 24 CarStatus is 1239 bytes");
        let Ok(Packet::Status(s)) = parse(&b) else { panic!("not parsed") };
        assert_eq!(
            s,
            CarStatus { fuel_kg: 42.5, fuel_capacity_kg: 110.0, max_rpm: 13000.0, idle_rpm: 4000.0, pit_limiter: true }
        );
    }

    #[test]
    fn skips_what_it_cannot_read() {
        assert_eq!(parse(b"short"), Err(Skip::NotF1));
        let mut f1_25 = header(TELEMETRY_ID, 0);
        f1_25[0..2].copy_from_slice(&2025u16.to_le_bytes());
        assert_eq!(parse(&f1_25), Err(Skip::Format(2025)));
        assert_eq!(parse(&header(0, 0)), Err(Skip::Other)); // motion
        assert_eq!(parse(&header(TELEMETRY_ID, 255)), Err(Skip::Other)); // spectating
    }

    #[test]
    fn maps_onto_the_phones_packet() {
        let t = CarTelemetry {
            speed_kmh: 120.0, throttle: 0.8, brake: 0.0, gear: -1, rpm: 6000.0,
            drs: false, rev_lights: 100, engine_temp: 105.0,
        };
        let s = CarStatus { fuel_kg: 55.0, fuel_capacity_kg: 110.0, max_rpm: 13000.0, idle_rpm: 4000.0, pit_limiter: true };
        let p = to_phone(&t, Some(&s));
        assert_eq!(p.gear, 0); // reverse
        assert_eq!(to_phone(&CarTelemetry { gear: 0, ..t }, None).gear, 1); // neutral
        assert_eq!(to_phone(&CarTelemetry { gear: 1, ..t }, None).gear, 2); // first
        assert_eq!(p.fuel, 0.5);
        assert_eq!((p.redline, p.max_rpm), (13000.0, 13000.0 * 1.08));
        assert_eq!(p.show_lights, DL_SHIFT | DL_PITSPEED);
        // Before the first CarStatus: no range yet, the phone keeps its default.
        assert_eq!(to_phone(&t, None).max_rpm, 0.0);
    }
}
