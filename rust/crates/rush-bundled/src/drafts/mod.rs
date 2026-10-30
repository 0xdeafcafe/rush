//! The drafts plugin wired to rush: events in, its commands and picks, and
//! the calls it makes back to set boxes, note them and say what it did.
//! The book (book.rs) is kept in its data folder's drafts.json, written
//! 300ms after the last change.
//! Ported from internal/bundled/drafts/plugin.go:1-394.

mod book;

pub use book::*;

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rush_plugin::rpc::{Error, from_raw, no_method};
use rush_plugin::{
    Bundle, CallError, CommandSpec, Conn, Ctx, EV_INPUT_CHANGED, EV_INPUT_CLEARED, EV_INPUT_SENT,
    EV_SESSION_OPENED, Manifest, Picked, SettingSpec, TextBox, UI_EVENTS, UI_INPUT, UI_NOTIFY,
    UiSession,
};
use rush_state::Time;
use serde::Deserialize;
use serde_json::json;

/// The drafts plugin's.
pub fn manifest() -> Manifest {
    Manifest {
        ui: vec![UI_EVENTS.into(), UI_INPUT.into(), UI_NOTIFY.into()],
        // No keys of its own: rush gives these its stash and history keys,
        // ctrl+p and ctrl+r, and its #stash.
        commands: vec![
            CommandSpec {
                name: "stash".into(),
                description:
                    "set the message aside, or bring it back; it comes back by itself once you send"
                        .into(),
                ..Default::default()
            },
            CommandSpec {
                name: "history".into(),
                description:
                    "what you set aside, sent, cleared and replaced, to put back in the box".into(),
                ..Default::default()
            },
        ],
        settings: vec![
            SettingSpec {
                key: "called".into(),
                title: "Called".into(),
                kind: "choice".into(),
                choices: CALLED.iter().map(|c| c.to_string()).collect(),
                default: CALLED[0].into(),
                description: "What it's called wherever rush shows it: Stash, or Drafts.".into(),
            },
            SettingSpec {
                key: "keep".into(),
                title: "Kept of each kind".into(),
                kind: "choice".into(),
                choices: vec!["100".into(), "300".into(), "1000".into()],
                default: "300".into(),
                description:
                    "How many sent, cleared and replaced messages it keeps; the oldest go first."
                        .into(),
            },
        ],
        ..crate::manifest(
            "drafts",
            "messages",
            "Keeps a message box until it's sent, even across rush restarts; sets a message aside while you send another; \
             and puts back what you set aside, sent, cleared or replaced. Keeps it all in its data folder.",
            64,
        )
    }
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), false, run)
}

/// How long it waits after the last change to write.
const SAVE_AFTER: Duration = Duration::from_millis(300);

#[derive(Default)]
struct State {
    book: Option<Book>,
    path: PathBuf,
    /// When the store is next written; None while nothing waits.
    due: Option<Instant>,
    /// The connection has ended: the saver writes once more and goes.
    done: bool,
}

#[derive(Default)]
struct App {
    st: Mutex<State>,
    /// Wakes the saver when a write is due or the plugin ends.
    wake: Condvar,
    /// One write at a time.
    saving: Mutex<()>,
}

/// The plugin, on its connection to the broker.
fn run(s: UnixStream) -> Result<(), String> {
    let a = Arc::new(App::default());
    let saver = {
        let a = a.clone();
        std::thread::spawn(move || a.saver())
    };
    let h = a.clone();
    let res = crate::serve(
        s,
        Arc::new(move |conn, _, method, params| h.handle(conn, method, params)),
    );
    a.lock().done = true;
    a.wake.notify_all();
    let _ = saver.join();
    a.save();
    res
}

/// What an event leaves to do off the reading thread.
type Later = Box<dyn FnOnce(&App, &Conn) + Send>;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Event {
    kind: String,
    ui: String,
    session: Option<UiSession>,
    text: String,
    #[serde(rename = "box")]
    text_box: Option<TextBox>,
}

