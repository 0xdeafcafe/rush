//! The plugin broker: the one long-lived `rush plugind` process that runs
//! every approved plugin, sandboxed, and stands between it and everything
//! else. Session hosts send it the MCP messages Claude Code addresses to a
//! plugin's tools; plugins call it to list, start, follow and message
//! sessions, and it checks each call against what the plugin was approved
//! for before doing it. rush's windows attach to it too, to tell plugins
//! what happens on screen and draw what they add ([`hooks`] is a window's
//! side of that).
//!
//! It restarts a plugin that exits (waiting longer each time), ends one
//! that grows past its memory limit, stops one whose files change, and
//! exits itself once no plugin is approved.
//!
//! Ported from Go's internal/plugind (plugind.go:1-11 for this part) and
//! internal/hooks. The socket, the protocol and the words are Go rush's,
//! so a Go rush window or session host talks to a Rust broker and the
//! other way round.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rush_events::Fields;
use rush_plugin::Raw;
use serde_json::value::RawValue;

mod broker;
mod cmd;
mod exec;
pub mod hooks;
pub mod host;
mod runner;
mod sessions;
mod signals;
mod ui;
mod watch;

pub use broker::{Broker, Status, VERSION, ask, reload, run, run_with};
pub use cmd::{plugin_main, plugind_main};
pub use host::{
    Dialed, FileHosts, HostClient, HostEvent, Hosts, Info, Lister, Op, SpawnConfig, View,
};
pub use sessions::{Session, follow};

/// Where a broker runs: rush's folder as it was when it started, and its
/// own log of events (rush's, unless a test gives it one), carried to every
/// thread it starts so all of them read and write the same files.
#[derive(Clone)]
pub struct Scope {
    home: PathBuf,
    events: Option<Arc<rush_events::Logger>>,
}

impl Scope {
    /// rush's folder as this thread sees it now, logging to rush's log.
    pub fn here() -> Scope {
        Scope {
            home: rush_state::dir(),
            events: None,
        }
    }

    /// rush's folder as this thread sees it now, logging events to l.
    pub fn with_events(l: Arc<rush_events::Logger>) -> Scope {
        Scope {
            home: rush_state::dir(),
            events: Some(l),
        }
    }

    /// Makes this thread see rush's folder as the broker does.
    pub fn enter(&self) {
        rush_state::set_thread_dir(Some(self.home.clone()));
    }

    /// Records an event in rush's log.
    pub fn log(&self, kind: &str, fields: Fields) {
        match &self.events {
            Some(l) => l.log(kind, fields),
            None => rush_events::log(kind, fields),
        }
    }

    /// Starts a thread that sees what this one does.
    pub(crate) fn spawn(&self, name: &str, f: impl FnOnce() + Send + 'static) {
        let s = self.clone();
        let _ = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                s.enter();
                f()
            });
    }
}

