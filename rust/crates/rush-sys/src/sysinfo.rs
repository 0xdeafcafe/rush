//! The machine's battery and free disk, for the top bar. Both are read in
//! the background at most every so often, so drawing a frame never waits
//! on them.
//!
//! Ported from internal/sysinfo/sysinfo.go:1-67, pmset.go:1-24,
//! battery_darwin.go:1-19 and battery_other.go:1-26.

use std::sync::Mutex;
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
}

static CUR: Mutex<Cur> = Mutex::new(Cur {
    bat: Battery {
        present: false,
        percent: 0,
        charging: false,
    },
    disk: Disk { free: 0, total: 0 },
    at: None,
    reading: false,
});

/// The last readings, starting a new one when they're stale. Before the
/// first has finished they're zero.
pub fn now() -> (Battery, Disk) {
    let mut c = CUR.lock().unwrap();
    if !c.reading && c.at.is_none_or(|at| at.elapsed() >= EVERY) {
        c.reading = true;
        std::thread::spawn(refresh);
    }
    (c.bat, c.disk)
}

fn refresh() {
    let b = read_battery();
    let d = read_disk();
    let mut c = CUR.lock().unwrap();
    c.bat = b;
    c.disk = d;
    c.at = Some(Instant::now());
    c.reading = false;
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

/// The battery, from `pmset -g batt`.
#[cfg(target_os = "macos")]
pub fn read_battery() -> Battery {
    match crate::cmd::output_within(
        std::process::Command::new("/usr/bin/pmset").args(["-g", "batt"]),
        Duration::from_secs(2),
    ) {
        Ok(o) if o.ok() => parse_pmset(&String::from_utf8_lossy(&o.out)),
        _ => Battery::default(),
    }
}

/// The battery, from Linux's power supply class; elsewhere there's none.
#[cfg(not(target_os = "macos"))]
pub fn read_battery() -> Battery {
    let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else {
        return Battery::default();
    };
    let mut dirs: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("BAT"))
        })
        .collect();
    dirs.sort();
    let Some(d) = dirs.first() else {
        return Battery::default();
    };
    let Ok(c) = std::fs::read_to_string(d.join("capacity")) else {
        return Battery::default();
    };
    let st = std::fs::read_to_string(d.join("status")).unwrap_or_default();
    Battery {
        present: true,
        percent: c.trim().parse().unwrap_or(0),
        charging: st.trim() != "Discharging",
    }
}

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
