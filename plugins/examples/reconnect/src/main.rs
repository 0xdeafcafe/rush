//! reconnect rebuilds rush's own retry as a plugin, to show the UI hooks
//! are enough for it: when a session stops because the network went away,
//! or on an error worth another go, it sends "continue" once the network is
//! back, backing off between tries, and says so when it gives up. It never
//! continues a session stopped by a usage limit, a login problem, or
//! anything else.
//!
//! It hears what rush's agent list shows of sessions, never what was said,
//! and may send only to sessions in its workspaces that ask before acting.
//! rush's built-in retry stays as it is; this exists to show the API.
//!
//! ```sh
//! cargo build --release -p reconnect
//! mkdir -p ~/.config/rush/plugins/reconnect
//! cp target/release/reconnect plugins/examples/reconnect/plugin.json ~/.config/rush/plugins/reconnect/
//! rush plugin check reconnect && rush plugin approve reconnect
//! ```
//!
//! Ported from plugins/examples/reconnect/main.go.

mod retry;

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rush_plugin_sdk::proto::{Initialize, UiEvent, UiSettings};
use rush_plugin_sdk::rpc::CODE_NO_METHOD;
use rush_plugin_sdk::{Conn, Error};
use serde_json::{Value, json};

use retry::{Action, Retrier, parse_config};

/// How long it waits for rush to answer.
const ANSWER: Duration = Duration::from_secs(90);

struct App {
    r: Mutex<Retrier>,
    /// Woken when an event came, so the loop looks again.
    kick: Condvar,
    kicked: Mutex<bool>,
}

fn main() {
    let app = Arc::new(App {
        r: Mutex::new(Retrier::new(parse_config(&Default::default()))),
        kick: Condvar::new(),
        kicked: Mutex::new(false),
    });
    let a = app.clone();
    let conn = rush_plugin_sdk::connect(move |_, method, p| handle(&a, method, p));
    let c = conn.clone();
    std::thread::spawn(move || run_loop(&app, &c));
    conn.wait();
}

fn handle(app: &App, method: &str, p: Value) -> Result<Value, Error> {
    match method {
        "initialize" => {
            let init: Initialize = serde_json::from_value(p).unwrap_or_default();
            lock(&app.r).config = parse_config(&init.settings);
            Ok(json!({}))
        }
        "ui.settings" => {
            let s: UiSettings = serde_json::from_value(p).unwrap_or_default();
            lock(&app.r).config = parse_config(&s.values);
            Ok(json!({}))
        }
        "tools.list" => Ok(json!({"tools": []})),
        "ui.event" => {
            if let Ok(ev) = serde_json::from_value::<UiEvent>(p) {
                event(app, &ev);
            }
            Ok(json!({}))
        }
        "ui.command" => Err(Error::new(CODE_NO_METHOD, "reconnect has no commands")),
        _ => Err(Error::no_method(method)),
    }
}

/// Takes what happened. It runs on the thread that reads from rush, so it
/// only changes the state; the loop does the sending.
fn event(app: &App, ev: &UiEvent) {
    let now = Instant::now();
    {
        let mut r = lock(&app.r);
        match ev.kind.as_str() {
            "session.stopped" => {
                if let Some(s) = &ev.session {
                    let alongside = r.config.alongside;
                    let kind = match &ev.error {
                        Some(e) if !e.retrying || alongside => e.kind.as_str(),
                        _ => "",
                    };
                    r.stopped(&s.id, &s.name, kind, now);
                }
            }
            "turn.started" => {
                if let Some(s) = &ev.session {
                    r.started(&s.id);
                }
            }
            "network.down" => r.down(),
            "network.up" => r.up(now),
            _ => {}
        }
    }
    *lock(&app.kicked) = true;
    app.kick.notify_one();
}

/// Does what's due, then sleeps until something next is.
fn run_loop(app: &App, conn: &Conn) {
    loop {
        let (due, next) = {
            let mut r = lock(&app.r);
            (r.due(Instant::now()), r.next())
        };
        for d in due {
            act(conn, app, &d);
        }
        let wait = next.map_or(Duration::from_secs(3600), |t| {
            t.saturating_duration_since(Instant::now())
                .max(Duration::from_millis(50))
        });
        let kicked = lock(&app.kicked);
        let (mut kicked, _) = app
            .kick
            .wait_timeout_while(kicked, wait, |k| !*k)
            .unwrap_or_else(|e| e.into_inner());
        *kicked = false;
    }
}

fn act(conn: &Conn, app: &App, d: &Action) {
    if d.give_up {
        notify(
            conn,
            &d.session,
            &format!("gave up after {} tries", d.tries),
        );
        return;
    }
    let sent = conn.call_timeout::<_, Value>(
        "ui.send",
        &json!({"session": d.session, "text": "continue"}),
        ANSWER,
    );
    if let Err(e) = sent {
        // Refused (outside the workspaces, a mode that doesn't ask, or
        // over ten a minute) or gone. The rate case (-32002) could go
        // later, but it's rare enough to give up with the rest.
        lock(&app.r).forget(&d.session);
        notify(conn, &d.session, &format!("can't continue it: {e}"));
        return;
    }
    eprintln!("continued {} (try {})", d.session, d.tries);
}

/// Says something about a session; rush names the plugin and it.
fn notify(conn: &Conn, session: &str, text: &str) {
    let _ = conn.call_timeout::<_, Value>(
        "ui.notify",
        &json!({"session": session, "text": text, "tone": "warn"}),
        ANSWER,
    );
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn manifest_is_valid() {
        let m = rush_plugin_sdk::Manifest::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(m.ui, ["events", "send", "notify"]);
        assert_eq!(m.settings.len(), 3);
    }
}
