//! The machine's battery and free disk, for the top bar. Both are read in
//! the background at most every so often, so drawing a frame never waits
//! on them.
//!
//! Ported from internal/sysinfo/sysinfo.go:1-67, pmset.go:1-24,
//! battery_darwin.go:1-19 and battery_other.go:1-26.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The machine's battery; `present` is false on one without.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Battery {
    pub present: bool,
    pub percent: i32,
    /// Plugged in, whether or not it is filling.
    pub charging: bool,
}

/// The space on the volume agents work on (the home folder's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    pub free: u64,
    pub total: u64,
}

const EVERY: Duration = Duration::from_secs(30);

struct Cur {
    bat: Battery,
    disk: Disk,
    at: Option<Instant>,
    reading: bool,
    /// What [`now`] last handed out: what's on screen.
    given: (Battery, Disk),
}

const NONE: Battery = Battery {
    present: false,
    percent: 0,
    charging: false,
};

static CUR: Mutex<Cur> = Mutex::new(Cur {
    bat: NONE,
    disk: Disk { free: 0, total: 0 },
    at: None,
    reading: false,
    given: (NONE, Disk { free: 0, total: 0 }),
});

/// Whether [`now`] was asked since [`unasked`]: the frame drawn showed
/// the readings.
static ASKED: AtomicBool = AtomicBool::new(false);

/// Set by [`pin`]: the readings stay as given and are never read.
static PINNED: AtomicBool = AtomicBool::new(false);

/// Holds the readings at b and d, never reading the machine: for tests
/// whose frames must not depend on the machine they run on.
pub fn pin(b: Battery, d: Disk) {
    PINNED.store(true, Ordering::Relaxed);
    let mut c = CUR.lock().unwrap();
    c.bat = b;
    c.disk = d;
    c.given = (b, d);
}

/// Told each reading made, with what was last handed out and the new one
/// (see [`on_read`]).
type OnRead = Box<dyn Fn((Battery, Disk), (Battery, Disk)) + Send>;
static ON_READ: Mutex<Option<OnRead>> = Mutex::new(None);

/// The last readings, starting a new one when they're stale. Before the
/// first has finished they're zero.
pub fn now() -> (Battery, Disk) {
    ASKED.store(true, Ordering::Relaxed);
    let mut c = CUR.lock().unwrap();
    if !c.reading && !PINNED.load(Ordering::Relaxed) && c.at.is_none_or(|at| at.elapsed() >= EVERY)
    {
        c.reading = true;
        std::thread::spawn(refresh);
    }
    c.given = (c.bat, c.disk);
    (c.bat, c.disk)
}

/// Forgets that the readings were asked for: a frame is about to be
/// drawn, and [`asked`] then says whether it showed them.
pub fn unasked() {
    ASKED.store(false, Ordering::Relaxed);
}

/// Whether the readings were asked for since [`unasked`].
pub fn asked() -> bool {
    ASKED.load(Ordering::Relaxed)
}

/// Reads again now, in the background, however fresh the last reading:
/// for a screen showing them that draws only when they change (Go reads
/// them every 30 seconds while they're drawn, every second).
pub fn read_again() {
    let mut c = CUR.lock().unwrap();
    if !c.reading && !PINNED.load(Ordering::Relaxed) {
        c.reading = true;
        std::thread::spawn(refresh);
    }
}

/// Has f told of each reading made from now on, with what [`now`] last
/// handed out and the new reading, so a screen can draw it if it shows
/// differently. It runs on the reading's thread.
pub fn on_read(f: impl Fn((Battery, Disk), (Battery, Disk)) + Send + 'static) {
    *ON_READ.lock().unwrap() = Some(Box::new(f));
}

fn refresh() {
    let b = read_battery();
    let d = read_disk();
    let given = {
        let mut c = CUR.lock().unwrap();
        c.bat = b;
        c.disk = d;
        c.at = Some(Instant::now());
        c.reading = false;
        c.given
    };
    if let Some(f) = ON_READ.lock().unwrap().as_ref() {
        f(given, (b, d));
    }
}

/// The free and total space of the home folder's volume.
pub fn read_disk() -> Disk {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "/".into());
    let Ok(c) = std::ffi::CString::new(home.as_encoded_bytes()) else {
        return Disk::default();
    };
    // SAFETY: statfs is plain data; statfs(2) fills it.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: c is a NUL-terminated path.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return Disk::default();
    }
    let bs = st.f_bsize as u64;
    Disk {
        free: st.f_bavail as u64 * bs,
        total: st.f_blocks as u64 * bs,
    }
}

/// The battery, as `pmset -g batt` says it (Go runs pmset), read from
/// IOKit's power sources where pmset reads it, with no program started:
/// present with an internal battery, its charge, and charging while the
/// power comes from AC.
pub fn read_battery() -> Battery {
    ps::read().unwrap_or_default()
}

/// The battery, from `pmset -g batt`: what [`read_battery`] reads,
/// the way Go does, to check it against.
pub fn read_battery_pmset() -> Battery {
    match crate::cmd::output_within(
        std::process::Command::new("/usr/bin/pmset").args(["-g", "batt"]),
        Duration::from_secs(2),
    ) {
        Ok(o) if o.ok() => parse_pmset(&String::from_utf8_lossy(&o.out)),
        _ => Battery::default(),
    }
}

