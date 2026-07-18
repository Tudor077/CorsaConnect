//! FreeTrack / TrackIR output: publishes the head pose to games.
//!
//! Games that support TrackIR read the registry key
//! `HKCU\Software\NaturalPoint\NATURALPOINT\NPClient Location` and load
//! `NPClient64.dll` from the folder it points to. Our DLL (built from the
//! `npclient` crate and embedded in this exe) reads the pose back out of the
//! standard FreeTrack shared memory (`FT_SharedMem`) that this module writes.
//! Games that speak raw FreeTrack 2.0 read the same mapping directly.
//!
//! On start we extract `NPClient64.dll` + a dummy `TrackIR.exe` (some games
//! check for the process) into `%LOCALAPPDATA%\CorsaConnect` and point the
//! registry key there.

use std::ffi::c_void;
use std::path::PathBuf;
use std::process::{Child, Command};

static NPCLIENT64_DLL: &[u8] =
    include_bytes!("../../npclient/target/release/NPClient64.dll");
static TRACKIR_EXE: &[u8] = include_bytes!("../../npclient/target/release/TrackIR.exe");

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileMappingA(
        file: *mut c_void,
        attrs: *mut c_void,
        protect: u32,
        size_hi: u32,
        size_lo: u32,
        name: *const u8,
    ) -> *mut c_void;
    fn MapViewOfFile(map: *mut c_void, access: u32, off_hi: u32, off_lo: u32, size: usize)
        -> *mut c_void;
    fn UnmapViewOfFile(base: *const c_void) -> i32;
    fn CreateMutexA(attrs: *mut c_void, initial_owner: i32, name: *const u8) -> *mut c_void;
    fn CloseHandle(h: *mut c_void) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(
        hkey: usize,
        sub_key: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        sam: u32,
        security: *mut c_void,
        result: *mut usize,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        hkey: usize,
        value_name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> i32;
    fn RegCloseKey(hkey: usize) -> i32;
}

const PAGE_READWRITE: u32 = 0x04;
const FILE_MAP_WRITE: u32 = 0x0002;
const HKEY_CURRENT_USER: usize = 0x8000_0001;
const KEY_WRITE: u32 = 0x0002_0006;
const REG_SZ: u32 = 1;

/// FreeTrack 2.0 shared memory layout; must match the `npclient` crate.
#[repr(C)]
struct FTData {
    data_id: u32,
    cam_width: i32,
    cam_height: i32,
    yaw: f32,   // radians
    pitch: f32, // radians
    roll: f32,  // radians
    x: f32,     // mm
    y: f32,
    z: f32,
    raw_yaw: f32,
    raw_pitch: f32,
    raw_roll: f32,
    raw_x: f32,
    raw_y: f32,
    raw_z: f32,
    points: [f32; 8],
}

#[repr(C)]
struct FTMemMap {
    data: FTData,
    game_id: i32,
    table: [u8; 8],
    game_id2: i32,
}

/// Head pose in tracker conventions: degrees, positive yaw = looking left,
/// positive pitch = looking up; translation in cm, x right, y up, z towards
/// the screen.
#[derive(Clone, Copy, Default)]
pub struct HeadPose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub struct FreetrackWriter {
    map: *mut c_void,
    mutex: *mut c_void,
    view: *mut FTMemMap,
    seen_game_id: i32,
    dummy: Option<Child>,
}

// Raw pointers to a named shared mapping; safe to move across threads.
unsafe impl Send for FreetrackWriter {}

