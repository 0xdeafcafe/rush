//! autodrafts rebuilds rush's own drafts as a plugin, to show the UI hooks
//! are enough for it: it hears what you type in a message box, keeps what
//! you left unsent (you left the Session, or cleared the box), and puts the
//! newest back when you ask.
//!
//! It sees everything you type in rush, and keeps it only in its data
//! folder, in drafts.json. It has no network and can't start programs.
//! rush's own stash is its bundled drafts plugin; this exists to show the
//! API.
//!
//! ```sh
//! cargo build --release -p autodrafts
//! mkdir -p ~/.config/rush/plugins/autodrafts
//! cp target/release/autodrafts plugins/examples/autodrafts/plugin.json ~/.config/rush/plugins/autodrafts/
//! rush plugin check autodrafts && rush plugin approve autodrafts
//! ```

mod drafts;

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use rush_plugin_sdk::proto::{Initialize, UiCommand, UiEvent, UiSession, UiSettings};
use rush_plugin_sdk::rpc::CODE_NO_METHOD;
use rush_plugin_sdk::{Conn, Error};
use serde_json::{Value, json};

use drafts::{Book, parse_keep};

/// How long it waits for rush to answer.
const ANSWER: Duration = Duration::from_secs(90);

#[derive(Default)]
struct State {
    book: Option<Book>,
    path: PathBuf,
    /// The store changed since it was last written.
    dirty: bool,
}

struct App {
    state: Mutex<State>,
    /// Woken when an event came, so the loop looks again.
    kick: Condvar,
    kicked: Mutex<bool>,
    /// One save at a time.
    saving: Mutex<()>,
}

impl App {
    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

/// The key of the box an event is about: a Session's by its id, the
/// Prompt's as "".
fn box_of(s: &Option<UiSession>) -> &str {
    s.as_ref().map_or("", |s| s.id.as_str())
}

fn main() {
    let app = Arc::new(App {
        state: Mutex::new(State::default()),
        kick: Condvar::new(),
        kicked: Mutex::new(false),
        saving: Mutex::new(()),
    });
    let a = app.clone();
    std::thread::spawn(move || settle_loop(&a));
    rush_plugin_sdk::serve(move |conn, method, p| handle(conn, &app, method, p))
}

fn handle(conn: &Conn, app: &App, method: &str, p: Value) -> Result<Value, Error> {
    match method {
        "initialize" => {
            let init: Initialize = serde_json::from_value(p).unwrap_or_default();
            let dir = if init.data_dir.is_empty() {
                std::env::var("RUSH_PLUGIN_DATA").unwrap_or_default()
            } else {
                init.data_dir
            };
            let path = PathBuf::from(dir).join("drafts.json");
            let keep = parse_keep(init.settings.get("keep").map_or("", String::as_str));
            let mut st = app.lock();
            st.book = Some(Book::new(drafts::load(&path), keep));
            st.path = path;
            Ok(json!({}))
        }
        "tools.list" => Ok(json!({"tools": []})),
        "ui.settings" => {
            let s: UiSettings = serde_json::from_value(p).unwrap_or_default();
            let keep = parse_keep(s.values.get("keep").map_or("", String::as_str));
            let dropped = app.lock().book.as_mut().is_some_and(|b| b.set_keep(keep));
            if dropped {
                save(app);
            }
            Ok(json!({}))
        }
        "ui.event" => {
            if let Ok(ev) = serde_json::from_value::<UiEvent>(p) {
                event(app, &ev);
            }
            Ok(json!({}))
        }
        "ui.command" => {
            let c: UiCommand = serde_json::from_value(p).unwrap_or_default();
            command(conn, app, &c)
        }
        _ => Err(Error::no_method(method)),
    }
}

/// Takes one thing that happened to a box. Events arrive in order, on the
/// thread that reads from rush, so it only notes them: the loop writes the
/// file.
fn event(app: &App, ev: &UiEvent) {
    let now = SystemTime::now();
    let key = box_of(&ev.session);
    {
        let mut st = app.lock();
        let Some(book) = st.book.as_mut() else {
            return;
        };
        let dirty = match ev.kind.as_str() {
            "input.changed" => {
                book.changed(key, &ev.text, now);
                false
            }
            "input.cleared" => !ev.text.is_empty() && book.keep(key, &ev.text, now),
            "session.left" => book.keep(key, "", now),
            "input.sent" => book.sent(key),
            _ => false,
        };
        st.dirty |= dirty;
    }
    *lock(&app.kicked) = true;
    app.kick.notify_one();
}

fn command(conn: &Conn, app: &App, c: &UiCommand) -> Result<Value, Error> {
    let key = box_of(&c.session);
    let mut st = app.lock();
    let Some(book) = st.book.as_mut() else {
        return Err(Error::server("not started yet"));
    };
    match c.command.as_str() {
        "restore" => {
            let text = book.restore(key);
            drop(st);
            let Some(text) = text else {
                notify(conn, &c.ui, "no drafts kept for this box", "dim");
                return Ok(json!({}));
            };
            save(app);
            // The session id is the box's: "" is the Prompt.
            conn.call_timeout::<_, Value>(
                "ui.input.set",
                &json!({"ui": c.ui, "session": key, "text": text}),
                ANSWER,
            )
            .map_err(|e| Error::server(format!("ui.input.set: {e}")))?;
            Ok(json!({}))
        }
        "list" => {
            let n = book.count(key);
            drop(st);
            let where_ = match &c.session {
                None => "the Prompt",
                Some(s) if s.name.is_empty() => "this Session",
                Some(s) => s.name.as_str(),
            };
            let msg = if n == 1 {
                format!("1 draft kept for {where_}")
            } else {
                format!("{n} drafts kept for {where_}")
            };
            notify(conn, &c.ui, &msg, "");
            Ok(json!({}))
        }
        name => Err(Error::new(CODE_NO_METHOD, format!("no command {name}"))),
    }
}

fn notify(conn: &Conn, ui: &str, text: &str, tone: &str) {
    let _ = conn.call_timeout::<_, Value>(
        "ui.notify",
        &json!({"ui": ui, "text": text, "tone": tone}),
        ANSWER,
    );
}

/// Settles boxes once they've stopped changing, writing autosaves.
fn settle_loop(app: &App) {
    loop {
        let (dirty, next) = {
            let mut st = app.lock();
            let st = &mut *st;
            match st.book.as_mut() {
                Some(b) => {
                    let dirty = b.settle(SystemTime::now()) || st.dirty;
                    st.dirty = false;
                    (dirty, b.next_settle())
                }
                None => (false, None),
            }
        };
        if dirty {
            save(app);
        }
        let wait = next.map_or(Duration::from_secs(3600), |t| {
            t.duration_since(SystemTime::now())
                .unwrap_or_default()
                .max(Duration::from_millis(10))
        });
        let kicked = lock(&app.kicked);
        let (mut kicked, _) = app
            .kick
            .wait_timeout_while(kicked, wait, |k| !*k)
            .unwrap_or_else(|e| e.into_inner());
        *kicked = false;
    }
}

/// Writes what's kept, as it is now.
fn save(app: &App) {
    let _one = lock(&app.saving);
    let (path, store) = {
        let st = app.lock();
        let Some(book) = &st.book else {
            return;
        };
        (st.path.clone(), book.store.clone())
    };
    if path.as_os_str().is_empty() {
        return;
    }
    if let Err(e) = drafts::save(&path, &store) {
        eprintln!("saving drafts: {e}");
    }
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
        assert_eq!(m.ui, ["events", "input", "notify"]);
        assert_eq!(m.commands.len(), 2);
    }
}
