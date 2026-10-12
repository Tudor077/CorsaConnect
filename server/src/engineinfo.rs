//! The car's real rpm range, from CorsaConnect's own BeamNG protocol.
//!
//! OutGauge carries the rpm but not the range, so without help the tach has
//! to infer the redline from how the engine behaves ([crate::server] does that
//! as a fallback). The mod in `beamng/` adds a protocol that sends the range
//! BeamNG's own tacho apps use: idle rpm and the rev limiter, a few times a
//! second, to UDP 4446. 12 bytes, little-endian, header "CE" + version 1.
//!
//! The protocol travels inside DotDash, the dashboard UI apps for BeamNG
//! (github.com/Tudor077/beamng-dotdash). CorsaConnect carries DotDash's
//! public zip (`beamng/dotdash.zip`, refreshed by `tools/update_dotdash.py`)
//! and installs it on request: one mod gives you both the apps on screen and
//! the rev range for the phone.

use std::path::{Path, PathBuf};

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

/// The protocol as it is in the repo. DotDash carries a copy of this file.
#[cfg(test)]
const MOD_LUA: &str = include_str!("../../beamng/lua/vehicle/protocols/corsaconnect.lua");

/// DotDash's public release zip, with the protocol inside.
const DOTDASH_ZIP: &[u8] = include_bytes!("../../beamng/dotdash.zip");
/// What we call it in the mods folder.
const DOTDASH_FILE: &str = "dotdash.zip";
/// The protocol-only mod an earlier 0.7.0 build installed; replaced by DotDash.
const LEGACY_DIR: &str = "corsaconnect";

/// The protocol's path inside any mod, as BeamNG mounts it.
const PROTOCOL_PATH: &str = "lua/vehicle/protocols/corsaconnect.lua";
/// A file every DotDash has, protocol or not.
const DOTDASH_MARK: &str = "ui/modules/apps/DotDash/app.json";

/// BeamNG's user mods folder (`%LOCALAPPDATA%\BeamNG\BeamNG.drive\current\mods`),
/// if BeamNG has ever run on this PC.
fn mods_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let dir = PathBuf::from(local).join("BeamNG").join("BeamNG.drive").join("current").join("mods");
    dir.is_dir().then_some(dir)
}

/// Where DotDash stands on this PC.
#[derive(Debug, Clone, PartialEq)]
pub enum ModState {
    /// No BeamNG user folder found.
    NoBeamNg,
    Missing,
    /// The copy we installed; `current` when it's the one this exe carries.
    Ours { current: bool },
    /// DotDash put there some other way (the Repository, or built from
    /// source). We leave it alone; without the protocol it's a version
    /// from before 1.1 and needs updating where it came from.
    Elsewhere { name: String, protocol: bool },
}

pub fn mod_state() -> ModState {
    match mods_dir() {
        None => ModState::NoBeamNg,
        Some(mods) => state_in(&mods),
    }
}

fn state_in(mods: &Path) -> ModState {
    if let Ok(bytes) = std::fs::read(mods.join(DOTDASH_FILE)) {
        return ModState::Ours { current: bytes == DOTDASH_ZIP };
    }
    if let Ok(unpacked) = std::fs::read_dir(mods.join("unpacked")) {
        for dir in unpacked.flatten().map(|e| e.path()) {
            if dir.join(DOTDASH_MARK).is_file() {
                return ModState::Elsewhere {
                    name: dir.file_name().unwrap().to_string_lossy().into_owned(),
                    protocol: dir.join(PROTOCOL_PATH).is_file(),
                };
            }
        }
    }
    for folder in [mods.to_path_buf(), mods.join("repo")] {
        let Ok(entries) = std::fs::read_dir(folder) else { continue };
        for zip in entries.flatten().map(|e| e.path()) {
            if !zip.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
                continue;
            }
            let names = zip_names(&zip);
            if contains(&names, DOTDASH_MARK) {
                return ModState::Elsewhere {
                    name: zip.file_stem().unwrap().to_string_lossy().into_owned(),
                    protocol: contains(&names, PROTOCOL_PATH),
                };
            }
        }
    }
    ModState::Missing
}

