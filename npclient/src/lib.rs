// NPClient64.dll — TrackIR interface emulation for CorsaConnect.
//
// Games that support TrackIR look up the registry key
// HKCU\Software\NaturalPoint\NATURALPOINT\NPClient Location and load
// NPClient64.dll from there, then poll NP_GetData() every frame.
// This DLL reads the head pose from the standard FreeTrack shared memory
// ("FT_SharedMem", written by the CorsaConnect server) and serves it in
// TrackIR units with the checksum TIR5 games verify.
//
// Interface facts (struct layouts, checksum, signature) follow the publicly
// documented NPClient ABI used by FreeTrack/linuxtrack/opentrack.

#![allow(non_snake_case, clippy::missing_safety_doc)]

mod sig;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

const NP_AXIS_MAX: f64 = 16383.0;

const STATUS_OK: i32 = 0;
const STATUS_DISABLED: i32 = 1;

// ---------------------------------------------------------------- win32 ffi

type Handle = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileMappingA(
        file: Handle,
        attrs: *mut c_void,
        protect: u32,
        size_hi: u32,
        size_lo: u32,
        name: *const u8,
    ) -> Handle;
    fn MapViewOfFile(map: Handle, access: u32, off_hi: u32, off_lo: u32, size: usize)
        -> *mut c_void;
    fn CreateMutexA(attrs: *mut c_void, initial_owner: i32, name: *const u8) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
}

const PAGE_READWRITE: u32 = 0x04;
const FILE_MAP_WRITE: u32 = 0x0002;

// ------------------------------------------------------- shared memory data

/// FreeTrack 2.0 shared-memory layout ("FT_SharedMem").
#[repr(C)]
struct FTData {
    data_id: i32,
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
    points: [f32; 8], // x1,y1..x4,y4
}

#[repr(C)]
struct FTMemMap {
    data: FTData,
    game_id: i32,
    table: [u8; 8],
    game_id2: i32,
}

/// TrackIR data struct returned to the game. 68 bytes.
#[repr(C)]
pub struct TirData {
    status: i16,
    frame: i16,
    cksum: u32,
    roll: f32,
    pitch: f32,
    yaw: f32,
    tx: f32,
    ty: f32,
    tz: f32,
    padding: [f32; 9],
}

#[repr(C)]
pub struct TirSignature {
    dll_signature: [u8; 200],
    app_signature: [u8; 200],
}

// ----------------------------------------------------------------- statics

static MAPPING: AtomicPtr<FTMemMap> = AtomicPtr::new(std::ptr::null_mut());
static FRAME: AtomicI32 = AtomicI32::new(0);
static ENC_CHECKED: AtomicBool = AtomicBool::new(false);
static ENCRYPTION: AtomicBool = AtomicBool::new(false);
static TABLE: [std::sync::atomic::AtomicU8; 8] = [
    std::sync::atomic::AtomicU8::new(0),
    std::sync::atomic::AtomicU8::new(0),
    std::sync::atomic::AtomicU8::new(0),
    std::sync::atomic::AtomicU8::new(0),
    std::sync::atomic::AtomicU8::new(0),
    std::sync::atomic::AtomicU8::new(0),
    std::sync::atomic::AtomicU8::new(0),
    std::sync::atomic::AtomicU8::new(0),
];

unsafe fn mapping() -> *mut FTMemMap {
    let p = MAPPING.load(Ordering::Acquire);
    if !p.is_null() {
        return p;
    }
    // Same open-or-create dance as every FreeTrack client; if two threads
    // race, both map the same named section and one extra handle leaks.
    let mutex = CreateMutexA(std::ptr::null_mut(), 0, b"FT_Mutext\0".as_ptr());
    if !mutex.is_null() {
        CloseHandle(mutex);
    }
    let map = CreateFileMappingA(
        usize::MAX as Handle, // INVALID_HANDLE_VALUE => pagefile-backed
        std::ptr::null_mut(),
        PAGE_READWRITE,
        0,
        std::mem::size_of::<FTMemMap>() as u32,
        b"FT_SharedMem\0".as_ptr(),
    );
    if map.is_null() {
        return std::ptr::null_mut();
    }
    let view = MapViewOfFile(map, FILE_MAP_WRITE, 0, 0, std::mem::size_of::<FTMemMap>())
        as *mut FTMemMap;
    if view.is_null() {
        CloseHandle(map);
        return std::ptr::null_mut();
    }
    MAPPING.store(view, Ordering::Release);
    view
}

