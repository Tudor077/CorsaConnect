//! Device-level camera exposure/gain control via DirectShow.
//!
//! In a dim room, webcams on auto-exposure stretch the shutter time and the
//! frame rate collapses (often to 5-10 fps), which is the main source of head
//! tracking lag. Forcing a short manual exposure keeps the camera at full
//! rate; our software gain then compensates for the darker image.
//!
//! These are UVC device controls, so a second DirectShow handle can set them
//! while nokhwa/MediaFoundation streams. (nokhwa's own control setter reuses
//! the current Auto flag, so it can't switch a control to Manual.)

use std::ffi::c_void;

// ---------------------------------------------------------------- COM FFI

type Guid = [u8; 16];
type HResult = i32;

// {62BE5D10-60EB-11d0-BD3B-00A0C911CE86} CLSID_SystemDeviceEnum
const CLSID_SYSTEM_DEVICE_ENUM: Guid = guid(0x62BE5D10, 0x60EB, 0x11d0, [0xBD, 0x3B, 0x00, 0xA0, 0xC9, 0x11, 0xCE, 0x86]);
// {29840822-5B84-11D0-BD3B-00A0C911CE86} IID_ICreateDevEnum
const IID_ICREATE_DEV_ENUM: Guid = guid(0x29840822, 0x5B84, 0x11D0, [0xBD, 0x3B, 0x00, 0xA0, 0xC9, 0x11, 0xCE, 0x86]);
// {860BB310-5D01-11d0-BD3B-00A0C911CE86} CLSID_VideoInputDeviceCategory
const CLSID_VIDEO_INPUT_DEVICE_CATEGORY: Guid = guid(0x860BB310, 0x5D01, 0x11d0, [0xBD, 0x3B, 0x00, 0xA0, 0xC9, 0x11, 0xCE, 0x86]);
// {C6E13370-30AC-11d0-A18C-00A0C9118956} IID_IAMCameraControl
const IID_IAM_CAMERA_CONTROL: Guid = guid(0xC6E13370, 0x30AC, 0x11d0, [0xA1, 0x8C, 0x00, 0xA0, 0xC9, 0x11, 0x89, 0x56]);
// {C6E13360-30AC-11d0-A18C-00A0C9118956} IID_IAMVideoProcAmp
const IID_IAM_VIDEO_PROC_AMP: Guid = guid(0xC6E13360, 0x30AC, 0x11d0, [0xA1, 0x8C, 0x00, 0xA0, 0xC9, 0x11, 0x89, 0x56]);
// {55272A00-42CB-11CE-8135-00AA004BB851} IID_IPropertyBag
const IID_IPROPERTY_BAG: Guid = guid(0x55272A00, 0x42CB, 0x11CE, [0x81, 0x35, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51]);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
    let mut g = [0u8; 16];
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    g[0] = a[0]; g[1] = a[1]; g[2] = a[2]; g[3] = a[3];
    g[4] = b[0]; g[5] = b[1];
    g[6] = c[0]; g[7] = c[1];
    let mut i = 0;
    while i < 8 {
        g[8 + i] = d[i];
        i += 1;
    }
    g
}

const CLSCTX_INPROC_SERVER: u32 = 0x1;
const COINIT_MULTITHREADED: u32 = 0x0;

const CAMERA_CONTROL_EXPOSURE: i32 = 4; // CameraControl_Exposure
const VIDEO_PROC_AMP_GAIN: i32 = 8; // VideoProcAmp_Gain
const FLAGS_AUTO: i32 = 0x1;
const FLAGS_MANUAL: i32 = 0x2;

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> HResult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        cls_context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult;
}
#[link(name = "oleaut32")]
extern "system" {
    fn VariantClear(var: *mut Variant) -> HResult;
}

/// Minimal VARIANT: vt + padding + BSTR pointer field.
#[repr(C)]
struct Variant {
    vt: u16,
    _r0: u16,
    _r1: u16,
    _r2: u16,
    val: *mut u16, // BSTR when vt == VT_BSTR
    _pad: usize,
}

const VT_BSTR: u16 = 8;

/// A COM interface pointer: first field of the object is the vtable.
#[repr(C)]
struct ComObj {
    vtbl: *const *const c_void,
}

unsafe fn com_call<R>(obj: *mut ComObj, slot: usize) -> *const c_void {
    let _ = std::marker::PhantomData::<R>;
    *(*obj).vtbl.add(slot)
}

unsafe fn release(obj: *mut ComObj) {
    if obj.is_null() {
        return;
    }
    // IUnknown::Release is vtable slot 2.
    let f: extern "system" fn(*mut ComObj) -> u32 =
        std::mem::transmute(com_call::<u32>(obj, 2));
    f(obj);
}

unsafe fn query_interface(obj: *mut ComObj, iid: &Guid) -> Option<*mut ComObj> {
    let f: extern "system" fn(*mut ComObj, *const Guid, *mut *mut c_void) -> HResult =
        std::mem::transmute(com_call::<HResult>(obj, 0));
    let mut out: *mut c_void = std::ptr::null_mut();
    if f(obj, iid, &mut out) >= 0 && !out.is_null() {
        Some(out as *mut ComObj)
    } else {
        None
    }
}

