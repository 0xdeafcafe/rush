//! A Pi session run through `pi --mode rpc`. Port of
//! internal/adapters/pi/conn.go:1-532.
//!
//! Go queues events for a pump goroutine so the reader never waits on
//! whoever draws them; here the caller's Sink never blocks, so the reader
//! thread hands each event on itself. One lock keeps every thread's events
//! in order and ended last.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rush_core::adapter::{self, Answerer, Conn, Input, Sink, StartOptions};
use rush_core::event::{Command as Cmd, Commands, Context, Event, Init, TurnEnd};
use rush_core::usage::TokenUsage;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::discover::session_file;
use crate::json::nd;
use crate::rpc::{CallError, Client, last_line};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Pi's Model, as much as rush reads of it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(crate) struct Model {
    #[serde(deserialize_with = "nd")]
    pub id: String,
    #[serde(deserialize_with = "nd")]
    pub provider: String,
    #[serde(rename = "contextWindow", deserialize_with = "nd")]
    pub context_window: i64,
}

/// get_state's answer.
#[derive(Default, Deserialize)]
#[serde(default)]
struct State {
    model: Option<Model>,
    #[serde(rename = "sessionId", deserialize_with = "nd")]
    session_id: String,
}

/// What a session keeps between lines.
#[derive(Default)]
pub(crate) struct St {
    pub session: String,
    pub model: Model,
    /// Between agent_start and agent_end.
    pub running: bool,
    /// The running run's start.
    pub began: Option<Instant>,
    /// The assistant message being written.
    pub msg: String,
    /// The run's last stop reason, and its error.
    pub stop: String,
    pub failed: String,
    /// The run's last words.
    pub last: String,
    /// The run's.
    pub tokens: TokenUsage,
    /// The process's, all told.
    pub cost: f64,
    /// Messages sent that pi hasn't said back yet.
    pub sent: Vec<String>,
    /// An extension's select or confirm waiting on the user, by id: whether
    /// it's a confirm.
    pub dialogs: HashMap<String, bool>,
    /// Compactions asked for and not yet done.
    pub compacts: i64,
}

/// What every thread of one session shares.
pub(crate) struct Shared {
    pub st: Mutex<St>,
    out: Mutex<Out>,
    pub rpc: OnceLock<Weak<Client>>,
    /// Closed by rush.
    quit: AtomicBool,
}

struct Out {
    sink: Arc<dyn Sink>,
    ended: bool,
    /// A start that failed tells the sink nothing.
    silent: bool,
}

impl Shared {
    pub(crate) fn new(sink: Arc<dyn Sink>) -> Arc<Shared> {
        Arc::new(Shared {
            st: Mutex::new(St::default()),
            out: Mutex::new(Out {
                sink,
                ended: false,
                silent: false,
            }),
            rpc: OnceLock::new(),
            quit: AtomicBool::new(false),
        })
    }

    /// Hands events to the sink, in order, until it has ended.
    pub(crate) fn emit(&self, evs: Vec<Event>) {
        let o = lock(&self.out);
        if o.ended || o.silent {
            return;
        }
        for e in evs {
            o.sink.event(e);
        }
    }

    fn end(&self, err: Option<adapter::Error>) {
        let mut o = lock(&self.out);
        if o.ended {
            return;
        }
        o.ended = true;
        if !o.silent {
            o.sink.ended(err);
        }
    }

    pub(crate) fn client(&self) -> Option<Arc<Client>> {
        self.rpc.get().and_then(Weak::upgrade)
    }
}

/// A session id pi takes with --session-id:
/// ^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$
fn valid_id(s: &str) -> bool {
    let b = s.as_bytes();
    let alnum = |c: &u8| c.is_ascii_alphanumeric();
    match b {
        [] => false,
        [c] => alnum(c),
        [first, mid @ .., last] => {
            alnum(first)
                && alnum(last)
                && mid
                    .iter()
                    .all(|c| alnum(c) || matches!(c, b'.' | b'_' | b'-'))
        }
    }
}

