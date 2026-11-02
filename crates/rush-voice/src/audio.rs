//! The Mac's microphones, asked of CoreAudio directly (listing them needs
//! no permission), and choosing one for the helper to hear. New in the
//! Rust port (no Go original).

use std::ffi::c_void;

use objc2::rc::Retained;
use objc2_foundation::NSString;

use crate::Device;

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        id: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        id: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioUnitSetProperty(
        unit: *mut c_void,
        id: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> i32;
}

const fn code(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SYSTEM: u32 = 1;
const DEVICES: u32 = code(b"dev#");
const DEFAULT_INPUT: u32 = code(b"dIn ");
const STREAMS: u32 = code(b"stm#");
const NAME: u32 = code(b"lnam");
const UID: u32 = code(b"uid ");
const GLOBAL: u32 = code(b"glob");
const INPUT: u32 = code(b"inpt");
const MAIN: u32 = 0;
/// kAudioOutputUnitProperty_CurrentDevice, on kAudioUnitScope_Global.
const CURRENT_DEVICE: u32 = 2000;
const UNIT_GLOBAL: u32 = 0;

fn address(selector: u32, scope: u32) -> Address {
    Address {
        selector,
        scope,
        element: MAIN,
    }
}

fn size(id: u32, selector: u32, scope: u32) -> u32 {
    let mut n = 0u32;
    // SAFETY: CoreAudio writes one u32 to n.
    let st = unsafe {
        AudioObjectGetPropertyDataSize(id, &address(selector, scope), 0, std::ptr::null(), &mut n)
    };
    if st == 0 { n } else { 0 }
}

fn ids(id: u32, selector: u32) -> Vec<u32> {
    let mut n = size(id, selector, GLOBAL);
    let mut v = vec![0u32; n as usize / 4];
    if v.is_empty() {
        return v;
    }
    // SAFETY: v has room for n bytes.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            &address(selector, GLOBAL),
            0,
            std::ptr::null(),
            &mut n,
            v.as_mut_ptr().cast(),
        )
    };
    if st != 0 {
        return Vec::new();
    }
    v.truncate(n as usize / 4);
    v
}

fn one(id: u32, selector: u32) -> Option<u32> {
    let mut v = 0u32;
    let mut n = 4u32;
    // SAFETY: v has room for the four bytes asked for.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            &address(selector, GLOBAL),
            0,
            std::ptr::null(),
            &mut n,
            (&mut v as *mut u32).cast(),
        )
    };
    (st == 0).then_some(v)
}

fn text(id: u32, selector: u32) -> Option<String> {
    let mut p: *mut NSString = std::ptr::null_mut();
    let mut n = size_of::<*mut NSString>() as u32;
    // SAFETY: CoreAudio writes one CFString, which it hands over (we own
    // the reference); a CFString is an NSString.
    unsafe {
        let st = AudioObjectGetPropertyData(
            id,
            &address(selector, GLOBAL),
            0,
            std::ptr::null(),
            &mut n,
            (&mut p as *mut *mut NSString).cast(),
        );
        if st != 0 {
            return None;
        }
        Retained::from_raw(p).map(|s| s.to_string())
    }
}

/// The Mac's microphones (devices with an input), the input first.
pub(crate) fn devices() -> Vec<Device> {
    let default = one(SYSTEM, DEFAULT_INPUT).unwrap_or(0);
    let mut out: Vec<Device> = ids(SYSTEM, DEVICES)
        .into_iter()
        .filter(|&d| size(d, STREAMS, INPUT) > 0)
        .filter_map(|d| {
            Some(Device {
                id: text(d, UID)?,
                name: text(d, NAME).unwrap_or_default(),
                default: d == default,
            })
        })
        .collect();
    out.sort_by_key(|d| !d.default);
    out
}

/// CoreAudio's number for the microphone with this unique id, if the Mac
/// still has it.
pub(crate) fn find(uid: &str) -> Option<u32> {
    ids(SYSTEM, DEVICES)
        .into_iter()
        .find(|&d| size(d, STREAMS, INPUT) > 0 && text(d, UID).as_deref() == Some(uid))
}

/// Points an input's audio unit (AVAudioEngine's inputNode.audioUnit) at
/// a microphone; before the engine starts. The error is CoreAudio's.
///
/// # Safety
/// unit must be a live audio unit.
pub(crate) unsafe fn hear(unit: *mut c_void, device: u32) -> Result<(), i32> {
    // SAFETY: the caller's unit; the device is one u32.
    let st = unsafe {
        AudioUnitSetProperty(
            unit,
            CURRENT_DEVICE,
            UNIT_GLOBAL,
            0,
            (&device as *const u32).cast(),
            4,
        )
    };
    if st == 0 { Ok(()) } else { Err(st) }
}