/// What we changed, so it can be restored.
#[derive(Clone, Copy, Default)]
pub struct Applied {
    pub exposure: bool,
    pub gain: bool,
}

/// Force manual exposure at `ev` (log2 seconds: -5 = 1/32 s ~ 30 fps,
/// -4 = 1/16 s, -3 = 1/8 s) and max out hardware gain on the video device
/// whose friendly name matches `camera_name`, so the sensor keeps a high
/// frame rate in a dark room. Returns what was applied.
pub fn force_low_light(camera_name: &str, ev: i32) -> Result<Applied, String> {
    with_device_controls(camera_name, |cam_ctl, proc_amp| {
        let mut applied = Applied::default();
        unsafe {
            if let Some(ctl) = cam_ctl {
                // IAMCameraControl: 3=GetRange, 4=Set, 5=Get
                let get_range: extern "system" fn(*mut ComObj, i32, *mut i32, *mut i32, *mut i32, *mut i32, *mut i32) -> HResult =
                    std::mem::transmute(com_call::<HResult>(ctl, 3));
                let set: extern "system" fn(*mut ComObj, i32, i32, i32) -> HResult =
                    std::mem::transmute(com_call::<HResult>(ctl, 4));
                let (mut lo, mut hi, mut step, mut def, mut caps) = (0, 0, 0, 0, 0);
                if get_range(ctl, CAMERA_CONTROL_EXPOSURE, &mut lo, &mut hi, &mut step, &mut def, &mut caps) >= 0 {
                    let target = ev.clamp(lo, hi);
                    applied.exposure = set(ctl, CAMERA_CONTROL_EXPOSURE, target, FLAGS_MANUAL) >= 0;
                }
            }
            if let Some(amp) = proc_amp {
                // IAMVideoProcAmp has the same vtable layout.
                let get_range: extern "system" fn(*mut ComObj, i32, *mut i32, *mut i32, *mut i32, *mut i32, *mut i32) -> HResult =
                    std::mem::transmute(com_call::<HResult>(amp, 3));
                let set: extern "system" fn(*mut ComObj, i32, i32, i32) -> HResult =
                    std::mem::transmute(com_call::<HResult>(amp, 4));
                let (mut lo, mut hi, mut step, mut def, mut caps) = (0, 0, 0, 0, 0);
                if get_range(amp, VIDEO_PROC_AMP_GAIN, &mut lo, &mut hi, &mut step, &mut def, &mut caps) >= 0 {
                    // Full range: the NN cares about contrast, not noise.
                    applied.gain = set(amp, VIDEO_PROC_AMP_GAIN, hi, FLAGS_MANUAL) >= 0;
                }
            }
        }
        applied
    })
}

/// Put exposure and gain back on automatic.
pub fn restore_auto(camera_name: &str) -> Result<(), String> {
    with_device_controls(camera_name, |cam_ctl, proc_amp| unsafe {
        if let Some(ctl) = cam_ctl {
            let get_range: extern "system" fn(*mut ComObj, i32, *mut i32, *mut i32, *mut i32, *mut i32, *mut i32) -> HResult =
                std::mem::transmute(com_call::<HResult>(ctl, 3));
            let set: extern "system" fn(*mut ComObj, i32, i32, i32) -> HResult =
                std::mem::transmute(com_call::<HResult>(ctl, 4));
            let (mut lo, mut hi, mut step, mut def, mut caps) = (0, 0, 0, 0, 0);
            if get_range(ctl, CAMERA_CONTROL_EXPOSURE, &mut lo, &mut hi, &mut step, &mut def, &mut caps) >= 0 {
                let _ = set(ctl, CAMERA_CONTROL_EXPOSURE, def, FLAGS_AUTO);
            }
        }
        if let Some(amp) = proc_amp {
            let get_range: extern "system" fn(*mut ComObj, i32, *mut i32, *mut i32, *mut i32, *mut i32, *mut i32) -> HResult =
                std::mem::transmute(com_call::<HResult>(amp, 3));
            let set: extern "system" fn(*mut ComObj, i32, i32, i32) -> HResult =
                std::mem::transmute(com_call::<HResult>(amp, 4));
            let (mut lo, mut hi, mut step, mut def, mut caps) = (0, 0, 0, 0, 0);
            if get_range(amp, VIDEO_PROC_AMP_GAIN, &mut lo, &mut hi, &mut step, &mut def, &mut caps) >= 0 {
                // Gain often has no Auto cap; manual default is the sane reset.
                let _ = set(amp, VIDEO_PROC_AMP_GAIN, def, FLAGS_AUTO | FLAGS_MANUAL);
            }
        }
    })
}

