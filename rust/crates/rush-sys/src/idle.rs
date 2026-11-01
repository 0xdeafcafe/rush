//! Whether you're at the machine (Rust-only): how long since any key or
//! mouse anywhere on it, and whether its screen is locked. The runaway
//! guard's away mode asks; both are cheap calls (IOKit's HIDIdleTime, the
//! window server's session dictionary). None where it can't tell.

use std::time::Duration;

/// How long since the last key or mouse anywhere on the machine.
pub fn since_input() -> Option<Duration> {
    #[cfg(target_os = "macos")]
    {
        mac::hid_idle()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Whether the screen is locked.
pub fn screen_locked() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        Some(mac::locked())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_void};
    use std::time::Duration;

    type Ref = *const c_void;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> Ref;
        fn IOServiceGetMatchingService(port: u32, matching: Ref) -> u32;
        fn IORegistryEntryCreateCFProperty(entry: u32, key: Ref, alloc: Ref, opts: u32) -> Ref;
        fn IOObjectRelease(o: u32) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(r: Ref);
        fn CFNumberGetValue(n: Ref, kind: isize, out: *mut c_void) -> u8;
        fn CFStringCreateWithCString(alloc: Ref, s: *const c_char, enc: u32) -> Ref;
        fn CFDictionaryGetValue(d: Ref, key: Ref) -> Ref;
        fn CFBooleanGetValue(b: Ref) -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> Ref;
    }

    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;

    struct Owned(Ref);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: an object this owns, from a Copy or Create call.
                unsafe { CFRelease(self.0) }
            }
        }
    }

    fn cfstr(s: &std::ffi::CStr) -> Owned {
        // SAFETY: a NUL-terminated string; a null allocator is the default.
        Owned(unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) })
    }

    pub fn hid_idle() -> Option<Duration> {
        // SAFETY: IOServiceMatching's dictionary is consumed by
        // IOServiceGetMatchingService; the service is released; the
        // property is a CFNumber of nanoseconds, released by Owned.
        unsafe {
            let svc = IOServiceGetMatchingService(0, IOServiceMatching(c"IOHIDSystem".as_ptr()));
            if svc == 0 {
                return None;
            }
            let key = cfstr(c"HIDIdleTime");
            let n = Owned(IORegistryEntryCreateCFProperty(
                svc,
                key.0,
                std::ptr::null(),
                0,
            ));
            IOObjectRelease(svc);
            if n.0.is_null() {
                return None;
            }
            let mut v: i64 = 0;
            (CFNumberGetValue(n.0, SINT64, (&raw mut v).cast()) != 0)
                .then(|| Duration::from_nanos(v.max(0) as u64))
        }
    }

    pub fn locked() -> bool {
        // SAFETY: a Copy call, released by Owned; the value lives with it.
        unsafe {
            let d = Owned(CGSessionCopyCurrentDictionary());
            if d.0.is_null() {
                return false;
            }
            let key = cfstr(c"CGSSessionScreenIsLocked");
            let v = CFDictionaryGetValue(d.0, key.0);
            !v.is_null() && CFBooleanGetValue(v) != 0
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn asks_without_failing() {
        // Whatever the machine says, or None: never a crash.
        let _ = super::since_input();
        let _ = super::screen_locked();
    }
}