impl FreetrackWriter {
    /// Create the mapping, install the DLL + registry key, start the dummy
    /// TrackIR.exe. `log` receives human-readable progress lines.
    pub fn new(mut log: impl FnMut(String)) -> Result<FreetrackWriter, String> {
        let dir = install_client_files(&mut log)?;

        unsafe {
            let mutex = CreateMutexA(std::ptr::null_mut(), 0, b"FT_Mutext\0".as_ptr());
            let map = CreateFileMappingA(
                usize::MAX as *mut c_void,
                std::ptr::null_mut(),
                PAGE_READWRITE,
                0,
                std::mem::size_of::<FTMemMap>() as u32,
                b"FT_SharedMem\0".as_ptr(),
            );
            if map.is_null() {
                return Err("Could not create FreeTrack shared memory.".into());
            }
            let view =
                MapViewOfFile(map, FILE_MAP_WRITE, 0, 0, std::mem::size_of::<FTMemMap>())
                    as *mut FTMemMap;
            if view.is_null() {
                CloseHandle(map);
                return Err("Could not map FreeTrack shared memory.".into());
            }

            std::ptr::write_volatile(std::ptr::addr_of_mut!((*view).data.data_id), 1);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*view).data.cam_width), 100);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*view).data.cam_height), 250);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*view).game_id2), 0);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*view).table), [0u8; 8]);

            let dummy = Command::new(dir.join("TrackIR.exe"))
                .arg(std::process::id().to_string())
                .spawn()
                .ok();
            if dummy.is_none() {
                log("Could not start the dummy TrackIR.exe (some games need it).".into());
            }

            Ok(FreetrackWriter {
                map,
                mutex,
                view,
                seen_game_id: 0,
                dummy,
            })
        }
    }

    /// Publish one pose sample. Returns the id of the game currently reading
    /// (0 when none has registered yet).
    pub fn write(&mut self, pose: HeadPose) -> i32 {
        const D2R: f32 = std::f32::consts::PI / 180.0;
        unsafe {
            let d = std::ptr::addr_of_mut!((*self.view).data);
            // FreeTrack sign convention matches ours for yaw/pitch (positive =
            // left/up); translation is cm -> mm.
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).yaw), pose.yaw * D2R);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).pitch), pose.pitch * D2R);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).roll), pose.roll * D2R);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).x), pose.x * 10.0);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).y), pose.y * 10.0);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).z), pose.z * 10.0);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).raw_yaw), pose.yaw * D2R);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).raw_pitch), pose.pitch * D2R);
            std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).raw_roll), pose.roll * D2R);

            let game_id = std::ptr::read_volatile(std::ptr::addr_of!((*self.view).game_id));
            if game_id != self.seen_game_id {
                // A game registered: complete the handshake. Modern titles all
                // use a zero table (no data obfuscation).
                std::ptr::write_volatile(std::ptr::addr_of_mut!((*self.view).table), [0u8; 8]);
                std::ptr::write_volatile(
                    std::ptr::addr_of_mut!((*self.view).game_id2),
                    game_id,
                );
                std::ptr::write_volatile(std::ptr::addr_of_mut!((*d).data_id), 0);
                self.seen_game_id = game_id;
            } else {
                let id = std::ptr::read_volatile(std::ptr::addr_of!((*d).data_id));
                std::ptr::write_volatile(
                    std::ptr::addr_of_mut!((*d).data_id),
                    id.wrapping_add(1),
                );
            }
            game_id
        }
    }
}

impl Drop for FreetrackWriter {
    fn drop(&mut self) {
        // Zero the pose so the game sees the tracker as gone, not frozen.
        self.write(HeadPose::default());
        unsafe {
            UnmapViewOfFile(self.view as *const c_void);
            if !self.map.is_null() {
                CloseHandle(self.map);
            }
            if !self.mutex.is_null() {
                CloseHandle(self.mutex);
            }
        }
        if let Some(mut child) = self.dummy.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Extract NPClient64.dll + TrackIR.exe into %LOCALAPPDATA%\CorsaConnect and
/// point the TrackIR registry key there. Returns the install dir.
fn install_client_files(log: &mut impl FnMut(String)) -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("LOCALAPPDATA is not set")?;
    let dir = base.join("CorsaConnect");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {dir:?}: {e}"))?;

    for (name, bytes) in [("NPClient64.dll", NPCLIENT64_DLL), ("TrackIR.exe", TRACKIR_EXE)] {
        let path = dir.join(name);
        // Only rewrite when the payload changed, so a running game keeping the
        // DLL open doesn't fail the whole start.
        let stale = std::fs::read(&path).map(|cur| cur != bytes).unwrap_or(true);
        if stale {
            std::fs::write(&path, bytes)
                .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        }
    }

    // Games expect forward slashes and a trailing slash here.
    let mut location = dir.to_string_lossy().replace('\\', "/");
    if !location.ends_with('/') {
        location.push('/');
    }
    set_npclient_location(&location)?;
    log(format!("TrackIR client installed ({location})"));
    Ok(dir)
}

fn set_npclient_location(location: &str) -> Result<(), String> {
    let key_path: Vec<u16> = "Software\\NaturalPoint\\NATURALPOINT\\NPClient Location"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value_name: Vec<u16> = "Path".encode_utf16().chain(std::iter::once(0)).collect();
    let data: Vec<u16> = location.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut hkey: usize = 0;
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_WRITE,
            std::ptr::null_mut(),
            &mut hkey,
            std::ptr::null_mut(),
        );
        if rc != 0 {
            return Err(format!("Could not open the NPClient registry key (code {rc})."));
        }
        let rc = RegSetValueExW(
            hkey,
            value_name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        );
        RegCloseKey(hkey);
        if rc != 0 {
            return Err(format!("Could not set the NPClient registry path (code {rc})."));
        }
    }
    Ok(())
}