/// Where the broker says what happened, a line each, as Go's log.Logger
/// with log.LstdFlags writes: the prefix, the local date and time, the
/// words.
#[derive(Clone)]
pub struct Log {
    prefix: String,
    out: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Log {
    /// To stderr, which `rush plugind` has as broker.log.
    pub fn stderr(prefix: &str) -> Log {
        Log::to(Box::new(std::io::stderr()), prefix)
    }

    /// Nowhere.
    pub fn discard() -> Log {
        Log::to(Box::new(std::io::sink()), "")
    }

    pub fn to(w: Box<dyn Write + Send>, prefix: &str) -> Log {
        Log {
            prefix: prefix.into(),
            out: Arc::new(Mutex::new(w)),
        }
    }

    /// The same place, under another prefix.
    pub fn named(&self, prefix: &str) -> Log {
        Log {
            prefix: prefix.into(),
            out: self.out.clone(),
        }
    }

    pub fn print(&self, msg: &str) {
        let mut line = format!("{}{} {msg}", self.prefix, lstd_now());
        if !line.ends_with('\n') {
            line.push('\n');
        }
        let mut w = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = w.write_all(line.as_bytes());
    }

    /// As the proxy takes one.
    pub(crate) fn as_proxy_log(&self) -> rush_plugin::proxy::Logger {
        let l = self.clone();
        Arc::new(move |m: &str| l.print(m))
    }
}

/// Now as log.LstdFlags writes it: 2009/01/23 01:23:23, local time.
fn lstd_now() -> String {
    // SAFETY: time and localtime_r only read the clock and the zone files,
    // into a tm of our own.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

/// Fields for an event, from pairs.
pub(crate) fn fields(kv: &[(&str, serde_json::Value)]) -> Fields {
    kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// JSON as it was given.
pub(crate) fn raw(s: &str) -> Raw {
    RawValue::from_string(s.to_string()).unwrap_or_else(|_| raw("null"))
}

/// v as JSON, as Go writes it; null when it can't be.
pub(crate) fn to_raw<T: serde::Serialize + ?Sized>(v: &T) -> Raw {
    rush_plugin::rpc::to_raw(v).unwrap_or_else(|_| raw("null"))
}

/// An object of these members, written in key order as Go writes a
/// map[string]any: values already JSON stay as they were written.
pub(crate) fn object(mut kv: Vec<(&str, Raw)>) -> Raw {
    kv.sort_by(|a, b| a.0.cmp(b.0));
    let mut s = String::from("{");
    for (i, (k, v)) in kv.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&String::from_utf8_lossy(
            &rush_state::json::to_vec(k).unwrap_or_default(),
        ));
        s.push(':');
        s.push_str(v.get());
    }
    s.push('}');
    raw(&s)
}

/// {}: what most calls answer.
pub(crate) fn empty() -> Option<Raw> {
    Some(raw("{}"))
}

/// The error a handler sends for anything that isn't a JSON-RPC error of
/// its own: Go's CodeServer.
pub(crate) fn server(e: impl std::fmt::Display) -> rush_plugin::Error {
    rush_plugin::Error::server(e)
}

pub(crate) fn bad_params(msg: impl Into<String>) -> rush_plugin::Error {
    rush_plugin::Error::new(rush_plugin::rpc::CODE_INVALID_PARAMS, msg)
}

/// A call's failure as the one who asked is told it: the other side's own
/// error as it was, anything else as a server error.
pub(crate) fn call_err(e: rush_plugin::CallError) -> rush_plugin::Error {
    match e {
        rush_plugin::CallError::Rpc(e) => e,
        e => server(e),
    }
}

/// Params read as Go's jsonx reads them; what can't be is invalid params.
pub(crate) fn params<T: serde::de::DeserializeOwned + Default>(
    p: Option<&RawValue>,
) -> Result<T, rush_plugin::Error> {
    if p.is_none() {
        return Ok(T::default());
    }
    rush_plugin::rpc::from_raw(p).map_err(bad_params)
}

/// within says dir is root or under it.
pub(crate) fn within(dir: &str, root: &str) -> bool {
    dir == root || dir.starts_with(&format!("{}/", root.trim_end_matches('/')))
}

/// a, or b when a is empty.
pub(crate) fn or(a: &str, b: &str) -> String {
    if a.is_empty() { b } else { a }.to_string()
}

/// A Duration as Go's String writes one of whole seconds: 1s, 1m0s.
pub(crate) fn go_secs(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else {
        let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
        if h > 0 {
            format!("{h}h{m}m{s}s")
        } else {
            format!("{m}m{s}s")
        }
    }
}

#[cfg(test)]
mod api_tests;
#[cfg(test)]
pub(crate) mod testutil;
#[cfg(test)]
mod ui_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_in_key_order() {
        let o = object(vec![("b", raw("1")), ("a", raw(r#"{"z":1, "y":2}"#))]);
        assert_eq!(o.get(), r#"{"a":{"z":1, "y":2},"b":1}"#);
        assert_eq!(object(vec![]).get(), "{}");
    }

    #[test]
    fn small_things() {
        assert!(within("/a/b", "/a") && within("/a", "/a") && within("/a/b", "/a/"));
        assert!(!within("/ab", "/a"));
        assert_eq!(go_secs(std::time::Duration::from_secs(60)), "1m0s");
        assert_eq!(go_secs(std::time::Duration::from_secs(8)), "8s");
        assert_eq!(or("", "x"), "x");
    }
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