impl Event {
    fn key(&self) -> (String, String) {
        match &self.session {
            Some(s) => (s.id.clone(), s.name.clone()),
            None => (String::new(), String::new()),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Init {
    #[serde(rename = "dataDir")]
    data_dir: String,
    settings: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Settings {
    values: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Command {
    command: String,
    ui: String,
    session: Option<UiSession>,
    #[serde(rename = "box")]
    box_id: String,
    input: Option<TextBox>,
}

fn rpc_err(e: CallError) -> Error {
    match e {
        CallError::Rpc(e) => e,
        e => Error::server(e),
    }
}

impl App {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn handle(
        self: &Arc<Self>,
        conn: &Conn,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Option<rush_plugin::Raw>, Error> {
        match method {
            "initialize" => {
                let input: Init = from_raw(params).unwrap_or_default();
                let stashed: Vec<String> = {
                    let mut st = self.lock();
                    st.path = Path::new(&input.data_dir).join("drafts.json");
                    let (s, took) = load(&st.path, Path::new(&input.data_dir));
                    let mut b = Book::new(
                        s,
                        parse_keep(input.settings.get("keep").map_or("", String::as_str)),
                    );
                    b.w = words_for(input.settings.get("called").map_or("", String::as_str));
                    let stashed = b.s.stashes.keys().cloned().collect();
                    st.book = Some(b);
                    if took {
                        self.later(&mut st);
                    }
                    stashed
                };
                // A note from before it restarted went with it.
                let (a, c) = (self.clone(), conn.clone());
                std::thread::spawn(move || {
                    for k in stashed {
                        a.note(&c, &k);
                    }
                });
                crate::empty()
            }
            "ui.settings" => {
                let input: Settings = from_raw(params).unwrap_or_default();
                let renamed = {
                    let mut st = self.lock();
                    let keep = parse_keep(input.values.get("keep").map_or("", String::as_str));
                    let w = words_for(input.values.get("called").map_or("", String::as_str));
                    let (kept_fewer, renamed) = match st.book.as_mut() {
                        Some(b) => {
                            let fewer = b.set_keep(keep);
                            let renamed = b.w != w;
                            b.w = w;
                            (fewer, renamed)
                        }
                        None => (false, false),
                    };
                    if kept_fewer {
                        self.later(&mut st);
                    }
                    renamed
                };
                // The notes on the boxes say the new name.
                if renamed {
                    let (a, c) = (self.clone(), conn.clone());
                    std::thread::spawn(move || {
                        for k in a.stash_keys() {
                            a.note(&c, &k);
                        }
                    });
                }
                Ok(None)
            }
            "ui.event" => {
                if let Ok(ev) = from_raw::<Event>(params) {
                    self.event(conn, ev);
                }
                Ok(None)
            }
            "ui.command" => {
                let input: Command = from_raw(params).map_err(Error::server)?;
                let name = match &input.session {
                    Some(s) if s.id == input.box_id => s.name.clone(),
                    _ => String::new(),
                };
                self.command(
                    conn,
                    &input.command,
                    &input.ui,
                    &input.box_id,
                    &name,
                    input.input,
                )?;
                crate::empty()
            }
            "ui.picked" => {
                let input: Picked = from_raw(params).map_err(Error::server)?;
                self.picked(conn, input);
                crate::empty()
            }
            "tools.list" => crate::no_tools(),
            _ => Err(no_method(method)),
        }
    }

    /// Takes one thing that happened. Events come in order on the
    /// connection's reading thread, so anything that calls rush back goes
    /// on its own.
    fn event(self: &Arc<Self>, conn: &Conn, ev: Event) {
        let (key, name) = ev.key();
        let now = Time::now();
        let mut st = self.lock();
        let Some(b) = st.book.as_mut() else {
            return;
        };
        let spawn = |f: Later| {
            let (a, c) = (self.clone(), conn.clone());
            std::thread::spawn(move || f(&a, &c));
        };
        match ev.kind.as_str() {
            EV_INPUT_CHANGED => {
                let tb = ev.text_box.clone().unwrap_or_else(|| TextBox {
                    text: ev.text.clone(),
                    cursor: ev.text.chars().count() as i64,
                    ..Default::default()
                });
                if b.changed(&key, tb) {
                    let k = key.clone();
                    spawn(Box::new(move |a, c| a.note(c, &k)));
                }
            }
            EV_INPUT_SENT => {
                if let Some(set) = b.sent(&key, &name, &ev.text, now) {
                    let ui = ev.ui.clone();
                    spawn(Box::new(move |a, c| a.set(c, &ui, &set)));
                    let k = key.clone();
                    spawn(Box::new(move |a, c| a.note(c, &k)));
                }
            }
            EV_INPUT_CLEARED => b.cleared(&key, &name, &ev.text, now),
            EV_SESSION_OPENED => {
                if let Some(set) = b.opened(&key) {
                    let ui = ev.ui.clone();
                    spawn(Box::new(move |a, c| a.set(c, &ui, &set)));
                }
                return;
            }
            _ => return,
        }
        self.later(&mut st);
    }

    fn command(
        &self,
        conn: &Conn,
        cmd: &str,
        ui: &str,
        key: &str,
        name: &str,
        input: Option<TextBox>,
    ) -> Result<(), Error> {
        let now = Time::now();
        let mut st = self.lock();
        let Some(b) = st.book.as_mut() else {
            return Ok(());
        };
        match cmd {
            "stash" => {
                let r = b.stash(key, name, input.unwrap_or_default(), now);
                self.later(&mut st);
                drop(st);
                if let Some(set) = &r.set {
                    self.set(conn, ui, set);
                }
                self.note(conn, key);
                self.notify(conn, ui, &r.said);
                Ok(())
            }
            "history" => {
                let p = b.pick(key, now);
                drop(st);
                conn.call::<_, serde_json::Value>(
                    &Ctx::background(),
                    "ui.pick",
                    &json!({ "ui": ui, "pick": p }),
                )
                .map(|_| ())
                .map_err(rpc_err)
            }
            _ => Ok(()),
        }
    }

    fn picked(&self, conn: &Conn, p: Picked) {
        let now = Time::now();
        let mut st = self.lock();
        if st.book.is_none() {
            return;
        }
        self.later(&mut st);
        let Some(b) = st.book.as_mut() else {
            return;
        };
        match p.action.as_str() {
            "ctrl+d" => {
                b.forget(&p.item);
                drop(st);
                self.note(conn, &p.box_id);
            }
            "enter" => {
                let got = b.put_back(&p.item, &p.box_id, "", p.input.as_ref(), now);
                let got = got.map(|(set, said)| (set, b.said(&said)));
                drop(st);
                if let Some((set, said)) = got {
                    self.set(conn, &p.ui, &set);
                    self.note(conn, &p.box_id);
                    for k in self.stash_keys() {
                        self.note(conn, &k);
                    }
                    self.notify(conn, &p.ui, &said);
                }
            }
            _ => {}
        }
    }

    fn stash_keys(&self) -> Vec<String> {
        self.lock()
            .book
            .as_ref()
            .map(|b| b.s.stashes.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Sets a box in rush, only while it holds what the book expects.
    fn set(&self, conn: &Conn, ui: &str, s: &Set) {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(5));
        let _ = conn.call::<_, serde_json::Value>(
            &ctx,
            "ui.input.set",
            &json!({ "ui": ui, "session": s.key, "box": s.text_box, "if": s.if_text }),
        );
    }

    /// Says on a box's edge whether it has a stash.
    fn note(&self, conn: &Conn, key: &str) {
        let text = self
            .lock()
            .book
            .as_ref()
            .map(|b| b.note(key))
            .unwrap_or_default();
        let ctx = Ctx::background().with_timeout(Duration::from_secs(5));
        let _ = conn.call::<_, serde_json::Value>(
            &ctx,
            "ui.box.note",
            &json!({ "session": key, "text": text, "tone": "warn" }),
        );
    }

    fn notify(&self, conn: &Conn, ui: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        let ctx = Ctx::background().with_timeout(Duration::from_secs(5));
        let _ = conn.call::<_, serde_json::Value>(
            &ctx,
            "ui.notify",
            &json!({ "ui": ui, "text": text }),
        );
    }

    /// Writes the store once changes stop for SAVE_AFTER. Under the lock.
    fn later(&self, st: &mut State) {
        st.due = Some(Instant::now() + SAVE_AFTER);
        self.wake.notify_all();
    }

    /// Waits for a write to come due, and makes it: asleep on the condvar
    /// while nothing is due, as Go's time.AfterFunc costs nothing until it
    /// fires.
    fn saver(&self) {
        let mut st = self.lock();
        loop {
            if st.done {
                return;
            }
            match st.due {
                None => {
                    st = self.wake.wait(st).unwrap_or_else(|e| e.into_inner());
                }
                Some(at) => {
                    let now = Instant::now();
                    if now >= at {
                        st.due = None;
                        drop(st);
                        self.save();
                        st = self.lock();
                    } else {
                        st = self
                            .wake
                            .wait_timeout(st, at - now)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                }
            }
        }
    }

    /// Writes the store whole, to a file renamed over the last.
    fn save(&self) {
        let _one = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let (b, path) = {
            let st = self.lock();
            let Some(book) = st.book.as_ref() else {
                return;
            };
            if st.path.as_os_str().is_empty() {
                return;
            }
            (rush_state::json::to_vec(&book.s), st.path.clone())
        };
        let Ok(b) = b else {
            return;
        };
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        if write_private(Path::new(&tmp), &b).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn write_private(path: &Path, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(b)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OldDraft {
    text: String,
    at: Time,
    agent: String,
    name: String,
    kind: String,
    sent: bool,
}

/// Reads the store, and once takes in the drafts rush kept itself before
/// they were a plugin, saying whether it did. Their file is left as it is.
fn load(path: &Path, data_dir: &Path) -> (Store, bool) {
    let mut s: Store = std::fs::read(path)
        .ok()
        .and_then(|b| rush_state::json::from_slice(&b).ok())
        .unwrap_or_default();
    if s.imported {
        return (s, false);
    }
    // The data folder is <rush's folder>/plugin-data/drafts.
    let old_path = data_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new(""))
        .join("drafts.json");
    let old = match std::fs::read(&old_path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(_) => return (s, false), // unreadable for now: tried again next time
    };
    s.imported = true;
    let Some(old) = old else {
        return (s, true);
    };
    let Ok(was) = rush_state::json::from_slice::<Vec<OldDraft>>(&old) else {
        return (s, true);
    };
    // A store from before Imported may have taken them in already.
    let have: std::collections::HashSet<(String, String)> = s
        .history
        .iter()
        .map(|e| (e.kind.clone(), plain(&e.text_box).trim().to_string()))
        .collect();
    for d in was {
        // A draft kept on purpose lands with what put-backs replaced.
        let kind = match d.kind.as_str() {
            "draft" => REPLACED,
            "sent" => SENT,
            "cleared" => CLEARED,
            _ if d.sent => SENT,
            _ => CLEARED,
        };
        if have.contains(&(kind.to_string(), d.text.trim().to_string())) {
            continue;
        }
        s.seq += 1;
        s.history.push(Entry {
            id: format!("h{}", s.seq),
            kind: kind.into(),
            text_box: TextBox {
                cursor: d.text.chars().count() as i64,
                text: d.text,
                ..Default::default()
            },
            session: d.agent,
            name: d.name,
            at: d.at,
        });
    }
    s.history.sort_by_key(|x| std::cmp::Reverse(x.at));
    (s, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Broker;
    use rush_plugin::UiEvent;

    fn b(s: &str) -> TextBox {
        TextBox {
            text: s.into(),
            cursor: s.chars().count() as i64,
            ..Default::default()
        }
    }

    // book_test.go TestLoadTakesInOldDrafts: the drafts rush kept itself
    // are taken in once: on the first run, or by a store from before that
    // never did; their file is left as it was.
    #[test]
    fn load_takes_in_old_drafts() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("plugin-data").join("drafts");
        std::fs::create_dir_all(&data).unwrap();
        let old = r#"[{"text":"older","at":"2026-09-01T00:00:00Z","kind":"draft"},{"text":"newer","at":"2026-09-02T00:00:00Z","sent":true}]"#;
        let (old_path, path) = (root.path().join("drafts.json"), data.join("drafts.json"));
        std::fs::write(&old_path, old).unwrap();
        let (s, took) = load(&path, &data);
        assert!(
            took && s.imported
                && s.history.len() == 2
                && s.history[0].text_box.text == "newer"
                && s.history[0].kind == SENT
                && s.history[1].kind == REPLACED,
            "history: {:?}",
            s.history
        );
        // A store from before, which took in one of them, gets the other.
        let before = Store {
            history: vec![Entry {
                id: "h1".into(),
                kind: SENT.into(),
                text_box: b("newer"),
                at: Time::parse("2026-09-29T12:00:00Z").unwrap(),
                ..Default::default()
            }],
            seq: 1,
            ..Default::default()
        };
        std::fs::write(&path, rush_state::json::to_vec(&before).unwrap()).unwrap();
        let (s, took) = load(&path, &data);
        assert!(
            took && s.history.len() == 2
                && s.history[0].text_box.text == "newer"
                && s.history[1].text_box.text == "older"
                && s.history[1].id == "h2",
            "taken in again: {:?}",
            s.history
        );
        std::fs::write(&path, rush_state::json::to_vec(&s).unwrap()).unwrap();
        let (s, took) = load(&path, &data);
        assert!(!took && s.history.len() == 2, "taken in twice");
        assert_eq!(
            std::fs::read_to_string(&old_path).unwrap(),
            old,
            "the old file should be left as it was"
        );
    }

    // plugin_test.go TestPluginOverItsConnection: the plugin as the broker
    // drives it: events in, commands, and the calls it makes back.
    #[test]
    fn plugin_over_its_connection() {
        let data = tempfile::tempdir().unwrap();
        let br = Broker::start(run, None);
        br.call("initialize", json!({ "dataDir": data.path() }))
            .unwrap();
        let s1 = UiSession {
            id: "s1".into(),
            name: "one".into(),
            ..Default::default()
        };
        let ev = |kind: &str, text: &str, tb: Option<TextBox>| UiEvent {
            kind: kind.into(),
            ui: "w".into(),
            session: Some(s1.clone()),
            text: text.into(),
            input_box: tb,
            ..Default::default()
        };
        br.conn
            .notify(
                "ui.event",
                &ev(
                    EV_INPUT_CHANGED,
                    "hello",
                    Some(TextBox {
                        text: "hello".into(),
                        cursor: 2,
                        ..Default::default()
                    }),
                ),
            )
            .unwrap();
        br.call(
            "ui.command",
            json!({ "command": "stash", "ui": "w", "box": "s1", "session": s1, "input": { "text": "hello", "cursor": 2 } }),
        )
        .unwrap();
        let set = br.want("ui.input.set");
        assert!(
            set["if"] == "hello" && set["session"] == "s1",
            "stash set: {set}"
        );
        let note = br.want("ui.box.note");
        assert_ne!(note["text"], "", "note: {note}");
        br.conn
            .notify("ui.event", &ev(EV_INPUT_SENT, "other", None))
            .unwrap();
        let set = br.want("ui.input.set");
        assert!(
            set["if"] == "" && set["box"]["text"] == "hello",
            "after send, the stash should come back: {set}"
        );
        br.call(
            "ui.command",
            json!({ "command": "history", "ui": "w", "box": "s1" }),
        )
        .unwrap();
        let p = br.want("ui.pick");
        assert_eq!(p["pick"]["id"], "history", "pick: {p}");
        br.close().unwrap();
        let kept = std::fs::read(data.path().join("drafts.json")).expect("not kept on disk");
        assert!(!kept.is_empty());
    }
}
