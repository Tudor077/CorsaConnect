//! End-to-end check of the DLL the way a game uses it: create the FreeTrack
//! shared memory, write a pose, LoadLibrary the built NPClient64.dll, and
//! read the pose back through NP_GetData, verifying scaling + checksum.
//!
//! Run with `cargo test --release` (the cdylib must already be built, which
//! `cargo test` guarantees since it builds all targets first).

use std::ffi::c_void;

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
    fn MapViewOfFile(map: *mut c_void, access: u32, hi: u32, lo: u32, size: usize) -> *mut c_void;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[repr(C)]
#[derive(Default)]
struct TirData {
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

/// Same checksum the TIR5 games run over the received struct.
fn cksum(buf: &[u8]) -> u32 {
    let size = buf.len();
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
    c ^= c.wrapping_shl(3);
    c = c.wrapping_add(c >> 5);
    c ^= c.wrapping_shl(4);
    c = c.wrapping_add(c >> 17);
    c ^= c.wrapping_shl(25);
    c = c.wrapping_add(c >> 6);
    c as u32
}

#[test]
fn game_style_roundtrip() {
    unsafe {
        // Server side: create the mapping and write a pose.
        let map = CreateFileMappingA(
            usize::MAX as *mut c_void,
            std::ptr::null_mut(),
            0x04, // PAGE_READWRITE
            0,
            108,
            b"FT_SharedMem\0".as_ptr(),
        );
        assert!(!map.is_null());
        let view = MapViewOfFile(map, 0x0002, 0, 0, 108) as *mut u8;
        assert!(!view.is_null());
        std::ptr::write_bytes(view, 0, 108);

        let yaw_rad = 0.5f32;
        let pitch_rad = -0.25f32;
        let x_mm = 120.0f32;
        std::ptr::write_unaligned(view.add(12) as *mut f32, yaw_rad);
        std::ptr::write_unaligned(view.add(16) as *mut f32, pitch_rad);
        std::ptr::write_unaligned(view.add(24) as *mut f32, x_mm);

        // Game side: load the DLL from the build output.
        let dll = format!(
            "{}\\target\\release\\NPClient64.dll",
            env!("CARGO_MANIFEST_DIR")
        );
        let wide: Vec<u16> = dll.encode_utf16().chain(std::iter::once(0)).collect();
        let module = LoadLibraryW(wide.as_ptr());
        assert!(!module.is_null(), "could not load {dll}");

        let get_data: extern "system" fn(*mut TirData) -> i32 = std::mem::transmute(
            GetProcAddress(module, b"NP_GetData\0".as_ptr()),
        );
        let query_version: extern "system" fn(*mut u16) -> i32 = std::mem::transmute(
            GetProcAddress(module, b"NP_QueryVersion\0".as_ptr()),
        );
        let get_signature: extern "system" fn(*mut [u8; 400]) -> i32 = std::mem::transmute(
            GetProcAddress(module, b"NP_GetSignature\0".as_ptr()),
        );

        let mut version = 0u16;
        assert_eq!(query_version(&mut version), 0);
        assert_eq!(version, 0x0500);

        let mut sig = [0u8; 400];
        assert_eq!(get_signature(&mut sig), 0);
        let dll_sig: Vec<u8> = sig[..200].iter().copied().take_while(|&b| b != 0).collect();
        assert!(String::from_utf8_lossy(&dll_sig).contains("precise head tracking"));

        let mut data = TirData::default();
        let status = get_data(&mut data);
        assert_eq!(status, 0, "pose should be flowing (NPCLIENT_STATUS_OK)");

        // Verify the game-side checksum over the struct with cksum zeroed.
        let received = data.cksum;
        data.cksum = 0;
        let bytes =
            std::slice::from_raw_parts(&data as *const TirData as *const u8, 68);
        assert_eq!(received, cksum(bytes), "checksum must match what games compute");

        // Values scaled to TrackIR units.
        let expect_yaw = yaw_rad * 16383.0 / std::f32::consts::PI;
        let expect_pitch = pitch_rad * 16383.0 / std::f32::consts::PI;
        let expect_tx = x_mm * 16383.0 / 500.0;
        assert!((data.yaw - expect_yaw).abs() < 0.5, "yaw {} vs {}", data.yaw, expect_yaw);
        assert!((data.pitch - expect_pitch).abs() < 0.5);
        assert!((data.tx - expect_tx).abs() < 0.5);
        assert_eq!(data.padding, [0.0f32; 9]);
    }
}
