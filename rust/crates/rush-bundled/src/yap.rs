//! The bundled plugin that shows a yap dictation in the message box it's
//! going to: the words come up as yap hears them, the ones it's still
//! guessing with a light passing over them, and a light runs round the
//! box's edge. yap (github.com/0xdeafcafe/yap) is dictation for macOS on
//! Apple's own speech model; it says what it hears on a socket of its own,
//! which the window reads (follow). The plugin is the switch.
//! Ported from internal/bundled/yap/plugin.go:1-116.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rush_plugin::{Bundle, Manifest};
use serde::{Deserialize, Serialize};

/// The yap plugin's.
pub fn manifest() -> Manifest {
    crate::manifest(
        "yap",
        "voice",
        "Shows a yap dictation in the message box as you speak: the words as yap hears them, \
         a light over the ones it's still guessing, and a light round the box. Does nothing without yap.",
        16,
    )
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), false, crate::run_switch)
}

/// Where a dictation is, as yap says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// hidden, listening, finishing (let go, tidying), done (the text is
    /// in settled and its paste follows) or failed.
    pub phase: String,
    /// words it's sure of
    pub settled: String,
    /// words it may still change
    pub guessing: String,
    /// how loud, 0 to 100
    pub level: i64,
    /// hands free: it listens until fn is tapped
    pub locked: bool,
    /// the bundle id of the app it'll paste into
    pub app: String,
}

impl State {
    /// Whether there's a dictation to show.
    pub fn active(&self) -> bool {
        matches!(self.phase.as_str(), "listening" | "finishing" | "done")
    }
}

/// Where yap says what it hears.
pub fn socket() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("yap").join("live.sock")
}

/// How long follow waits between looks for yap.
const RETRY: Duration = Duration::from_secs(2);

/// Hands out each state yap reports until stop is sent to or dropped,
/// finding yap again whenever it starts. Losing yap mid-dictation ends the
/// dictation.
pub fn follow(stop: mpsc::Receiver<()>, out: impl FnMut(State)) {
    follow_at(&socket(), stop, out);
}

fn follow_at(path: &std::path::Path, stop: mpsc::Receiver<()>, mut out: impl FnMut(State)) {
    // stop is watched on a thread of its own, which wakes a blocked read
    // by shutting the socket down. Both sides take the lock, so a socket
    // handed over is always seen by a stop that comes after.
    let (stopped_tx, stopped) = mpsc::channel::<()>();
    type Shared = std::sync::Arc<std::sync::Mutex<(bool, Option<UnixStream>)>>;
    let current: Shared = Default::default();
    let cur = current.clone();
    std::thread::spawn(move || {
        let _ = stop.recv();
        let mut g = cur.lock().unwrap_or_else(|e| e.into_inner());
        g.0 = true;
        if let Some(c) = g.1.take() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
        drop(g);
        let _ = stopped_tx.send(());
    });
    loop {
        if let Ok(c) = UnixStream::connect(path)
            && let Ok(c2) = c.try_clone()
        {
            {
                let mut g = current.lock().unwrap_or_else(|e| e.into_inner());
                if g.0 {
                    return;
                }
                g.1 = Some(c2);
            }
            read(c, &mut out);
            current.lock().unwrap_or_else(|e| e.into_inner()).1.take();
        }
        match stopped.recv_timeout(RETRY) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => return,
        }
    }
}

fn read(c: UnixStream, out: &mut impl FnMut(State)) {
    let mut r = BufReader::with_capacity(4096, c);
    let mut shown = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        // A line past the scanner's 1 MiB ends the read, as in Go.
        if line.len() > 1 << 20 {
            break;
        }
        let Ok(s) = serde_json::from_slice::<State>(line.trim_ascii_end()) else {
            continue;
        };
        shown = s.active();
        out(s);
    }
    if shown {
        out(State {
            phase: "hidden".into(),
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    // plugin_test.go TestFollowReadsYapAndEndsWithIt
    #[test]
    fn follow_reads_yap_and_ends_with_it() {
        let home = tempfile::tempdir().unwrap();
        let sock = home.path().join("live.sock");
        let l = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            let Ok((mut c, _)) = l.accept() else { return };
            let _ = c.write_all(
                b"{\"phase\":\"listening\",\"settled\":\"hello\",\"guessing\":\"wor\",\"level\":40}\nnot json\n",
            );
            // yap quits mid-dictation.
        });
        let (got_tx, got) = mpsc::channel();
        let (stop_tx, stop) = mpsc::channel();
        let t = std::thread::spawn(move || {
            follow_at(&sock, stop, move |s| {
                let _ = got_tx.send(s);
            })
        });
        let want = [
            State {
                phase: "listening".into(),
                settled: "hello".into(),
                guessing: "wor".into(),
                level: 40,
                ..Default::default()
            },
            State {
                phase: "hidden".into(),
                ..Default::default()
            },
        ];
        for w in want {
            let s = got
                .recv_timeout(Duration::from_secs(5))
                .expect("nothing came");
            assert_eq!(s, w);
        }
        drop(stop_tx);
        t.join().unwrap();
    }
}
