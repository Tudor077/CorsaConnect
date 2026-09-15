//! Virtual racing wheel output through vJoy.
//!
//! ViGEmBus can only emulate an Xbox 360 pad or a DualShock 4, and an Xbox pad
//! is exactly what we *don't* want: XInput games index pads by slot, so a
//! second virtual Xbox pad fights the real one and every binding has to be
//! redone depending on which is plugged in. vJoy instead exposes a plain
//! DirectInput HID joystick with our own axis layout - steering on a 15-bit
//! axis, one axis per pedal - so the game sees a wheel that has nothing to do
//! with XInput, and a real Xbox pad can stay bound at the same time.
//!
//! `vJoyInterface.dll` ships with the driver, so we can't link it at build
//! time; it's loaded at runtime from the vJoy install directory (or anywhere on
//! the PATH). Everything here degrades to a readable error string when vJoy
//! isn't installed - the launcher shows it and offers the Xbox mode instead.
//!
//! Driver: https://github.com/njz3/vJoy/releases (or the original
//! sourceforge.net/projects/vjoystick).

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::sync::Mutex;

use crate::protocol::InputPacket;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        hkey: usize,
        sub_key: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
}

const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const RRF_RT_REG_SZ: u32 = 0x0000_0002;

/// vJoy device id we drive. Device 1 is the one a stock install creates.
pub const DEVICE_ID: u32 = 1;

/// HID usage ids vJoy uses to name axes.
const AXIS_X: u32 = 0x30;
const AXIS_Y: u32 = 0x31;
const AXIS_Z: u32 = 0x32;
const AXIS_RX: u32 = 0x33;
const AXIS_RY: u32 = 0x34;
const AXIS_RZ: u32 = 0x35;
const AXIS_SL0: u32 = 0x36;
const AXIS_SL1: u32 = 0x37;

/// `GetVJDStatus` results.
const VJD_STAT_OWN: i32 = 0; // already ours (a previous run in this process)
const VJD_STAT_FREE: i32 = 1;
const VJD_STAT_BUSY: i32 = 2; // owned by another feeder app
const VJD_STAT_MISS: i32 = 3; // device not configured in vJoyConf

/// Buttons we drive, in vJoy button order: index 0 is vJoy button 1. Each entry
/// is the XInput bit the phone sends for it and a label for the log, so the
/// user can see what to bind in-game.
pub const BUTTON_MAP: [(u16, &str); 14] = [
    (0x1000, "A"),
    (0x2000, "B"),
    (0x4000, "X"),
    (0x8000, "Y"),
    (0x0100, "LB"),
    (0x0200, "RB"),
    (0x0020, "Back"),
    (0x0010, "Start"),
    (0x0040, "L-stick"),
    (0x0080, "R-stick"),
    (0x0001, "D-Up"),
    (0x0002, "D-Down"),
    (0x0004, "D-Left"),
    (0x0008, "D-Right"),
];

/// `JOYSTICK_POSITION_V2` from vJoy's `public.h`. The layout must match
/// exactly; `bDevice` is a single byte followed by 4-byte-aligned LONGs, which
/// is what `repr(C)` gives us.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Position {
    device: u8,
    throttle: i32,
    rudder: i32,
    aileron: i32,
    axis_x: i32,
    axis_y: i32,
    axis_z: i32,
    axis_x_rot: i32,
    axis_y_rot: i32,
    axis_z_rot: i32,
    slider: i32,
    dial: i32,
    wheel: i32,
    axis_vx: i32,
    axis_vy: i32,
    axis_vz: i32,
    axis_vbrx: i32,
    axis_vbry: i32,
    axis_vbrz: i32,
    buttons: i32,
    hats: u32,
    hats_ex1: u32,
    hats_ex2: u32,
    hats_ex3: u32,
    buttons_ex1: i32,
    buttons_ex2: i32,
    buttons_ex3: i32,
}

impl Position {
    /// Write `v` into the field vJoy identifies by HID usage `axis`.
    fn set(&mut self, axis: u32, v: i32) {
        match axis {
            AXIS_X => self.axis_x = v,
            AXIS_Y => self.axis_y = v,
            AXIS_Z => self.axis_z = v,
            AXIS_RX => self.axis_x_rot = v,
            AXIS_RY => self.axis_y_rot = v,
            AXIS_RZ => self.axis_z_rot = v,
            AXIS_SL0 => self.slider = v,
            AXIS_SL1 => self.dial = v,
            _ => {}
        }
    }
}

