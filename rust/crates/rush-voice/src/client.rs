//! The window's side of a dictation: starting the helper if need be, and
//! talking to it over its socket. New in the Rust port (no Go original).

use std::io::{BufRead, BufReader, Write as _};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{Command, Live, bundle};

/// How long a helper just opened has to start answering.
const STARTUP: Duration = Duration::from_secs(5);

/// One dictation's line to the helper.
pub(crate) struct Conn {
    stream: Mutex<UnixStream>,
    finished: Arc<AtomicBool>,
}

impl Conn {
    /// Sends one command; a helper that's gone has nothing to hear it.
    pub(crate) fn send(&self, cmd: &Command) {
        let mut s = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.write_all(&encode(cmd));
    }

    pub(crate) fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Says nothing more comes from this side.
    pub(crate) fn close(&self) {
        let s = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.shutdown(Shutdown::Write);
    }
}

/// One JSON line.
pub(crate) fn encode<T: serde::Serialize>(v: &T) -> Vec<u8> {
    let mut b = serde_json::to_vec(v).unwrap_or_default();
    b.push(b'\n');
    b
}

pub(crate) fn start(
    device: &str,
    context: Vec<String>,
    mut out: impl FnMut(Live) + Send + 'static,
) -> Result<Conn, String> {
    if bundle::build_app()? {
        // An older helper may still be running: the new one takes over.
        bundle::quit();
    }
    let sock = crate::socket();
    let stream = match UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(_) => {
            bundle::launch()?;
            connect_soon(&sock)?
        }
    };
    let reader = stream.try_clone().map_err(|e| e.to_string())?;
    let conn = Conn {
        stream: Mutex::new(stream),
        finished: Arc::new(AtomicBool::new(false)),
    };
    conn.send(&Command::Start {
        device: device.to_string(),
        context,
    });
    let finished = conn.finished.clone();
    std::thread::Builder::new()
        .name("rush-voice".into())
        .spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Ok(live) = serde_json::from_str::<Live>(&line) else {
                    continue;
                };
                let end = live.is_end();
                if end {
                    finished.store(true, Ordering::SeqCst);
                }
                out(live);
                if end {
                    return;
                }
            }
            if !finished.swap(true, Ordering::SeqCst) {
                out(Live {
                    phase: "failed".into(),
                    error: "rush voice stopped before the words were finished".into(),
                    ..Live::default()
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Connects to a helper that's just been opened, giving it a few seconds
/// to start answering.
fn connect_soon(sock: &Path) -> Result<UnixStream, String> {
    let until = Instant::now() + STARTUP;
    loop {
        match UnixStream::connect(sock) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() >= until => {
                return Err(format!("rush voice didn't start: {e}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}