/// An error in words.
#[derive(Debug)]
pub(crate) struct Says(pub String);

impl std::fmt::Display for Says {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Says {}

pub(crate) fn says(s: impl Into<String>) -> adapter::Error {
    Box::new(Says(s.into()))
}

/// pi's command line for o.
pub(crate) fn args_for(o: &StartOptions) -> adapter::Result<Vec<String>> {
    let mut args: Vec<String> = vec!["--mode".into(), "rpc".into()];
    if o.fork || o.resume {
        if o.session_id.is_empty() {
            return Err(says("pi: no session to resume"));
        }
        let path = session_file(&o.profile, &o.session_id);
        if path.is_empty() {
            return Err(says(format!(
                "pi: no session {} in {}",
                o.session_id, o.profile.dir
            )));
        }
        args.push(if o.fork { "--fork" } else { "--session" }.into());
        args.push(path);
    } else if !o.session_id.is_empty() && valid_id(&o.session_id) {
        args.push("--session-id".into());
        args.push(o.session_id.clone());
    }
    if !o.model.is_empty() {
        args.extend(["--model".into(), o.model.clone()]);
    }
    if !o.effort.is_empty() {
        args.extend(["--thinking".into(), o.effort.clone()]);
    }
    let p = o.prompt.trim();
    if !p.is_empty() {
        args.extend(["--append-system-prompt".into(), p.to_string()]);
    }
    args.extend(o.flags.iter().cloned());
    Ok(args)
}

/// How long pi has to load its extensions and say it's ready.
const START_FOR: Duration = Duration::from_secs(60);
/// How long a command pi answers at once has.
const CALL_FOR: Duration = Duration::from_secs(30);
/// How long a compaction has: the model summarises the whole conversation.
const COMPACT_FOR: Duration = Duration::from_secs(10 * 60);

/// A Pi session.
pub struct PiConn {
    sh: Arc<Shared>,
    rpc: Arc<Client>,
}

fn cmd(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The sink's end, once pi has gone: why, as it last said on stderr, when
/// it went by itself.
fn done(sh: Arc<Shared>) -> crate::rpc::Done {
    Box::new(move |c: &Client| {
        let err = if sh.quit.load(Ordering::SeqCst) {
            None
        } else {
            let said = c.said();
            (!said.is_empty()).then(|| says(format!("pi: {}", last_line(&said))))
        };
        sh.end(err);
    })
}

/// Starts, resumes or forks a session: PI_CODING_AGENT_DIR is the
/// profile's folder, the model is a pattern pi matches ("provider/id" or
/// an id), and effort is its thinking level. Pi has no modes; o.mode is
/// let be.
pub(crate) fn start(o: &StartOptions, path: &str, sink: Arc<dyn Sink>) -> adapter::Result<PiConn> {
    let args = args_for(o)?;
    let binary = if !o.binary.is_empty() {
        o.binary.clone()
    } else if !path.is_empty() {
        path.to_string()
    } else {
        "pi".into()
    };
    let mut c = std::process::Command::new(binary);
    c.args(&args).env("PI_SKIP_VERSION_CHECK", "1");
    if !o.dir.is_empty() {
        c.current_dir(&o.dir);
    }
    if !o.profile.dir.is_empty() {
        c.env("PI_CODING_AGENT_DIR", &o.profile.dir);
    }
    if o.lean {
        c.env("PI_OFFLINE", "1");
    }
    if !o.temp_dir.is_empty() && mkdir_private(&o.temp_dir) {
        c.env("TMPDIR", &o.temp_dir);
    }
    for kv in &o.env {
        if let Some((k, v)) = kv.split_once('=') {
            c.env(k, v);
        }
    }
    let sh = Shared::new(sink);
    let h = sh.clone();
    let rpc = Client::spawn(
        &mut c,
        Box::new(move |t, b| h.handle(t, b)),
        done(sh.clone()),
    )?;
    let _ = sh.rpc.set(Arc::downgrade(&rpc));
    let conn = PiConn { sh, rpc };
    if let Err(e) = conn.begin(o) {
        lock(&conn.sh.out).silent = true;
        let _ = conn.close();
        return Err(e);
    }
    Ok(conn)
}

fn mkdir_private(d: &str) -> bool {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(d)
        .is_ok()
}

impl PiConn {
    /// A Conn on a client already talking: for tests.
    #[cfg(test)]
    pub(crate) fn on(sh: Arc<Shared>, rpc: Arc<Client>) -> PiConn {
        let _ = sh.rpc.set(Arc::downgrade(&rpc));
        PiConn { sh, rpc }
    }

    #[cfg(test)]
    pub(crate) fn done_for(sh: &Arc<Shared>) -> crate::rpc::Done {
        done(sh.clone())
    }

    /// Asks pi where it stands, which is also how rush knows it's up.
    pub(crate) fn begin(&self, o: &StartOptions) -> adapter::Result<()> {
        let st: State = match self
            .rpc
            .call_into(&cmd(json!({"type": "get_state"})), START_FOR)
        {
            Ok(s) => s.unwrap_or_default(),
            Err(CallError::Closed) if !self.rpc.said().is_empty() => {
                return Err(says(format!("pi: {}", last_line(&self.rpc.said()))));
            }
            Err(e) => return Err(Box::new(e)),
        };
        let (m, window) = {
            let mut s = lock(&self.sh.st);
            s.session = st.session_id.clone();
            if let Some(m) = st.model {
                s.model = m;
            }
            (s.model.id.clone(), s.model.context_window)
        };
        let mut evs = vec![Event::Init(Box::new(Init {
            session_id: st.session_id,
            model: m,
            cwd: o.dir.clone(),
            ..Init::default()
        }))];
        if window > 0 {
            evs.push(Event::Context(Context {
                window,
                ..Context::default()
            }));
        }
        self.sh.emit(evs);
        let (sh, rpc) = (self.sh.clone(), self.rpc.clone());
        std::thread::spawn(move || read_commands(&sh, &rpc));
        Ok(())
    }
}

/// Lists the session's commands once: prompt templates, skills and
/// extensions' commands, all sent as a prompt starting with /.
fn read_commands(sh: &Shared, rpc: &Client) {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Res {
        #[serde(deserialize_with = "nd")]
        commands: Vec<C>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct C {
        #[serde(deserialize_with = "nd")]
        name: String,
        #[serde(deserialize_with = "nd")]
        description: String,
    }
    let Ok(Some(res)) = rpc.call_into::<Res>(&cmd(json!({"type": "get_commands"})), CALL_FOR)
    else {
        return;
    };
    if res.commands.is_empty() {
        return;
    }
    let list = res
        .commands
        .into_iter()
        .map(|c| Cmd {
            name: c.name,
            description: c.description,
            ..Cmd::default()
        })
        .collect();
    sh.emit(vec![Event::Commands(Commands { list })]);
}

/// Whether text is /compact, and what's after it.
pub(crate) fn compact_command(text: &str) -> Option<String> {
    let t = text.trim();
    if t != "/compact" && !t.starts_with("/compact ") {
        return None;
    }
    Some(t["/compact".len()..].trim().to_string())
}

/// Compacts the conversation, and ends the turn /compact began when
/// nothing else is running.
fn compact(sh: &Shared, rpc: &Client, instructions: &str) {
    let mut c = cmd(json!({"type": "compact"}));
    if !instructions.is_empty() {
        c.insert("customInstructions".into(), instructions.into());
    }
    let r = rpc.call(&c, COMPACT_FOR);
    let (running, cost) = {
        let mut s = lock(&sh.st);
        s.compacts -= 1;
        (s.running, s.cost)
    };
    if running {
        return;
    }
    let mut end = TurnEnd {
        reason: "done".into(),
        cost,
        ..TurnEnd::default()
    };
    if let Err(e) = r {
        end.reason = "error".into();
        end.err = match e {
            CallError::Refused { message, .. } => message,
            e => e.to_string(),
        };
    }
    sh.emit(vec![Event::TurnEnd(end)]);
}

/// Image files as pi takes them: base64, with their type.
fn images(paths: &[String]) -> adapter::Result<Vec<Value>> {
    let mut out = Vec::new();
    for p in paths {
        let b = std::fs::read(p)?;
        out.push(json!({"type": "image", "data": STANDARD.encode(&b), "mimeType": mime_of(p, &b)}));
    }
    Ok(out)
}

/// An image's type, by its name, else by its first bytes (Go's
/// http.DetectContentType, for the types an image is).
pub(crate) fn mime_of(path: &str, b: &[u8]) -> String {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => return "image/png".into(),
        "jpg" | "jpeg" => return "image/jpeg".into(),
        "gif" => return "image/gif".into(),
        "webp" => return "image/webp".into(),
        _ => {}
    }
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if b.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        "image/gif"
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp"
    } else if b.starts_with(b"BM") {
        "image/bmp"
    } else {
        "application/octet-stream"
    }
    .into()
}

/// The model name means: "provider/id" exactly, else an id, the
/// provider's own first.
pub(crate) fn find_model(models: &[Model], name: &str, provider: &str) -> Option<Model> {
    if let Some(m) = models
        .iter()
        .find(|m| format!("{}/{}", m.provider, m.id) == name)
    {
        return Some(m.clone());
    }
    let ids: Vec<&Model> = models.iter().filter(|m| m.id == name).collect();
    ids.iter()
        .find(|m| m.provider == provider)
        .or(ids.first())
        .map(|m| (*m).clone())
}

/// A confirm's choices.
pub(crate) const CONFIRM_YES: &str = "Yes";
pub(crate) const CONFIRM_NO: &str = "No";

impl Conn for PiConn {
    /// A prompt when pi is idle, else one it steers the running work with,
    /// after the tools it's running. /compact compacts.
    fn send(&self, input: Input) -> adapter::Result<()> {
        if let Some(rest) = compact_command(&input.text) {
            lock(&self.sh.st).compacts += 1;
            let (sh, rpc) = (self.sh.clone(), self.rpc.clone());
            std::thread::spawn(move || compact(&sh, &rpc, &rest));
            return Ok(());
        }
        let mut c = cmd(json!({"type": "prompt", "message": input.text}));
        if !input.images.is_empty() {
            c.insert("images".into(), Value::Array(images(&input.images)?));
        }
        let busy = {
            let mut s = lock(&self.sh.st);
            s.sent.push(input.text.clone());
            s.running
        };
        if busy {
            c.insert("streamingBehavior".into(), "steer".into());
        }
        let mut r = self.rpc.call(&c, CALL_FOR);
        if let Err(CallError::Refused { message, .. }) = &r
            && !busy
            && message.contains("already processing")
        {
            // It started on something of its own meanwhile.
            c.insert("streamingBehavior".into(), "steer".into());
            r = self.rpc.call(&c, CALL_FOR);
        }
        if let Err(e) = r {
            let mut s = lock(&self.sh.st);
            if s.sent.last() == Some(&input.text) {
                s.sent.pop();
            }
            return Err(Box::new(e));
        }
        Ok(())
    }

