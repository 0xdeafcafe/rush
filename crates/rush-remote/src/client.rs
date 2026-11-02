//! Asking another machine's serve: its sessions, a session started there
//! (posted once, never again), and whether an attach to it is running
//! here. After Go's internal/remote/create.go (ErrMaybeStarted :21,
//! StartOn :29, Attached :62), watch.go (peerSessions :111, peerGet :119)
//! and git.go (peerPost :213).

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::{Peer, dir, or};
use crate::server::{ErrorBody, Session, StartRequest};

/// What marks a start whose outcome isn't known: the request went out,
/// but no whole answer came back, so the session may exist there.
pub const MAYBE_STARTED: &str = "may or may not have started";

/// Go's resp.Status: "404 Not Found".
pub fn status_line(code: u16) -> String {
    let t = crate::http::status_text(code);
    if t.is_empty() {
        code.to_string()
    } else {
        format!("{code} {t}")
    }
}

fn agent(most: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(most))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Reads JSON from a peer's serve.
pub fn peer_get<T: DeserializeOwned>(p: &Peer, path: &str, most: Duration) -> Result<T, String> {
    let mut r = agent(most)
        .get(&format!("{}{path}", p.url))
        .header("Authorization", &format!("Bearer {}", p.token))
        .call()
        .map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    let b = r
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("{}{path}: {}", p.name, status_line(status)));
    }
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}

/// Posts JSON to a peer's serve and reads its answer.
pub fn peer_post<B: Serialize, T: DeserializeOwned>(
    p: &Peer,
    path: &str,
    body: &B,
    most: Duration,
) -> Result<T, String> {
    let b = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let mut r = agent(most)
        .post(&format!("{}{path}", p.url))
        .header("Authorization", &format!("Bearer {}", p.token))
        .header("Content-Type", "application/json")
        .send(&b[..])
        .map_err(|e| e.to_string())?;
    let status = r.status().as_u16();
    let raw = r
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if status != 200 {
        let e: ErrorBody = serde_json::from_slice(&raw).unwrap_or_default();
        return Err(format!(
            "{}: {}",
            p.name,
            or(&e.error, &status_line(status))
        ));
    }
    serde_json::from_slice(&raw).map_err(|e| e.to_string())
}

/// A peer's sessions.
pub fn peer_sessions(p: &Peer) -> Result<Vec<Session>, String> {
    peer_get::<Option<Vec<Session>>>(p, "/api/sessions", Duration::from_secs(10))
        .map(Option::unwrap_or_default)
}

/// Asks peer p's serve to start a session: one POST /api/sessions, never
/// sent again here. A failure with no answer may still have started one
/// there; the caller says so rather than retry. The folder is the
/// server's to read and check, never resolved here.
pub fn start_on(p: &Peer, r: &StartRequest) -> Result<Session, String> {
    let body = serde_json::to_vec(r).map_err(|e| e.to_string())?;
    let res = agent(Duration::from_secs(30))
        .post(&format!("{}/api/sessions", p.url))
        .header("Authorization", &format!("Bearer {}", p.token))
        .header("Content-Type", "application/json")
        .send(&body[..]);
    let mut res =
        res.map_err(|e| format!("no answer from {}, so it {MAYBE_STARTED}: {e}", p.name))?;
    let status = res.status().as_u16();
    let raw = res.body_mut().with_config().limit(1 << 20).read_to_vec();
    if status >= 300 {
        let e: ErrorBody = raw
            .as_deref()
            .ok()
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or_default();
        return Err(format!(
            "{}: {}",
            p.name,
            or(&e.error, &status_line(status))
        ));
    }
    let read = raw
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<Session>(&b).map_err(|e| e.to_string()));
    // It answered 2xx: the session is likely there.
    read.map_err(|e| {
        format!(
            "{} answered but its reply couldn't be read, so it {MAYBE_STARTED}: {e}",
            p.name
        )
    })
}

/// Whether a rush remote attach to machine is running here.
pub fn attached(machine: &str) -> bool {
    attached_at(&dir().join(format!("attach-{machine}.lock")))
}

fn attached_at(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        return false;
    };
    // SAFETY: flock on a file this holds open.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if r == 0 {
        // SAFETY: as above.
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_is_the_lock_held() {
        use std::os::fd::AsRawFd;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("attach-mini.lock");
        assert!(!attached_at(&p));
        let f = std::fs::File::create(&p).unwrap();
        assert!(!attached_at(&p));
        // SAFETY: flock on a file this holds open.
        assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) }, 0);
        assert!(attached_at(&p));
        drop(f);
        assert!(!attached_at(&p));
    }

    #[test]
    fn status_lines() {
        assert_eq!(status_line(404), "404 Not Found");
        assert_eq!(status_line(599), "599");
    }
}
