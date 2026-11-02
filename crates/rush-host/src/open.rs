//! Opening a session's host for the UI: a socket that refuses, or isn't
//! there, is read against the host's info before it's called a failure.
//! A host that has gone (stopped, asleep, or ended without a word and its
//! socket left behind) is a stopped session, to be shown from its
//! transcript as any other: its leftover socket is removed. A host still
//! starting is waited for, its folder and its exit watched (no polling),
//! up to a few seconds. Only a host that runs and still doesn't answer,
//! or a dial that fails some other way, is a failure, and the UI logs it
//! as session-open-failed with the raw error (note_failed).
//!
//! Rust-only: Go's openHost (rushmode.go:1866) said "couldn't open the
//! session: <error>" for any of these.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::client::{Client, ClientError, read_info, running};
use crate::events::log_event;
use crate::paths::{dir, sock_path};
use crate::wire::Info;

/// Why a session's host couldn't be opened.
#[derive(Debug)]
pub enum Unreached {
    /// Its host has gone: the session is a stopped one, its info as it was
    /// left (stopped, or lost when it went without a word).
    Stopped(Option<Box<Info>>),
    /// Its host runs but didn't answer within the wait.
    Silent(ClientError),
    /// The dial failed some other way.
    Failed(ClientError),
}

impl Unreached {
    /// How the event names it (session-open-failed's reason).
    pub fn reason(&self) -> &'static str {
        match self {
            Unreached::Stopped(_) => "stopped",
            Unreached::Silent(_) => "not-answering",
            Unreached::Failed(_) => "failed",
        }
    }
}

/// Connects to session id's host, waiting up to wait for one still
/// starting. Waits: never on the UI thread.
pub fn open(id: &str, wait: Duration) -> Result<Client, Unreached> {
    let deadline = Instant::now() + wait;
    let mut watch: Option<Watch> = None;
    loop {
        let err = match Client::dial(id) {
            Ok(c) => return Ok(c),
            Err(e) => e,
        };
        let refused = match &err {
            ClientError::Io(e) => match e.kind() {
                std::io::ErrorKind::ConnectionRefused => true,
                std::io::ErrorKind::NotFound => false,
                _ => return Err(Unreached::Failed(err)),
            },
            _ => return Err(Unreached::Failed(err)),
        };
        let info = read_info(id).ok();
        let Some(pid) = info.as_ref().filter(|i| up(i)).map(|i| i.host_pid) else {
            if refused {
                remove_left(id, &err);
            }
            return Err(Unreached::Stopped(info.map(gone)));
        };
        let now = Instant::now();
        if now >= deadline {
            return Err(Unreached::Silent(err));
        }
        let w = watch.get_or_insert_with(|| Watch::new(id, pid, deadline - now));
        w.wait(deadline - now);
    }
}

/// Logs that opening session id failed, reason and the raw error, so
/// `rush why` shows it.
pub fn note_failed(id: &str, reason: &str, err: &str) {
    log_event(
        id,
        "session-open-failed",
        json!({"reason": reason, "error": err}),
    );
}

/// Whether info's host is up or coming up: neither stopped nor asleep,
/// its process running.
fn up(i: &Info) -> bool {
    i.state != "stopped" && !i.sleeping && running(i)
}

/// A gone host's info as a listing reads it (Lister: revive::lose).
fn gone(mut i: Info) -> Box<Info> {
    if !i.sleeping && i.state != "stopped" {
        crate::revive::lose(&mut i);
    }
    Box::new(i)
}

/// Removes the socket a gone host left, refusing every dial: unless a
/// host is being started for it (its wake lock held), or one came up
/// since its info was read.
fn remove_left(id: &str, err: &ClientError) {
    use std::os::fd::AsRawFd;
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir(id).join("wake.lock"))
    else {
        return;
    };
    // SAFETY: lock's descriptor is open; LOCK_NB never waits.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return; // a host is being started: its own start removes it
    }
    if read_info(id).is_ok_and(|i| up(&i)) {
        return;
    }
    if std::fs::remove_file(sock_path(id)).is_ok() {
        log_event(
            id,
            "host-socket-removed",
            json!({"why": "its host had gone", "error": err.to_string()}),
        );
    }
}

/// What wakes a wait for a host coming up: its folder changing (its
/// socket bound, its info written) or its process ending.
struct Watch {
    w: rush_sys::fswait::Watcher,
    stop: Option<Arc<rush_sys::fswait::Stop>>,
}