    /// Pi asks no approvals; an extension's questions are answered with
    /// answer_question.
    fn answer(&self, approval_id: &str, _option_id: &str) -> adapter::Result<()> {
        Err(says(format!(
            "pi: nothing is waiting on {approval_id}: Pi asks no approvals"
        )))
    }

    /// Aborts what pi is doing. Pi answers once it has stopped, so the
    /// answer isn't waited for.
    fn interrupt(&self) -> adapter::Result<()> {
        Ok(self.rpc.post(&cmd(json!({"type": "abort"})))?)
    }

    /// Switches model at once: "provider/id", or an id, looked for first
    /// with the provider in use.
    fn set_model(&self, name: &str) -> adapter::Result<()> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Res {
            #[serde(deserialize_with = "nd")]
            models: Vec<Model>,
        }
        let res: Res = self
            .rpc
            .call_into(&cmd(json!({"type": "get_available_models"})), CALL_FOR)?
            .unwrap_or_default();
        let provider = lock(&self.sh.st).model.provider.clone();
        let Some(m) = find_model(&res.models, name, &provider) else {
            return Err(says(format!("pi: no model {name:?}")));
        };
        let set_cmd = cmd(json!({"type": "set_model", "provider": m.provider, "modelId": m.id}));
        let mut set: Model = self.rpc.call_into(&set_cmd, CALL_FOR)?.unwrap_or_default();
        if set.id.is_empty() {
            set = m;
        }
        let window = set.context_window;
        lock(&self.sh.st).model = set;
        if window > 0 {
            self.sh.emit(vec![Event::Context(Context {
                window,
                ..Context::default()
            })]);
        }
        Ok(())
    }