/// The tail of a zip, where its central directory lists every file name
/// uncompressed - enough to see what's inside without unzipping. Car mods run
/// to hundreds of MB and are never DotDash, so big zips aren't read at all.
fn zip_names(zip: &Path) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(zip) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > 20 << 20 {
        return Vec::new();
    }
    let tail = len.min(1 << 20);
    let mut buf = vec![0; tail as usize];
    if f.seek(SeekFrom::End(-(tail as i64))).is_err() || f.read_exact(&mut buf).is_err() {
        return Vec::new();
    }
    buf
}

fn contains(hay: &[u8], name: &str) -> bool {
    hay.windows(name.len()).any(|w| w == name.as_bytes())
}

/// Put DotDash in the mods folder (or update it). BeamNG loads it on its next
/// start. Also clears out the protocol-only mod an earlier build installed,
/// which would otherwise be loaded twice.
pub fn install_mod() -> Result<PathBuf, String> {
    let mods = mods_dir().ok_or("BeamNG's user folder wasn't found. Start BeamNG once first.")?;
    let file = mods.join(DOTDASH_FILE);
    std::fs::write(&file, DOTDASH_ZIP).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(mods.join("unpacked").join(LEGACY_DIR));
    Ok(file)
}

/// Delete the DotDash zip we installed, and only that.
pub fn remove_mod() -> Result<(), String> {
    let mods = mods_dir().ok_or("BeamNG's user folder wasn't found.")?;
    std::fs::remove_file(mods.join(DOTDASH_FILE)).map_err(|e| e.to_string())
}

/// The DotDash version this exe carries. The zip's own info.json is
/// compressed, so tools/update_dotdash.py writes it down next to the zip.
pub fn dotdash_version() -> &'static str {
    include_str!("../../beamng/dotdash.version").trim()
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

    fn scratch(tag: &str) -> PathBuf {
        let mods = std::env::temp_dir().join(format!("cc_mods_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&mods);
        std::fs::create_dir_all(mods.join("unpacked").join("ets2_link")).unwrap();
        std::fs::create_dir_all(mods.join("repo")).unwrap();
        mods
    }

    /// Ours is told apart from a DotDash that came from somewhere else.
    #[test]
    fn tells_where_dotdash_came_from() {
        let mods = scratch("where");
        assert_eq!(state_in(&mods), ModState::Missing);

        // From the Repository, 1.0: no protocol yet.
        let old = mods.join("repo").join("dotdash_repo.zip");
        std::fs::write(&old, format!("PK\x03\x04 ... PK\x01\x02 {DOTDASH_MARK}")).unwrap();
        assert_eq!(state_in(&mods), ModState::Elsewhere { name: "dotdash_repo".into(), protocol: false });
        // 1.1 has it.
        std::fs::write(&old, format!("PK\x03\x04 ... PK\x01\x02 {DOTDASH_MARK} {PROTOCOL_PATH}")).unwrap();
        assert_eq!(state_in(&mods), ModState::Elsewhere { name: "dotdash_repo".into(), protocol: true });
        std::fs::remove_file(&old).unwrap();

        // Built from source, unpacked.
        let dev = mods.join("unpacked").join("dotdash");
        std::fs::create_dir_all(dev.join("ui/modules/apps/DotDash")).unwrap();
        std::fs::write(dev.join(DOTDASH_MARK), "{}").unwrap();
        assert_eq!(state_in(&mods), ModState::Elsewhere { name: "dotdash".into(), protocol: false });
        std::fs::remove_dir_all(&dev).unwrap();

        // Ours, current and then stale.
        std::fs::write(mods.join(DOTDASH_FILE), DOTDASH_ZIP).unwrap();
        assert_eq!(state_in(&mods), ModState::Ours { current: true });
        std::fs::write(mods.join(DOTDASH_FILE), b"an older zip").unwrap();
        assert_eq!(state_in(&mods), ModState::Ours { current: false });
        std::fs::remove_dir_all(&mods).unwrap();
    }

    /// The zip we ship really is DotDash, with this exact protocol inside.
    #[test]
    fn the_shipped_zip_carries_the_protocol() {
        assert!(contains(DOTDASH_ZIP, DOTDASH_MARK), "beamng/dotdash.zip isn't DotDash");
        assert!(contains(DOTDASH_ZIP, PROTOCOL_PATH), "beamng/dotdash.zip has no protocol");
        assert_eq!(&DOTDASH_ZIP[..2], b"PK");
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
