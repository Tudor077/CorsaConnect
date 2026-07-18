//! Dummy TrackIR.exe. Several TrackIR-enabled games refuse to start head
//! tracking unless a process named TrackIR.exe is running; this one just
//! exists. Launched by the CorsaConnect server with its PID as the only
//! argument so we can quit together with it.

#![windows_subsystem = "windows"]

use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
}

const SYNCHRONIZE: u32 = 0x0010_0000;
const INFINITE: u32 = 0xFFFF_FFFF;

fn main() {
    let parent = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<u32>().ok());
    match parent {
        Some(pid) => unsafe {
            let handle = OpenProcess(SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return;
            }
            // Returns when the server process exits.
            WaitForSingleObject(handle, INFINITE);
        },
        None => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
    }
}