/// IOKit's power sources (IOPowerSources.h), as pmset reads them.
mod ps {
    use std::ffi::{c_char, c_void};

    use super::Battery;

    type Ref = *const c_void;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> Ref;
        fn IOPSCopyPowerSourcesList(blob: Ref) -> Ref;
        fn IOPSGetPowerSourceDescription(blob: Ref, ps: Ref) -> Ref;
        fn IOPSGetProvidingPowerSourceType(blob: Ref) -> Ref;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(r: Ref);
        fn CFArrayGetCount(a: Ref) -> isize;
        fn CFArrayGetValueAtIndex(a: Ref, i: isize) -> Ref;
        fn CFDictionaryGetValue(d: Ref, key: Ref) -> Ref;
        fn CFNumberGetValue(n: Ref, kind: isize, out: *mut c_void) -> u8;
        fn CFStringCreateWithCString(alloc: Ref, s: *const c_char, enc: u32) -> Ref;
        fn CFEqual(a: Ref, b: Ref) -> u8;
    }

    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;

    /// A CF object released when dropped.
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

    fn equal(a: Ref, b: &Owned) -> bool {
        // SAFETY: both are live CF objects (a checked not null).
        !a.is_null() && !b.0.is_null() && unsafe { CFEqual(a, b.0) } != 0
    }

    fn number(d: Ref, key: &Owned) -> Option<i64> {
        // SAFETY: d is a live dictionary; the value it holds lives with it.
        let n = unsafe { CFDictionaryGetValue(d, key.0) };
        let mut v: i64 = 0;
        // SAFETY: n is a CFNumber (the keys asked for hold numbers), v
        // fits a 64-bit integer.
        (!n.is_null() && unsafe { CFNumberGetValue(n, SINT64, (&raw mut v).cast()) } != 0)
            .then_some(v)
    }

    /// The internal battery, or none; None when IOKit can't say.
    pub fn read() -> Option<Battery> {
        // SAFETY: Copy calls, released by Owned; the descriptions and the
        // providing type are got, not copied, and live with the blob.
        unsafe {
            let blob = Owned(IOPSCopyPowerSourcesInfo());
            if blob.0.is_null() {
                return None;
            }
            let list = Owned(IOPSCopyPowerSourcesList(blob.0));
            if list.0.is_null() {
                return None;
            }
            let (typ, internal) = (cfstr(c"Type"), cfstr(c"InternalBattery"));
            let (cur, max) = (cfstr(c"Current Capacity"), cfstr(c"Max Capacity"));
            for i in 0..CFArrayGetCount(list.0) {
                let d = IOPSGetPowerSourceDescription(blob.0, CFArrayGetValueAtIndex(list.0, i));
                if d.is_null() || !equal(CFDictionaryGetValue(d, typ.0), &internal) {
                    continue;
                }
                let (Some(c), Some(m)) = (number(d, &cur), number(d, &max)) else {
                    continue;
                };
                let percent = if m > 0 && m != 100 {
                    (c as f64 * 100.0 / m as f64).round() as i32
                } else {
                    c as i32
                };
                return Some(Battery {
                    present: true,
                    percent,
                    charging: equal(IOPSGetProvidingPowerSourceType(blob.0), &cfstr(c"AC Power")),
                });
            }
            Some(Battery::default())
        }
    }
}

/// The battery, from Linux's power supply class; elsewhere there's none.

/// Reads `pmset -g batt`: its first line says what power it's drawing
/// from, the next the battery's charge.
pub fn parse_pmset(out: &str) -> Battery {
    let (first, rest) = out.split_once('\n').unwrap_or((out, ""));
    if !rest.contains("InternalBattery") {
        return Battery::default();
    }
    // The first run of digits followed by '%'.
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let j = i + b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            if b.get(j) == Some(&b'%') {
                return Battery {
                    present: true,
                    percent: rest[i..j].parse().unwrap_or(0),
                    charging: first.contains("AC Power"),
                };
            }
            i = j;
        } else {
            i += 1;
        }
    }
    Battery::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // IOKit's reading is the one pmset prints (Go reads pmset).
    #[test]
    fn iokit_reads_as_pmset_does() {
        let (a, b) = (read_battery(), read_battery_pmset());
        // The charge may move a point between the two reads.
        assert_eq!((a.present, a.charging), (b.present, b.charging));
        assert!((a.percent - b.percent).abs() <= 1, "{a:?} {b:?}");
    }

    #[test]
    fn pmset() {
        for (out, want) in [
            (
                "Now drawing from 'AC Power'\n -InternalBattery-0 (id=28442723)\t7%; AC attached; not charging present: true\n",
                Battery {
                    present: true,
                    percent: 7,
                    charging: true,
                },
            ),
            (
                "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t64%; discharging; 3:12 remaining present: true\n",
                Battery {
                    present: true,
                    percent: 64,
                    charging: false,
                },
            ),
            ("Now drawing from 'AC Power'\n", Battery::default()), // a desktop
        ] {
            assert_eq!(parse_pmset(out), want, "{out:?}");
        }
    }

    #[test]
    fn disk_is_read() {
        let d = read_disk();
        assert!(d.total > 0 && d.free <= d.total, "{d:?}");
    }
}