/// The handful of `vJoyInterface.dll` entry points we need. All of them are
/// `__cdecl`, which on x86-64 Windows is the one and only calling convention.
#[derive(Clone, Copy)]
struct Api {
    enabled: unsafe extern "C" fn() -> i32,
    version: unsafe extern "C" fn() -> i16,
    status: unsafe extern "C" fn(u32) -> i32,
    acquire: unsafe extern "C" fn(u32) -> i32,
    relinquish: unsafe extern "C" fn(u32),
    reset: unsafe extern "C" fn(u32) -> i32,
    update: unsafe extern "C" fn(u32, *mut c_void) -> i32,
    axis_exists: unsafe extern "C" fn(u32, u32) -> i32,
    axis_max: unsafe extern "C" fn(u32, u32, *mut i32) -> i32,
    button_count: unsafe extern "C" fn(u32) -> i32,
}

/// Cached once loaded; the DLL then stays mapped for the program's life. A
/// failure is *not* cached, so the launcher's "Re-check" button picks vJoy up
/// right after it's installed, without a restart.
static API: Mutex<Option<Api>> = Mutex::new(None);

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Read a REG_SZ value, 64-bit view. Used to find where vJoy was installed.
fn reg_string(sub_key: &str, value: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            wide(sub_key).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut u8,
            &mut len,
        )
    };
    if rc != 0 {
        return None;
    }
    let chars = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..chars]))
}

/// Candidate paths for `vJoyInterface.dll`, best first. The bare name lets
/// `LoadLibraryW` search the PATH and our own folder, which covers portable
/// installs and anyone who dropped the DLL next to the exe.
fn dll_candidates() -> Vec<String> {
    let mut out = vec!["vJoyInterface.dll".to_string()];
    // Where the installer put it, if we can find its uninstall entry. Inno
    // Setup writes a 32-bit key, so look in both views.
    const UNINSTALL: [&str; 4] = [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\vJoy",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\vJoy",
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{8E31F76F-74C3-47F1-9550-E041EEDC5FBB}_is1",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{8E31F76F-74C3-47F1-9550-E041EEDC5FBB}_is1",
    ];
    for key in UNINSTALL {
        if let Some(dir) = reg_string(key, "InstallLocation") {
            let dir = dir.trim_end_matches('\\');
            out.push(format!("{dir}\\x64\\vJoyInterface.dll"));
        }
    }
    for var in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(dir) = std::env::var(var) {
            out.push(format!("{dir}\\vJoy\\x64\\vJoyInterface.dll"));
        }
    }
    out
}

fn load() -> Result<Api, String> {
    let mut module = std::ptr::null_mut();
    for path in dll_candidates() {
        let m = unsafe { LoadLibraryW(wide(&path).as_ptr()) };
        if !m.is_null() {
            module = m;
            break;
        }
    }
    if module.is_null() {
        return Err("vJoyInterface.dll not found - install the vJoy driver.".into());
    }

    // Every symbol is required; a partial load means a wrong/ancient DLL.
    macro_rules! sym {
        ($name:literal) => {{
            let p = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) };
            if p.is_null() {
                return Err(format!("vJoyInterface.dll has no {} - is it an old version?", $name));
            }
            unsafe { std::mem::transmute(p) }
        }};
    }
    let api = Api {
        enabled: sym!("vJoyEnabled"),
        version: sym!("GetvJoyVersion"),
        status: sym!("GetVJDStatus"),
        acquire: sym!("AcquireVJD"),
        relinquish: sym!("RelinquishVJD"),
        reset: sym!("ResetVJD"),
        update: sym!("UpdateVJD"),
        axis_exists: sym!("GetVJDAxisExist"),
        axis_max: sym!("GetVJDAxisMax"),
        button_count: sym!("GetVJDButtonNumber"),
    };
    if unsafe { (api.enabled)() } == 0 {
        return Err("vJoy is installed but the driver isn't enabled (reboot, or re-run vJoyConf).".into());
    }
    Ok(api)
}

fn api() -> Result<Api, String> {
    let mut slot = API.lock().unwrap();
    if let Some(api) = *slot {
        return Ok(api);
    }
    let api = load()?;
    *slot = Some(api);
    Ok(api)
}

/// What the launcher shows about vJoy before anything is launched.
pub struct Probe {
    pub version: String,
    /// How the device is set up, in one line.
    pub summary: String,
    /// Anything that needs a trip to vJoyConf (missing axes, too few buttons).
    pub warnings: Vec<String>,
}

/// Whether vJoy is usable right now, and how device [DEVICE_ID] is configured.
/// Reading the configuration doesn't need the device acquired, so this is safe
/// to call while the server is running - or while another feeder owns it.
pub fn probe() -> Result<Probe, String> {
    let api = api()?;
    let v = unsafe { (api.version)() };
    let layout = Layout::read(&api, DEVICE_ID);
    Ok(Probe {
        version: format!("{:x}.{:x}", (v >> 8) & 0xff, v & 0xff),
        summary: layout.summary(DEVICE_ID),
        warnings: layout.warnings(DEVICE_ID),
    })
}