// ------------------------------------------------------- checksum / encrypt

/// TIR5 checksum over the 68-byte struct (cksum field zeroed first).
/// Mirrors the reference C exactly: 32-bit signed wrapping arithmetic.
fn cksum(buf: &[u8]) -> u32 {
    let size = buf.len();
    if size == 0 {
        return 0;
    }
    let mut c: i32 = size as i32;
    let mut i = 0usize;
    for _ in 0..(size >> 2) {
        let a0 = i16::from_le_bytes([buf[i], buf[i + 1]]) as i32;
        let mut a2 = i16::from_le_bytes([buf[i + 2], buf[i + 3]]) as i32;
        i += 4;
        c = c.wrapping_add(a0);
        a2 ^= c.wrapping_shl(5);
        a2 = a2.wrapping_shl(11);
        c ^= a2;
        c = c.wrapping_add(c >> 11);
    }
    let rem = size & 3;
    let tail: i32 = match rem {
        3 => {
            let a0 = i16::from_le_bytes([buf[i], buf[i + 1]]) as i32;
            let a2 = buf[i + 2] as i8 as i32;
            c = c.wrapping_add(a0);
            let a2 = a2.wrapping_shl(2) ^ c;
            c ^= a2.wrapping_shl(16);
            c >> 11
        }
        2 => {
            let a2 = i16::from_le_bytes([buf[i], buf[i + 1]]) as i32;
            c = c.wrapping_add(a2);
            c ^= c.wrapping_shl(11);
            c >> 17
        }
        1 => {
            let a2 = buf[i] as i8 as i32;
            c = c.wrapping_add(a2);
            c ^= c.wrapping_shl(10);
            c >> 1
        }
        _ => 0,
    };
    if rem != 0 {
        c = c.wrapping_add(tail);
    }
    c ^= c.wrapping_shl(3);
    c = c.wrapping_add(c >> 5);
    c ^= c.wrapping_shl(4);
    c = c.wrapping_add(c >> 17);
    c ^= c.wrapping_shl(25);
    c = c.wrapping_add(c >> 6);
    c as u32
}

/// Legacy per-game obfuscation; only runs when the server hands out a
/// non-zero table (no modern game does).
fn enhance(buf: &mut [u8], table: &[u8; 8]) {
    let mut table_ptr = 0usize;
    let mut var: u8 = 0x88;
    let mut size = buf.len();
    if size == 0 {
        return;
    }
    loop {
        size -= 1;
        let tmp = buf[size];
        buf[size] = tmp ^ table[table_ptr] ^ var;
        var = var.wrapping_add((size as u8).wrapping_add(tmp));
        table_ptr += 1;
        if table_ptr >= table.len() {
            table_ptr = 0;
        }
        if size == 0 {
            break;
        }
    }
}

// ----------------------------------------------------------------- exports

