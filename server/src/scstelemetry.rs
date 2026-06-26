//! Reader for the SCS Telemetry shared memory used by Euro/American Truck Sim.
//!
//! Unlike BeamNG, the trucks have no built-in UDP telemetry. You install the
//! `scs-sdk-plugin` DLL (RenCloud) into the game's `bin/win_x64/plugins` folder,
//! and it publishes telemetry into a named memory-mapped file. We open that file
//! read-only and pull speed / rpm / gear out of it.
//!
//! The exact byte offsets of the fields aren't something we can assume, so we
//! also expose a diagnostic dump of several candidate offsets to calibrate
//! against the real game.

use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn OpenFileMappingW(dw_desired_access: u32, b_inherit_handle: i32, lp_name: *const u16) -> *mut c_void;
    fn MapViewOfFile(
        h_file_mapping_object: *mut c_void,
        dw_desired_access: u32,
        dw_file_offset_high: u32,
        dw_file_offset_low: u32,
        dw_number_of_bytes_to_map: usize,
    ) -> *mut c_void;
    fn UnmapViewOfFile(lp_base_address: *const c_void) -> i32;
    fn CloseHandle(h_object: *mut c_void) -> i32;
}

const FILE_MAP_READ: u32 = 0x0004;

// Offsets anchored on the confirmed speed/gear; the rest follow the documented
// truck channel field order.
const OFF_SPEED: usize = 948; // float, m/s
const OFF_RPM: usize = 952; // float
const OFF_GEAR: usize = 504; // int (1=1st, 0=N, -1=R)
const OFF_FUEL: usize = 1000; // float, litres
const OFF_FUEL_CAP: usize = 704; // float, tank capacity (config)
const OFF_WATER_TEMP: usize = 1024; // float, deg C
const OFF_BOOLS: usize = 1500; // start of the truck bool channels
const B_PARK_BRAKE: usize = 0;
const B_OIL_WARN: usize = 6;
const B_BATTERY_WARN: usize = 8;
const B_HIGH_BEAM: usize = 18;

/// A read-only view of the SCS telemetry shared memory. Keeping the handle open
/// keeps the mapping alive even if the game closes, so reads never fault.
pub struct ScsShared {
    handle: *mut c_void,
    view: *mut u8,
}

impl ScsShared {
    /// Open the mapping, or `None` if the plugin/game isn't running yet.
    pub fn open() -> Option<ScsShared> {
        for name in ["Local\\SCSTelemetry", "SCSTelemetry"] {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                let handle = OpenFileMappingW(FILE_MAP_READ, 0, wide.as_ptr());
                if handle.is_null() {
                    continue;
                }
                let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) as *mut u8;
                if view.is_null() {
                    CloseHandle(handle);
                    continue;
                }
                return Some(ScsShared { handle, view });
            }
        }
        None
    }

    fn f32_at(&self, off: usize) -> f32 {
        unsafe { (self.view.add(off) as *const f32).read_unaligned() }
    }

    fn i32_at(&self, off: usize) -> i32 {
        unsafe { (self.view.add(off) as *const i32).read_unaligned() }
    }

    pub fn speed_ms(&self) -> f32 {
        self.f32_at(OFF_SPEED)
    }

    pub fn rpm(&self) -> f32 {
        self.f32_at(OFF_RPM)
    }

    pub fn gear(&self) -> i32 {
        self.i32_at(OFF_GEAR)
    }

    fn bool_at(&self, off: usize) -> bool {
        unsafe { *self.view.add(off) != 0 }
    }

    /// Fuel level 0..1 (litres / tank capacity).
    pub fn fuel_frac(&self) -> f32 {
        let cap = self.f32_at(OFF_FUEL_CAP);
        if cap > 1.0 {
            (self.f32_at(OFF_FUEL) / cap).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Water (engine) temperature in deg C.
    pub fn water_temp(&self) -> f32 {
        self.f32_at(OFF_WATER_TEMP)
    }

    /// Warning lights mapped to our dash-light bitmask (HBRK/OIL/BATT/BEAM).
    pub fn show_lights(&self) -> u32 {
        let mut m = 0u32;
        if self.bool_at(OFF_BOOLS + B_PARK_BRAKE) {
            m |= 0x4; // HBRK
        }
        if self.bool_at(OFF_BOOLS + B_OIL_WARN) {
            m |= 0x100; // OIL
        }
        if self.bool_at(OFF_BOOLS + B_BATTERY_WARN) {
            m |= 0x200; // BATT
        }
        if self.bool_at(OFF_BOOLS + B_HIGH_BEAM) {
            m |= 0x2; // BEAM
        }
        m
    }

}

impl Drop for ScsShared {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view as *const c_void);
            CloseHandle(self.handle);
        }
    }
}