/// Where each pedal and the free stick end up, resolved against the axes the
/// device actually has. First existing *unclaimed* axis in each list wins; a
/// control with no axis left is simply not sent (and the launcher says so).
/// Claiming runs in this order, so the pedals always win over the stick.
const THROTTLE_AXES: [(u32, &str); 3] = [(AXIS_Y, "Y"), (AXIS_Z, "Z"), (AXIS_RY, "RY")];
const BRAKE_AXES: [(u32, &str); 3] = [(AXIS_RZ, "RZ"), (AXIS_Z, "Z"), (AXIS_RX, "RX")];
const CLUTCH_AXES: [(u32, &str); 3] = [(AXIS_SL0, "Slider"), (AXIS_RX, "RX"), (AXIS_SL1, "Dial")];
const STICK_X_AXES: [(u32, &str); 3] = [(AXIS_RX, "RX"), (AXIS_Z, "Z"), (AXIS_SL1, "Dial")];
const STICK_Y_AXES: [(u32, &str); 3] = [(AXIS_RY, "RY"), (AXIS_SL1, "Dial"), (AXIS_Z, "Z")];

/// One resolved axis: HID usage, its full-scale value, and a display name.
#[derive(Clone, Copy)]
struct Axis {
    usage: u32,
    max: i32,
    name: &'static str,
}

/// How a vJoy device's configuration maps onto what we want to send.
struct Layout {
    steer: Option<Axis>,
    throttle: Option<Axis>,
    brake: Option<Axis>,
    clutch: Option<Axis>,
    stick_x: Option<Axis>,
    stick_y: Option<Axis>,
    buttons: usize,
}

impl Layout {
    /// Ask the driver which axes and buttons device `id` actually has, and
    /// hand each control the first axis on its list nothing else claimed.
    fn read(api: &Api, id: u32) -> Layout {
        let axis = |usage: u32, name: &'static str| -> Option<Axis> {
            if unsafe { (api.axis_exists)(id, usage) } == 0 {
                return None;
            }
            let mut max = 0i32;
            let max = if unsafe { (api.axis_max)(id, usage, &mut max) } != 0 && max > 0 {
                max
            } else {
                32767
            };
            Some(Axis { usage, max, name })
        };
        // Each control takes an axis out of the pool, so a device short on axes
        // drops the last controls instead of doubling two onto one axis.
        let steer = axis(AXIS_X, "X");
        let mut taken: Vec<u32> = steer.iter().map(|a| a.usage).collect();
        let mut claim = |list: &[(u32, &'static str); 3]| -> Option<Axis> {
            let found = list
                .iter()
                .filter(|(u, _)| !taken.contains(u))
                .find_map(|(u, n)| axis(*u, n));
            if let Some(a) = found {
                taken.push(a.usage);
            }
            found
        };

        let throttle = claim(&THROTTLE_AXES);
        let brake = claim(&BRAKE_AXES);
        let clutch = claim(&CLUTCH_AXES);
        let stick_x = claim(&STICK_X_AXES);
        let stick_y = claim(&STICK_Y_AXES);
        Layout {
            steer,
            throttle,
            brake,
            clutch,
            stick_x,
            stick_y,
            buttons: unsafe { (api.button_count)(id) }.max(0) as usize,
        }
    }

    /// One line describing what the game will see.
    fn summary(&self, id: u32) -> String {
        let steer = match self.steer {
            Some(a) => format!("steering = {} axis ({} steps)", a.name, a.max + 1),
            None => "no steering axis".to_string(),
        };
        let pedals = [
            ("throttle", self.throttle),
            ("brake", self.brake),
            ("clutch", self.clutch),
        ]
        .iter()
        .filter_map(|(label, a)| a.map(|a| format!("{label} = {}", a.name)))
        .collect::<Vec<_>>()
        .join(", ");
        let stick = match (self.stick_x, self.stick_y) {
            (Some(x), Some(y)) => format!(", stick = {}/{}", x.name, y.name),
            _ => String::new(),
        };
        format!(
            "vJoy device {id}: {steer}, {pedals}{stick}, {} buttons.",
            self.buttons
        )
    }

    /// Everything about this configuration that needs fixing in vJoyConf.
    fn warnings(&self, id: u32) -> Vec<String> {
        let mut out = Vec::new();
        if self.steer.is_none() {
            out.push(format!(
                "vJoy device {id} has no X axis - enable X in vJoyConf, it's the steering axis."
            ));
        }
        for (label, pedal) in [
            ("Throttle", &self.throttle),
            ("Brake", &self.brake),
            ("Clutch", &self.clutch),
        ] {
            if pedal.is_none() {
                out.push(format!(
                    "{label} has no free axis on vJoy device {id} - add axes in vJoyConf."
                ));
            }
        }
        // The stick is optional - it only does anything if the phone's HUD has
        // one - so this is worded as a heads-up, not a broken configuration.
        if self.stick_x.is_none() || self.stick_y.is_none() {
            out.push(format!(
                "The free stick needs two spare axes on vJoy device {id} - enable RX and RY in vJoyConf."
            ));
        }
        if self.buttons < BUTTON_MAP.len() {
            out.push(format!(
                "vJoy device {id} has {} buttons, so {} are dropped - set it to {} in vJoyConf.",
                self.buttons,
                BUTTON_MAP
                    .iter()
                    .skip(self.buttons)
                    .map(|(_, n)| *n)
                    .collect::<Vec<_>>()
                    .join("/"),
                BUTTON_MAP.len(),
            ));
        }
        out
    }
}

