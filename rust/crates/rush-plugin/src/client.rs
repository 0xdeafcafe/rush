//! Reaching the broker (`rush plugind`), which runs every plugin: for
//! session hosts and the CLI. Plugins can't reach it; their sandbox
//! connects nowhere but their proxy.
//! Ported from internal/plugin/client.go.

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::value::RawValue;

use crate::ctx::Ctx;
use crate::rpc::{CODE_SERVER, Conn, Error, Handler, Raw, to_raw};

/// Where the broker listens for rush itself.
pub fn broker_sock() -> PathBuf {
    crate::root().join("broker.sock")
}

/// Held by the running broker, so there is only ever one.
pub fn broker_lock() -> PathBuf {
    crate::root().join("broker.lock")
}

/// Where the broker writes what happened to its plugins.
pub fn broker_log() -> PathBuf {
    crate::root().join("broker.log")
}

/// Connects to the running broker.
pub fn dial_broker() -> std::io::Result<Conn> {
    dial_broker_with(None)
}

/// Connects to the running broker, answering what it sends with h: how a
/// rush window hears from its plugins.
pub fn dial_broker_with(h: Option<Handler>) -> std::io::Result<Conn> {
    let s = UnixStream::connect(broker_sock())?;
    Conn::new(s, h)
}

/// Starts the broker if any plugin runs and it isn't running. It returns
/// once the broker is starting, not started.
pub fn ensure_broker() -> Result<(), String> {
    if crate::bundled::enabled().is_empty() {
        return Ok(());
    }
    if UnixStream::connect(broker_sock()).is_ok() {
        return Ok(());
    }
    // In a test this binary is the test binary, not rush: it has no
    // broker to start (rush_core::sandbox's variable, read here as
    // rush-plugin doesn't use rush-core).
    if std::env::var_os("RUSH_TEST_SANDBOX").is_some() {
        return Err("no plugin broker in a test".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    crate::mkdir_all(&crate::root())?;
    let log = broker_log();
    let lg = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log)
        .map_err(|e| crate::gopath::os_err("open", &log, &e))?;
    let lg2 = lg.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&exe);
    cmd.arg("plugind")
        .stdin(Stdio::null())
        .stdout(Stdio::from(lg))
        .stderr(Stdio::from(lg2));
    // Its own session, so it outlives whoever started it.
    // SAFETY: setsid is async-signal-safe, as pre_exec needs.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = crate::gopath::os_err("fork/exec", &exe, &e);
            rush_events::log("plugind-launch", fields(&[("err", msg.clone().into())]));
            return Err(msg);
        }
    };
    rush_events::log(
        "plugind-launch",
        fields(&[
            ("child", child.id().into()),
            (
                "why",
                "a plugin is approved and the broker wasn't running".into(),
            ),
        ]),
    );
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// What the broker says of one plugin it runs (Go's plugind.Status).
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Status {
    pub name: String,
    /// starting, running, waiting, refused
    pub state: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pid: i64,
    pub since: rush_state::Time,
    #[serde(skip_serializing_if = "is_zero")]
    pub restarts: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Asks the running broker what it runs (Go's plugind.Ask).
pub fn ask_status() -> Result<Vec<Status>, String> {
    let c = dial_broker().map_err(|e| e.to_string())?;
    let ctx = Ctx::background().with_timeout(Duration::from_secs(3));
    let out: Result<Option<Vec<Status>>, _> = c.call(&ctx, "status", &());
    c.close();
    out.map(Option::unwrap_or_default)
        .map_err(|e| e.to_string())
}

/// Tells the running broker the approvals (or switches) changed, starting
/// it if it isn't running: Go's plugind.Reload and hooks.Reload, which
/// are the same call.
pub fn reload() -> Result<(), String> {
    let Ok(c) = dial_broker() else {
        return ensure_broker();
    };
    let ctx = Ctx::background().with_timeout(Duration::from_secs(3));
    let out: Result<serde_json::Value, _> = c.call(&ctx, "reload", &());
    c.close();
    out.map(|_| ()).map_err(|e| e.to_string())
}

fn fields(kv: &[(&str, serde_json::Value)]) -> rush_events::Fields {
    kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// A session host's connection to the broker, made when first needed and
/// again after it drops.
#[derive(Default)]
pub struct Broker {
    conn: Mutex<Option<Conn>>,
}

impl Broker {
    pub fn new() -> Broker {
        Broker::default()
    }

    fn get(&self, ctx: &Ctx) -> Result<Conn, String> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = conn.as_ref() {
            if !c.is_done() {
                return Ok(c.clone());
            }
            *conn = None;
        }
        let mut started = false;
        loop {
            if let Ok(c) = dial_broker() {
                *conn = Some(c.clone());
                return Ok(c);
            }
            if !started {
                started = true;
                ensure_broker()?;
            }
            if !ctx.sleep(Duration::from_millis(50)) {
                return Err("the plugin broker is not running".into());
            }
        }
    }

    /// Passes one of Claude Code's MCP messages to a plugin and returns the
    /// plugin's reply, or an error reply of its own: Claude Code always
    /// gets an answer.
    pub fn mcp(&self, plugin: &str, session: &str, msg: &RawValue) -> Raw {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Head {
            #[serde(deserialize_with = "some_raw")]
            id: Option<Raw>,
            method: String,
        }
        let head: Head = serde_json::from_str(msg.get()).unwrap_or_default();
        let Some(id) = head.id else {
            // A notification: nothing to pass on, and Claude Code expects
            // an empty result.
            return raw(r#"{"jsonrpc":"2.0","result":{}}"#);
        };
        let timeout = if head.method == "tools/call" {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(30)
        };
        let ctx = Ctx::background().with_timeout(timeout);
        // map[string]any, written in key order.
        #[derive(Serialize)]
        struct Params<'a> {
            message: &'a RawValue,
            plugin: &'a str,
            session: &'a str,
        }
        let res = match self.get(&ctx) {
            Err(e) => Err((CODE_SERVER, e)),
            Ok(c) => match to_raw(&Params {
                message: msg,
                plugin,
                session,
            }) {
                Err(e) => Err((CODE_SERVER, e.message)),
                // A plugin's own error code goes back as it gave it.
                Ok(p) => c
                    .call_raw(&ctx, "mcp", Some(p))
                    .map_err(|e| (e.rpc().map_or(CODE_SERVER, |r| r.code), e.to_string())),
            },
        };
        match res {
            Ok(r) => r.unwrap_or_else(|| raw("null")),
            Err((code, text)) => error_reply(plugin, &id, code, &text),
        }
    }
}

/// The answer Claude Code gets when a plugin couldn't give one.
fn error_reply(plugin: &str, id: &RawValue, code: i64, text: &str) -> Raw {
    // map[string]any, written in key order.
    #[derive(Serialize)]
    struct Reply<'a> {
        error: Error,
        id: &'a RawValue,
        jsonrpc: &'a str,
    }
    let e = Error::new(code, format!("rush plugin {plugin}: {text}"));
    to_raw(&Reply {
        error: e,
        id,
        jsonrpc: "2.0",
    })
    .unwrap_or_else(|_| raw("null"))
}

fn some_raw<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Raw>, D::Error> {
    use serde::Deserialize;
    Raw::deserialize(d).map(Some)
}

fn raw(s: &str) -> Raw {
    RawValue::from_string(s.to_string()).expect("valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_answers_without_a_broker() {
        let home = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(home.path().to_path_buf()));
        let b = Broker::new();
        let note = b.mcp(
            "p",
            "s",
            &raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
        );
        assert_eq!(note.get(), r#"{"jsonrpc":"2.0","result":{}}"#);
        let got = error_reply(
            "p",
            &raw("7"),
            CODE_SERVER,
            "the plugin broker is not running",
        );
        assert_eq!(
            got.get(),
            r#"{"error":{"code":-32000,"message":"rush plugin p: the plugin broker is not running"},"id":7,"jsonrpc":"2.0"}"#
        );
        rush_state::set_thread_dir(None);
    }
}