impl Watch {
    fn new(id: &str, pid: i64, most: Duration) -> Watch {
        let w = rush_sys::fswait::Watcher::new();
        w.watch(&[dir(id)]);
        let stop = rush_sys::fswait::Stop::new().ok().map(Arc::new);
        if let Some(s) = stop.clone() {
            std::thread::spawn(move || {
                crate::sys::wait_exit(pid, most);
                s.close();
            });
        }
        Watch { w, stop }
    }

    fn wait(&self, most: Duration) {
        self.w.wait(self.stop.as_deref(), Some(most));
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::process::{Child, Command, Stdio};

    use super::*;
    use crate::testing::Home;

    fn write_info(id: &str, state: &str, host_pid: i64) {
        std::fs::create_dir_all(dir(id)).unwrap();
        let info = Info {
            id: id.into(),
            state: state.into(),
            host_pid,
            ..Default::default()
        };
        std::fs::write(
            dir(id).join("info.json"),
            serde_json::to_vec(&info).unwrap(),
        )
        .unwrap();
    }

    /// A socket file with nothing listening: what a host that ended
    /// without tidying leaves.
    fn left_socket(id: &str) {
        drop(UnixListener::bind(sock_path(id)).unwrap());
        assert!(sock_path(id).exists());
    }

    /// A process of this test's that runs until killed: a host's pid.
    fn live() -> Child {
        Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn events(id: &str) -> String {
        crate::events::flush_test();
        std::fs::read_to_string(crate::paths::events_path(id)).unwrap_or_default()
    }

    // A refused or missing socket whose host has gone is a stopped
    // session, read as a listing reads it, its leftover socket removed:
    // no failure.
    #[test]
    fn a_gone_hosts_socket_is_a_stopped_session() {
        let _h = Home::new();
        let cases: [(&str, &str, bool, &str); 4] = [
            ("gone-idle", "idle", true, "stopped"),
            ("gone-working", "working", true, "stopped"),
            ("said-stopped", "stopped", true, "stopped"),
            ("no-socket", "idle", false, "stopped"),
        ];
        for (id, state, sock, want) in cases {
            write_info(id, state, 0);
            if sock {
                left_socket(id);
            }
            let i = match open(id, Duration::from_secs(5)) {
                Err(Unreached::Stopped(Some(i))) => i,
                r => panic!("{id}: {:?}", r.err()),
            };
            assert_eq!(i.state, want, "{id}");
            assert!(!sock_path(id).exists(), "{id}: its socket stayed");
        }
        assert!(events("gone-idle").contains("\"host-socket-removed\""));
        // No info at all: stopped too, with nothing to show of it.
        std::fs::create_dir_all(dir("bare")).unwrap();
        left_socket("bare");
        assert!(matches!(
            open("bare", Duration::ZERO),
            Err(Unreached::Stopped(None))
        ));
    }

    // A host that runs but doesn't answer is a failure, its raw error
    // kept; the failure's event says why. Once its process has ended it's
    // a stopped one.
    #[test]
    fn a_running_host_that_doesnt_answer_fails() {
        let _h = Home::new();
        let mut p = live();
        write_info("silent", "idle", p.id() as i64);
        left_socket("silent");
        let e = match open("silent", Duration::ZERO) {
            Err(e) => e,
            Ok(_) => panic!("reached a host that isn't listening"),
        };
        let Unreached::Silent(ClientError::Io(io)) = &e else {
            panic!("{e:?}");
        };
        assert_eq!(io.kind(), std::io::ErrorKind::ConnectionRefused);
        assert!(sock_path("silent").exists(), "a live host's socket stays");
        note_failed("silent", e.reason(), &io.to_string());
        let ev = events("silent");
        assert!(ev.contains("\"session-open-failed\""), "{ev}");
        assert!(ev.contains("\"not-answering\""), "{ev}");
        assert!(ev.contains(&io.to_string()), "{ev}");
        p.kill().unwrap();
        p.wait().unwrap();
        assert!(matches!(
            open("silent", Duration::from_secs(5)),
            Err(Unreached::Stopped(_))
        ));
        assert!(!sock_path("silent").exists());
    }

    // A host still starting is waited for: reached once it listens.
    #[test]
    fn a_starting_host_is_waited_for() {
        let h = Home::new();
        let mut p = live();
        write_info("starting", "starting", p.id() as i64);
        left_socket("starting");
        let home = h.path();
        let t = std::thread::spawn(move || {
            Home::enter(&home);
            let _ = std::fs::remove_file(sock_path("starting"));
            let ln = UnixListener::bind(sock_path("starting")).unwrap();
            ln.accept().map(|_| ()).unwrap();
        });
        let r = open("starting", Duration::from_secs(10));
        assert!(r.is_ok(), "{:?}", r.err());
        t.join().unwrap();
        p.kill().unwrap();
        p.wait().unwrap();
    }
}