/// An acquired vJoy device, fed from phone input packets.
pub struct Wheel {
    api: Api,
    id: u32,
    steer: Axis,
    throttle: Option<Axis>,
    brake: Option<Axis>,
    clutch: Option<Axis>,
    stick_x: Option<Axis>,
    stick_y: Option<Axis>,
    buttons: usize,
}

impl Wheel {
    /// Acquire vJoy device `id`. On success returns the wheel plus lines
    /// describing the resulting axis/button mapping for the log.
    pub fn open(id: u32) -> Result<(Wheel, Vec<String>), String> {
        let api = api()?;
        match unsafe { (api.status)(id) } {
            VJD_STAT_FREE | VJD_STAT_OWN => {}
            VJD_STAT_BUSY => {
                return Err(format!(
                    "vJoy device {id} is already used by another program - close it and Launch again."
                ))
            }
            VJD_STAT_MISS => {
                return Err(format!(
                    "vJoy device {id} isn't configured - open vJoyConf, enable device {id}, apply."
                ))
            }
            _ => return Err(format!("vJoy device {id} is in an unknown state.")),
        }
        let layout = Layout::read(&api, id);
        let Some(steer) = layout.steer else {
            return Err(format!(
                "vJoy device {id} has no X axis - enable X in vJoyConf, it's the steering axis."
            ));
        };
        if unsafe { (api.acquire)(id) } == 0 {
            return Err(format!("Could not acquire vJoy device {id}."));
        }
        unsafe { (api.reset)(id) };

        // Tell the user what to bind in-game, then what still needs fixing.
        let mut notes = vec![
            layout.summary(id),
            format!(
                "Buttons: {}.",
                BUTTON_MAP
                    .iter()
                    .take(layout.buttons.min(BUTTON_MAP.len()))
                    .enumerate()
                    .map(|(i, (_, name))| format!("{}={name}", i + 1))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ];
        notes.extend(layout.warnings(id));

        Ok((
            Wheel {
                api,
                id,
                steer,
                throttle: layout.throttle,
                brake: layout.brake,
                clutch: layout.clutch,
                stick_x: layout.stick_x,
                stick_y: layout.stick_y,
                buttons: layout.buttons,
            },
            notes,
        ))
    }

    /// Push one input packet to the device.
    pub fn update(&mut self, input: &InputPacket) {
        let mut pos = Position {
            device: self.id as u8,
            ..Default::default()
        };
        // Steering spans the axis; i16 center (0) lands mid-travel.
        pos.set(
            self.steer.usage,
            (input.steer as i32 + 32768) * self.steer.max / 65535,
        );
        // Same centered mapping for the free stick. Y is inverted on the way
        // out because DirectInput axes count downwards, while the phone sends
        // "up is positive" like a thumbstick.
        for (axis, value) in [
            (self.stick_x, input.joy_x),
            (self.stick_y, input.joy_y.saturating_neg()),
        ] {
            if let Some(a) = axis {
                pos.set(a.usage, (value as i32 + 32768) * a.max / 65535);
            }
        }
        for (pedal, value) in [
            (self.throttle, input.throttle),
            (self.brake, input.brake),
            (self.clutch, input.clutch),
        ] {
            if let Some(a) = pedal {
                pos.set(a.usage, value as i32 * a.max / 255);
            }
        }
        let mut mask = 0u32;
        for (i, (bit, _)) in BUTTON_MAP.iter().enumerate().take(self.buttons) {
            if input.buttons & bit != 0 {
                mask |= 1 << i;
            }
        }
        pos.buttons = mask as i32;
        unsafe { (self.api.update)(self.id, &mut pos as *mut Position as *mut c_void) };
    }

    /// Wheel centered, pedals up, no buttons - used when the phone goes quiet.
    pub fn center(&mut self) {
        self.update(&InputPacket {
            steer: 0,
            throttle: 0,
            brake: 0,
            clutch: 0,
            buttons: 0,
            joy_x: 0,
            joy_y: 0,
        });
    }
}

impl Drop for Wheel {
    fn drop(&mut self) {
        unsafe {
            (self.api.reset)(self.id);
            (self.api.relinquish)(self.id);
        }
    }
}
