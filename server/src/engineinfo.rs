//! The car's real rpm range, from CorsaConnect's own BeamNG protocol.
//!
//! OutGauge carries the rpm but not the range, so without help the tach has
//! to infer the redline from how the engine behaves ([crate::server] does that
//! as a fallback). The mod in `beamng/` adds a protocol that sends the range
//! BeamNG's own tacho apps use: idle rpm and the rev limiter, a few times a
//! second, to UDP 4446. 12 bytes, little-endian, header "CE" + version 1.
//!
//! This file also installs that mod: it's one Lua file, so it's embedded in
//! the exe and written into BeamNG's unpacked mods folder on request.

use std::path::PathBuf;

/// Port the mod sends to (next to OutGauge 4444 and MotionSim 4445).
pub const ENGINEINFO_PORT: u16 = 4446;

/// One packet from the mod.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineInfo {
    pub idle_rpm: f32,
    pub max_rpm: f32,
    /// The engine cuts at `max_rpm`; without one it's just where power ends.
    pub rev_limiter: bool,
}

/// Parse a packet, or `None` if it isn't one of ours (or carries no range yet).
pub fn parse(buf: &[u8]) -> Option<EngineInfo> {
    if buf.len() < 12 || &buf[0..2] != b"CE" || buf[2] != 1 {
        return None;
    }
    let f = |off: usize| f32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
    let info = EngineInfo { idle_rpm: f(4), max_rpm: f(8), rev_limiter: buf[3] & 1 != 0 };
    // An electric car or a vehicle still spawning reports 0; that's no range.
    (info.max_rpm.is_finite() && info.max_rpm > 500.0).then_some(info)
}

/// The mod, exactly as it is in the repo.
const MOD_LUA: &str = include_str!("../../beamng/lua/vehicle/protocols/corsaconnect.lua");
const MOD_NAME: &str = "corsaconnect";

/// BeamNG's user mods folder (`%LOCALAPPDATA%\BeamNG\BeamNG.drive\current\mods`),
/// if BeamNG has ever run on this PC.
fn mods_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let dir = PathBuf::from(local).join("BeamNG").join("BeamNG.drive").join("current").join("mods");
    dir.is_dir().then_some(dir)
}

fn mod_file() -> Option<PathBuf> {
    Some(
        mods_dir()?
            .join("unpacked")
            .join(MOD_NAME)
            .join("lua")
            .join("vehicle")
            .join("protocols")
            .join("corsaconnect.lua"),
    )
}

/// Where the mod stands on this PC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModState {
    /// No BeamNG user folder found.
    NoBeamNg,
    Missing,
    /// Installed, but not the version this exe carries.
    Outdated,
    Installed,
}

pub fn mod_state() -> ModState {
    let Some(file) = mod_file() else { return ModState::NoBeamNg };
    match std::fs::read_to_string(&file) {
        Ok(s) if s.replace("\r\n", "\n") == MOD_LUA.replace("\r\n", "\n") => ModState::Installed,
        Ok(_) => ModState::Outdated,
        Err(_) => ModState::Missing,
    }
}

/// Write (or update) the mod. BeamNG picks it up on its next start.
pub fn install_mod() -> Result<PathBuf, String> {
    let file = mod_file().ok_or("BeamNG's user folder wasn't found. Start BeamNG once first.")?;
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&file, MOD_LUA).map_err(|e| e.to_string())?;
    Ok(file)
}

/// Delete the mod's folder, and only that.
pub fn remove_mod() -> Result<(), String> {
    let dir = mods_dir().ok_or("BeamNG's user folder wasn't found.")?.join("unpacked").join(MOD_NAME);
    std::fs::remove_dir_all(dir).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(flags: u8, idle: f32, max: f32) -> Vec<u8> {
        let mut b = vec![b'C', b'E', 1, flags];
        b.extend_from_slice(&idle.to_le_bytes());
        b.extend_from_slice(&max.to_le_bytes());
        b
    }

    #[test]
    fn reads_what_the_mod_sends() {
        let info = parse(&packet(1, 850.0, 7200.0)).unwrap();
        assert_eq!(info, EngineInfo { idle_rpm: 850.0, max_rpm: 7200.0, rev_limiter: true });
    }

    #[test]
    fn ignores_other_traffic_and_empty_ranges() {
        assert!(parse(b"BNG1 something else entirely").is_none());
        assert!(parse(&packet(0, 0.0, 0.0)).is_none());
        let mut v2 = packet(1, 850.0, 7200.0);
        v2[2] = 2;
        assert!(parse(&v2).is_none());
    }

    /// The struct in the Lua file must stay the 12 bytes parsed above.
    #[test]
    fn lua_struct_matches_the_parser() {
        for field in ["char          magic[2];", "unsigned char version;", "unsigned char flags;",
                      "float         idleRpm;", "float         maxRpm;"] {
            assert!(MOD_LUA.contains(field), "mod struct changed: {field}");
        }
        assert!(MOD_LUA.contains("local PORT = 4446"));
        assert!(MOD_LUA.contains("o.magic = \"CE\"") && MOD_LUA.contains("o.version = 1"));
    }
}