#[no_mangle]
pub unsafe extern "system" fn NP_GetData(data: *mut TirData) -> i32 {
    if data.is_null() {
        return STATUS_DISABLED;
    }
    let mut y = 0.0f64;
    let mut p = 0.0f64;
    let mut r = 0.0f64;
    let mut tx = 0.0f64;
    let mut ty = 0.0f64;
    let mut tz = 0.0f64;

    let mem = mapping();
    if !mem.is_null() {
        let d = std::ptr::addr_of!((*mem).data);
        y = std::ptr::read_volatile(std::ptr::addr_of!((*d).yaw)) as f64 * NP_AXIS_MAX
            / std::f64::consts::PI;
        p = std::ptr::read_volatile(std::ptr::addr_of!((*d).pitch)) as f64 * NP_AXIS_MAX
            / std::f64::consts::PI;
        r = std::ptr::read_volatile(std::ptr::addr_of!((*d).roll)) as f64 * NP_AXIS_MAX
            / std::f64::consts::PI;
        tx = std::ptr::read_volatile(std::ptr::addr_of!((*d).x)) as f64 * NP_AXIS_MAX / 500.0;
        ty = std::ptr::read_volatile(std::ptr::addr_of!((*d).y)) as f64 * NP_AXIS_MAX / 500.0;
        tz = std::ptr::read_volatile(std::ptr::addr_of!((*d).z)) as f64 * NP_AXIS_MAX / 500.0;

        let game_id = std::ptr::read_volatile(std::ptr::addr_of!((*mem).game_id));
        let game_id2 = std::ptr::read_volatile(std::ptr::addr_of!((*mem).game_id2));
        if game_id == game_id2 && !ENC_CHECKED.load(Ordering::Relaxed) {
            ENC_CHECKED.store(true, Ordering::Relaxed);
            let table = std::ptr::read_volatile(std::ptr::addr_of!((*mem).table));
            for (slot, b) in TABLE.iter().zip(table) {
                slot.store(b, Ordering::Relaxed);
                if b != 0 {
                    ENCRYPTION.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    let clamp = |v: f64| v.clamp(-NP_AXIS_MAX, NP_AXIS_MAX) as f32;
    let running =
        y != 0.0 || p != 0.0 || r != 0.0 || tx != 0.0 || ty != 0.0 || tz != 0.0;

    let out = &mut *data;
    out.frame = FRAME.fetch_add(1, Ordering::Relaxed).wrapping_add(1) as i16;
    out.status = if running { STATUS_OK as i16 } else { STATUS_DISABLED as i16 };
    out.cksum = 0;
    out.roll = clamp(r);
    out.pitch = clamp(p);
    out.yaw = clamp(y);
    out.tx = clamp(tx);
    out.ty = clamp(ty);
    out.tz = clamp(tz);
    out.padding = [0.0; 9];

    let bytes = std::slice::from_raw_parts_mut(
        data as *mut u8,
        std::mem::size_of::<TirData>(),
    );
    let sum = cksum(bytes);
    (*data).cksum = sum;

    if ENCRYPTION.load(Ordering::Relaxed) {
        let mut table = [0u8; 8];
        for (dst, slot) in table.iter_mut().zip(TABLE.iter()) {
            *dst = slot.load(Ordering::Relaxed);
        }
        enhance(bytes, &table);
    }

    if running {
        STATUS_OK
    } else {
        STATUS_DISABLED
    }
}

#[no_mangle]
pub unsafe extern "system" fn NP_GetSignature(sig: *mut TirSignature) -> i32 {
    if sig.is_null() {
        return 1;
    }
    (*sig).dll_signature = sig::SIG_DLL;
    (*sig).app_signature = sig::SIG_APP;
    0
}

#[no_mangle]
pub unsafe extern "system" fn NP_QueryVersion(version: *mut u16) -> i32 {
    if version.is_null() {
        return 1;
    }
    *version = 0x0500;
    0
}

#[no_mangle]
pub unsafe extern "system" fn NP_RegisterProgramProfileID(id: u16) -> i32 {
    let mem = mapping();
    if !mem.is_null() {
        std::ptr::write_volatile(std::ptr::addr_of_mut!((*mem).game_id), id as i32);
    }
    0
}

#[no_mangle]
pub extern "system" fn NP_ReCenter() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_RegisterWindowHandle(_hwnd: *mut c_void) -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_UnregisterWindowHandle() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_RequestData(_req: u16) -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_GetParameter(_a: i32, _b: i32) -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_SetParameter(_a: i32, _b: i32) -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_StartCursor() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_StopCursor() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_StartDataTransmission() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NP_StopDataTransmission() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NPPriv_ClientNotify() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NPPriv_GetLastError() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NPPriv_SetData() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NPPriv_SetLastError() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NPPriv_SetParameter() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NPPriv_SetSignature() -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn NPPriv_SetVersion() -> i32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes() {
        assert_eq!(std::mem::size_of::<TirData>(), 68);
        assert_eq!(std::mem::size_of::<FTMemMap>(), 108);
        assert_eq!(std::mem::size_of::<TirSignature>(), 400);
    }

    #[test]
    fn signature_text() {
        let dll = sig::SIG_DLL;
        let text: Vec<u8> = dll.iter().copied().take_while(|&b| b != 0).collect();
        assert!(String::from_utf8_lossy(&text).contains("precise head tracking"));
    }
}