/// Find the DirectShow capture filter matching `camera_name` (fallback: the
/// first video device) and hand its control interfaces to `f`.
fn with_device_controls<R>(
    camera_name: &str,
    f: impl FnOnce(Option<*mut ComObj>, Option<*mut ComObj>) -> R,
) -> Result<R, String> {
    unsafe {
        // S_FALSE (already initialized) is fine.
        let hr = CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED);
        if hr < 0 && hr != -2147417850i32 {
            // RPC_E_CHANGED_MODE: apartment already set up differently; COM
            // still works for our purposes.
            return Err(format!("CoInitializeEx failed: {hr:#x}"));
        }

        let mut dev_enum: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_SYSTEM_DEVICE_ENUM,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_ICREATE_DEV_ENUM,
            &mut dev_enum,
        );
        if hr < 0 {
            return Err(format!("SystemDeviceEnum failed: {hr:#x}"));
        }
        let dev_enum = dev_enum as *mut ComObj;

        // ICreateDevEnum::CreateClassEnumerator is slot 3.
        let create_enum: extern "system" fn(*mut ComObj, *const Guid, *mut *mut ComObj, u32) -> HResult =
            std::mem::transmute(com_call::<HResult>(dev_enum, 3));
        let mut class_enum: *mut ComObj = std::ptr::null_mut();
        let hr = create_enum(dev_enum, &CLSID_VIDEO_INPUT_DEVICE_CATEGORY, &mut class_enum, 0);
        if hr != 0 || class_enum.is_null() {
            release(dev_enum);
            return Err("No video capture devices found.".to_string());
        }

        // IEnumMoniker::Next is slot 3.
        let next: extern "system" fn(*mut ComObj, u32, *mut *mut ComObj, *mut u32) -> HResult =
            std::mem::transmute(com_call::<HResult>(class_enum, 3));

        let mut fallback: Option<*mut ComObj> = None;
        let mut chosen: Option<*mut ComObj> = None;
        loop {
            let mut moniker: *mut ComObj = std::ptr::null_mut();
            let mut fetched = 0u32;
            if next(class_enum, 1, &mut moniker, &mut fetched) != 0 || fetched == 0 {
                break;
            }
            let name = moniker_friendly_name(moniker).unwrap_or_default();
            if chosen.is_none() && !name.is_empty() && name == camera_name {
                chosen = Some(moniker);
                continue;
            }
            if fallback.is_none() {
                fallback = Some(moniker);
            } else {
                release(moniker);
            }
        }
        let moniker = match chosen.or(fallback) {
            Some(m) => m,
            None => {
                release(class_enum);
                release(dev_enum);
                return Err("No video capture devices found.".to_string());
            }
        };

        // IMoniker::BindToObject is slot 8.
        let bind: extern "system" fn(*mut ComObj, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> HResult =
            std::mem::transmute(com_call::<HResult>(moniker, 8));
        // Bind straight to the control interfaces (the filter implements them).
        let mut cam_ctl_raw: *mut c_void = std::ptr::null_mut();
        let _ = bind(moniker, std::ptr::null_mut(), std::ptr::null_mut(), &IID_IAM_CAMERA_CONTROL, &mut cam_ctl_raw);
        let cam_ctl = (!cam_ctl_raw.is_null()).then_some(cam_ctl_raw as *mut ComObj);
        let proc_amp = cam_ctl.and_then(|c| query_interface(c, &IID_IAM_VIDEO_PROC_AMP));

        let result = f(cam_ctl, proc_amp);

        if let Some(p) = proc_amp {
            release(p);
        }
        if let Some(c) = cam_ctl {
            release(c);
        }
        release(moniker);
        release(class_enum);
        release(dev_enum);
        Ok(result)
    }
}

/// Read the device FriendlyName from the moniker's property bag.
unsafe fn moniker_friendly_name(moniker: *mut ComObj) -> Option<String> {
    // IMoniker::BindToStorage is slot 9.
    let bind_storage: extern "system" fn(*mut ComObj, *mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> HResult =
        std::mem::transmute(com_call::<HResult>(moniker, 9));
    let mut bag_raw: *mut c_void = std::ptr::null_mut();
    if bind_storage(moniker, std::ptr::null_mut(), std::ptr::null_mut(), &IID_IPROPERTY_BAG, &mut bag_raw) < 0
        || bag_raw.is_null()
    {
        return None;
    }
    let bag = bag_raw as *mut ComObj;
    // IPropertyBag::Read is slot 3.
    let read: extern "system" fn(*mut ComObj, *const u16, *mut Variant, *mut c_void) -> HResult =
        std::mem::transmute(com_call::<HResult>(bag, 3));
    let key: Vec<u16> = "FriendlyName".encode_utf16().chain(std::iter::once(0)).collect();
    let mut var = Variant {
        vt: 0,
        _r0: 0,
        _r1: 0,
        _r2: 0,
        val: std::ptr::null_mut(),
        _pad: 0,
    };
    let name = if read(bag, key.as_ptr(), &mut var, std::ptr::null_mut()) >= 0
        && var.vt == VT_BSTR
        && !var.val.is_null()
    {
        let mut len = 0usize;
        while *var.val.add(len) != 0 {
            len += 1;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(var.val, len)))
    } else {
        None
    };
    let _ = VariantClear(&mut var);
    release(bag);
    name
}