    /// Pi has no permission modes.
    fn set_mode(&self, _mode: &str) -> adapter::Result<()> {
        Err(says(
            "pi has no permission modes: it runs every tool without asking",
        ))
    }

    /// Ends pi and the session with it.
    fn close(&self) -> adapter::Result<()> {
        if self.sh.quit.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.rpc.close();
        Ok(())
    }

    fn answerer(&self) -> Option<&dyn Answerer> {
        Some(self)
    }

    fn pid(&self) -> Option<i32> {
        (self.rpc.pid != 0).then_some(self.rpc.pid)
    }
}

impl Answerer for PiConn {
    /// Answers an extension's select or confirm with the label chosen; none
    /// cancels it.
    fn answer_question(
        &self,
        id: &str,
        answers: &HashMap<String, Vec<String>>,
    ) -> adapter::Result<()> {
        let Some(confirm) = lock(&self.sh.st).dialogs.remove(id) else {
            return Err(says(format!("pi: nothing is waiting on {id}")));
        };
        let chosen = answers
            .values()
            .find_map(|v| v.first().cloned())
            .unwrap_or_default();
        let mut reply = cmd(json!({"type": "extension_ui_response", "id": id}));
        if chosen.is_empty() {
            reply.insert("cancelled".into(), true.into());
        } else if confirm {
            reply.insert("confirmed".into(), (chosen == CONFIRM_YES).into());
        } else {
            reply.insert("value".into(), chosen.into());
        }
        Ok(self.rpc.post(&reply)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_pi_takes() {
        for (id, ok) in [
            ("new-one", true),
            ("a", true),
            ("a.b_c-d", true),
            ("-a", false),
            ("a-", false),
            ("", false),
            ("a/b", false),
        ] {
            assert_eq!(valid_id(id), ok, "{id}");
        }
    }

    #[test]
    fn compact_commands() {
        assert_eq!(
            compact_command(" /compact keep the tests "),
            Some("keep the tests".into())
        );
        assert_eq!(compact_command("/compact"), Some(String::new()));
        assert_eq!(compact_command("/compactly"), None);
    }

    #[test]
    fn models_by_name() {
        let ms = vec![
            Model {
                id: "q".into(),
                provider: "other".into(),
                ..Model::default()
            },
            Model {
                id: "q".into(),
                provider: "ollama".into(),
                ..Model::default()
            },
        ];
        assert_eq!(find_model(&ms, "q", "ollama").unwrap().provider, "ollama");
        assert_eq!(find_model(&ms, "q", "x").unwrap().provider, "other");
        assert_eq!(
            find_model(&ms, "other/q", "ollama").unwrap().provider,
            "other"
        );
        assert!(find_model(&ms, "z", "").is_none());
        assert_eq!(mime_of("a.PNG", b""), "image/png");
        assert_eq!(mime_of("a", b"GIF89a.."), "image/gif");
    }
}
