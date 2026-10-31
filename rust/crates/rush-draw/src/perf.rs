//! Rust-only timing log for measuring rush, off unless `RUSH_PERF_LOG`
//! names a file. Each line starts with the wall clock in microseconds
//! since the Unix epoch:
//!
//! - `start` once: when the process started (its kernel start time), so a
//!   harness can measure startup without counting how it launched rush.
//! - `frame`: one per frame written to the terminal: what was handled
//!   for it (`keys`, `msgs`, `wake`), the time spent handling (`upd`) and
//!   drawing and writing (`draw`) in µs, how many cells changed
//!   (`cells`), and for frames that answer input, `lat`: µs from the
//!   oldest key's arrival in the loop's queue to the frame written.
//! - `mark`: the first frame whose text holds `RUSH_PERF_MARK`.
//! - `shown <text>` / `gone <text>`: for each of `RUSH_PERF_WATCH`'s
//!   texts (split by `|`), the frames it appears in and leaves: how long
//!   a placeholder such as "reading…" stayed on screen.
//!
//! Off, it costs one load of a OnceLock per frame and per key.

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct Perf {
    file: Mutex<File>,
    mark: Option<String>,
    marked: AtomicBool,
    /// RUSH_PERF_WATCH's texts, each with whether the last frame held it.
    watch: Vec<(String, AtomicBool)>,
    /// The last frame's changed cells, set by the terminal.
    cells: AtomicU64,
}

static PERF: OnceLock<Option<Perf>> = OnceLock::new();

/// The log, when asked for.
pub(crate) fn on() -> Option<&'static Perf> {
    PERF.get_or_init(|| {
        let path = std::env::var_os("RUSH_PERF_LOG").filter(|p| !p.is_empty())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        let p = Perf {
            file: Mutex::new(file),
            mark: std::env::var("RUSH_PERF_MARK")
                .ok()
                .filter(|m| !m.is_empty()),
            marked: AtomicBool::new(false),
            watch: std::env::var("RUSH_PERF_WATCH")
                .unwrap_or_default()
                .split('|')
                .filter(|w| !w.is_empty())
                .map(|w| (w.to_string(), AtomicBool::new(false)))
                .collect(),
            cells: AtomicU64::new(0),
        };
        p.line(&format!(
            "{} start pid={}",
            started_us(),
            std::process::id()
        ));
        Some(p)
    })
    .as_ref()
}

/// Microseconds since the Unix epoch, now.
pub(crate) fn now_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

/// When this process started, from the kernel (macOS: kinfo_proc's
/// p_starttime); now where that can't be read.
fn started_us() -> u128 {
    #[cfg(target_os = "macos")]
    {
        let mut mib = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PID,
            std::process::id() as libc::c_int,
        ];
        let mut buf = [0u8; 1024];
        let mut len = buf.len();
        // SAFETY: mib and buf are valid for their lengths; len is buf's.
        let r = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if r == 0 && len >= 16 {
            // kp_proc.p_starttime: struct timeval { i64 sec; i32 usec }.
            let sec = i64::from_ne_bytes(buf[0..8].try_into().unwrap_or_default());
            let usec = i32::from_ne_bytes(buf[8..12].try_into().unwrap_or_default());
            if sec > 0 {
                return sec as u128 * 1_000_000 + usec.max(0) as u128;
            }
        }
    }
    now_us()
}

impl Perf {
    pub(crate) fn line(&self, s: &str) {
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{s}");
        }
    }

    /// Called by the terminal for each frame it writes: the cells
    /// changed, and the frame's text when a mark is still looked for.
    pub(crate) fn drawn(&self, cells: usize, text: impl FnOnce() -> String) {
        self.cells.store(cells as u64, Ordering::Relaxed);
        let want = self.mark.is_some() && !self.marked.load(Ordering::Relaxed);
        if !want && self.watch.is_empty() {
            return;
        }
        let text = text();
        if let Some(m) = &self.mark
            && want
            && text.contains(m.as_str())
        {
            self.marked.store(true, Ordering::Relaxed);
            self.line(&format!("{} mark", now_us()));
        }
        for (w, was) in &self.watch {
            let is = text.contains(w.as_str());
            if is != was.swap(is, Ordering::Relaxed) {
                let what = if is { "shown" } else { "gone" };
                self.line(&format!("{} {what} {w}", now_us()));
            }
        }
    }

    pub(crate) fn cells(&self) -> u64 {
        self.cells.load(Ordering::Relaxed)
    }
}

/// Notes a line in the timing log, when it's on: what the app itself
/// times (a session's host dialled, its history read). `what` is only
/// called when the log is on.
pub fn note(what: impl FnOnce() -> String) {
    if let Some(p) = on() {
        p.line(&format!("{} {}", now_us(), what()));
    }
}
