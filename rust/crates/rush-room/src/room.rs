//! A room is a group chat of fresh agents on one topic: they take turns,
//! see everything said, argue, and end on a verdict for you. A driver
//! process (rush room run) runs the turns through ordinary rush sessions;
//! the room is one append-only log any client tails and posts to. See
//! docs/room.md. Same files as Go rush: rush's folder's rooms/<id>/
//! room.json, log.jsonl and driver.pid.
//! Ported from internal/room/room.go:1-398.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use rush_core::tool::Call;
use rush_state::Time;
use rush_state::json::RawJson;
use serde::{Deserialize, Serialize};

use crate::Agents;

/// Who you are in a room's log.
pub const USER: &str = "user";

/// Entry kinds.
pub const TURN: &str = "turn"; // From starts speaking
pub const SAY: &str = "say"; // a message: an agent's turn, or yours
pub const NOTE: &str = "note"; // an agent's words on the way, between tool calls
pub const TOOL: &str = "tool"; // a tool call, by ID; a later one with the same ID replaces it
pub const DONE: &str = "done"; // tool call ID came back (Failed when it failed)
pub const VERDICT: &str = "verdict"; // the room's answer: each member's final position, a line each; Detail says why it ended
pub const CONTROL: &str = "control"; // yours: Text is stop, verdict, pause or resume
pub const PAUSED: &str = "paused"; // From's turn was cut short by a pause
pub const USES: &str = "model"; // From runs model Text: its default, as its session says
pub const JOINED: &str = "joined"; // From's session started: ID is its rush session id
pub const INFO: &str = "info"; // the room's own notices
pub const END: &str = "end"; // the room is over; Text says why

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// One line of a room's log.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub at: Time,
    pub from: String,
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// A tool call's command or path.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// A message of yours @someone.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    /// A tool call's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "is_false")]
    pub failed: bool,
    /// A turn's READY: yes or no.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ready: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub round: i64,
    /// A tool call as made, for the pane to draw as a step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<Call>,
}

/// One agent on the panel. Its config is the session config (host.Config)
/// it starts with, kept as written: the host module reads and writes it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub config: RawJson,
    /// Its model's context window in tokens, when known: a small one keeps
    /// prompts short.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub context: i64,
}

impl Default for Member {
    fn default() -> Self {
        Member {
            name: String::new(),
            config: RawJson::from_value(&serde_json::json!({})).expect("{} is JSON"),
            context: 0,
        }
    }
}

impl PartialEq for Member {
    fn eq(&self, o: &Member) -> bool {
        // The same config, however it was laid out.
        let v = |r: &RawJson| serde_json::from_str::<serde_json::Value>(r.get()).ok();
        self.name == o.name && v(&self.config) == v(&o.config) && self.context == o.context
    }
}

/// What of a member's config the room reads.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ConfigView {
    kind: String,
    model: String,
}

impl Member {
    fn view(&self) -> ConfigView {
        rush_state::json::from_slice(self.config.get().as_bytes()).unwrap_or_default()
    }

    /// The agent it runs, as its config has it (an empty one is Claude
    /// Code's, as an older rush wrote it).
    pub fn kind(&self) -> String {
        self.view().kind
    }

    /// The model it starts on; "" for its agent's default.
    pub fn model(&self) -> String {
        self.view().model
    }

    /// The member's agent in words: "Opus 5.5 · Claude Code".
    pub fn label(&self, agents: &dyn Agents) -> String {
        let v = self.view();
        if v.model.is_empty() {
            return agents.harness_label(&v.kind);
        }
        format!(
            "{} · {}",
            agents.model_name(&v.kind, &v.model),
            agents.harness_label(&v.kind)
        )
    }
}

/// How a room was opened.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Room {
    pub id: String,
    pub topic: String,
    pub cwd: String,
    /// The most rounds before a verdict is forced.
    pub rounds: i64,
    #[serde(with = "null_vec")]
    pub members: Vec<Member>,
    pub created: Time,
    /// The chat the room was opened from, by its agent key: the room shows
    /// in that chat and its verdict goes back there.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    /// What of that chat every member reads with the topic.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub brief: String,
}

mod null_vec {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
        if v.is_empty() {
            return s.serialize_none(); // Go's nil slice
        }
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Vec<T>, D::Error> {
        Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
    }
}

/// Caps a room that never converges.
pub const DEFAULT_ROUNDS: i64 = 4;

/// Where rooms are kept, under rush's folder home.
#[derive(Clone, Debug)]
pub struct Rooms {
    root: PathBuf,
}

impl Default for Rooms {
    fn default() -> Self {
        Rooms::new()
    }
}

impl Rooms {
    /// The rooms under rush's folder.
    pub fn new() -> Rooms {
        Rooms::at(rush_state::dir())
    }

    /// The rooms under rush's folder home.
    pub fn at(home: impl AsRef<Path>) -> Rooms {
        Rooms {
            root: home.as_ref().join("rooms"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn log_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("log.jsonl")
    }

    pub fn pid_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("driver.pid")
    }

    /// Keeps r as a new room, its id filled in (new_id makes one when it
    /// has none: host.NewSessionID's).
    pub fn create(&self, mut r: Room, new_id: impl FnOnce() -> String) -> Result<Room, String> {
        if r.topic.trim().is_empty() {
            return Err("a room needs a topic".into());
        }
        if r.members.len() < 2 {
            return Err("a room needs at least two agents".into());
        }
        if r.rounds <= 0 {
            r.rounds = DEFAULT_ROUNDS;
        }
        if r.id.is_empty() {
            r.id = new_id();
        }
        r.created = Time::now();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(self.dir(&r.id))
            .map_err(|e| e.to_string())?;
        let b = rush_state::json::to_vec_indent(&r).map_err(|e| e.to_string())?;
        write_file(&self.dir(&r.id).join("room.json"), &b).map_err(|e| e.to_string())?;
        Ok(r)
    }

    /// Reads room id.
    pub fn load(&self, id: &str) -> Result<Room, String> {
        let no = || format!("no room {}", go_quote(id));
        if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
            return Err(no());
        }
        let b = std::fs::read(self.dir(id).join("room.json")).map_err(|_| no())?;
        rush_state::json::from_slice(&b).map_err(|e| e.to_string())
    }

    /// Every room, newest first.
    pub fn list(&self) -> Vec<Room> {
        let mut out: Vec<Room> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| self.load(&e.file_name().to_string_lossy()).ok())
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.created));
        out
    }

    /// Runs the room's driver as a process of its own, exe room run id, so
    /// closing rush leaves the room going.
    pub fn start(&self, exe: &Path, id: &str) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.dir(id).join("driver.log"))
            .map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["room", "run", id])
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log);
        // SAFETY: setsid only, between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        // Reaped, or it lingers as a zombie that looks alive.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    /// Whether room id's driver is still going.
    pub fn running(&self, id: &str) -> bool {
        let Ok(b) = std::fs::read_to_string(self.pid_path(id)) else {
            return false;
        };
        let pid: i32 = b.trim().parse().unwrap_or(0);
        pid > 0 && alive(pid)
    }

    /// Adds e to room id's log.
    /// ponytail: driver and clients append to one file; each line is a
    /// single O_APPEND write, which keeps lines whole on a local disk. A
    /// socket if rooms ever span machines.
    pub fn post(&self, id: &str, mut e: Entry) -> io::Result<()> {
        if e.at.is_zero() {
            e.at = Time::now();
        }
        let mut b = rush_state::json::to_vec(&e).map_err(io::Error::other)?;
        b.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.log_path(id))?;
        f.write_all(&b)
    }

    /// Room id's log from byte off: the whole lines there, and the offset
    /// after them.
    pub fn read(&self, id: &str, off: u64) -> io::Result<(Vec<Entry>, u64)> {
        let mut f = match File::open(self.log_path(id)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), off)),
            Err(e) => return Err(e),
        };
        f.seek(SeekFrom::Start(off))?;
        let mut r = BufReader::with_capacity(64 << 10, f);
        let mut out = Vec::new();
        let mut off = off;
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = r.read_until(b'\n', &mut line)?;
            if n == 0 || line.last() != Some(&b'\n') {
                break; // a partial last line waits for its newline
            }
            off += n as u64;
            if let Ok(e) = rush_state::json::from_slice::<Entry>(&line) {
                out.push(e);
            }
        }
        Ok((out, off))
    }
}

/// Whether pid is a running process: a zombie isn't.
pub(crate) fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let ok = match rush_sys::proc::kill(pid, 0) {
        Ok(()) => true,
        Err(rush_sys::proc::KillError::Os(e)) => e == libc::EPERM,
        Err(_) => false,
    };
    ok && !rush_sys::proc::zombie(pid)
}

fn write_file(path: &Path, b: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(b)
}

/// s quoted as Go's %q.
pub(crate) fn go_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// Gives each member a short name to be called by: the model's first word
/// ("Opus"), else the harness's ("Codex"), numbered when taken.
pub fn name(agents: &dyn Agents, kind: &str, model: &str, taken: &[String]) -> String {
    let mut base = agents.harness_label(kind);
    let n = agents.model_name(kind, model);
    if n != model {
        base = n; // the adapter's name for it: "Opus 5.5"
    } else {
        let f: Vec<&str> = model
            .split(['-', '_', '.', '/', ':'])
            .filter(|f| !f.is_empty())
            .collect();
        if let Some(last) = f.last()
            && last.len() >= 3
            && last.bytes().all(|c| c.is_ascii_lowercase())
        {
            base = last[..1].to_uppercase() + &last[1..]; // "gpt-6-luna" is Luna; "gpt-5.5" stays the harness's
        }
    }
    let base = format!("{base} Agent");
    let base = base.split_whitespace().next().unwrap_or("Agent");
    let base = base.trim_matches(['@', ':', ',', '.']).to_string();
    let mut name = base.clone();
    let mut n = 2;
    while taken
        .iter()
        .any(|t| t.to_lowercase() == name.to_lowercase())
    {
        name = format!("{base}{n}");
        n += 1;
    }
    name
}

/// A message of yours as you'd type it: "@codex why?" is to Codex; /stop,
/// /verdict, /pause and /resume are controls.
pub fn parse(text: &str, members: &[Member]) -> Entry {
    let text = text.trim();
    let lower = text.to_lowercase();
    if matches!(lower.as_str(), "/stop" | "/verdict" | "/pause" | "/resume") {
        return Entry {
            from: USER.into(),
            kind: CONTROL.into(),
            text: lower[1..].to_string(),
            ..Entry::default()
        };
    }
    let mut e = Entry {
        from: USER.into(),
        kind: SAY.into(),
        text: text.to_string(),
        ..Entry::default()
    };
    if let Some(at) = text.strip_prefix('@') {
        let word = at.split(' ').next().unwrap_or("");
        let word = word.trim_end_matches([',', ':']);
        for m in members {
            if m.name.to_lowercase() == word.to_lowercase() {
                e.to = m.name.clone();
            }
        }
    }
    e
}

fn ready_line() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)[*_`]*(?-u:\b)READY[*_`\t\n\f\r ]*:[*_`\t\n\f\r ]*(yes|no)(?-u:\b)[*_`.]*")
            .expect("a fixed pattern")
    })
}

/// Takes the last READY: yes or no off a turn: the text without it, and
/// "yes", "no" or "" when it had none.
pub fn ready(text: &str) -> (String, String) {
    let Some(c) = ready_line().captures_iter(text).last() else {
        return (text.trim().to_string(), String::new());
    };
    let all = c.get(0).expect("a match");
    let word = c.get(1).expect("its word");
    let rest = format!("{}{}", &text[..all.start()], &text[all.end()..]);
    (rest.trim().to_string(), word.as_str().to_lowercase())
}

/// A room as a list row shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub room: Room,
    /// rush session id: member name.
    pub sessions: HashMap<String, String>,
    pub running: bool,
    pub paused: bool,
    /// Whose turns are under way.
    pub speaking: Vec<String>,
    /// Why it ended, once it has.
    pub over: String,
    pub verdict: bool,
    /// The verdict: each member's final position, a line each.
    pub final_: String,
    /// The round under way, or the last one.
    pub round: i64,
    /// A member: the first line of what it said last.
    pub last: HashMap<String, String>,
    pub updated: Time,
}

/// Lists rooms again and again, reading only the logs that grew.
#[derive(Default)]
pub struct Lister {
    seen: HashMap<String, (u64, Summary)>,
}

impl Lister {
    /// Every room, newest first.
    pub fn list(&mut self, rooms: &Rooms) -> Vec<Summary> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(rooms.root())
            .into_iter()
            .flatten()
            .flatten()
        {
            let id = e.file_name().to_string_lossy().into_owned();
            let Ok(st) = std::fs::metadata(rooms.log_path(&id)) else {
                continue;
            };
            let fresh = self
                .seen
                .get(&id)
                .is_some_and(|(size, _)| *size == st.len());
            if !fresh {
                let Ok(r) = rooms.load(&id) else {
                    continue;
                };
                let es = rooms.read(&id, 0).map(|(es, _)| es).unwrap_or_default();
                self.seen.insert(id.clone(), (st.len(), summarise(&r, &es)));
            }
            let mut s = self.seen[&id].1.clone();
            s.running = s.over.is_empty() && rooms.running(&id);
            out.push(s);
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.room.created));
        out
    }
}

/// Room r as its log es says it stands.
pub fn summarise(r: &Room, es: &[Entry]) -> Summary {
    let mut s = Summary {
        room: r.clone(),
        updated: r.created,
        ..Summary::default()
    };
    let mut speaking: HashMap<&str, bool> = HashMap::new();
    for e in es {
        s.updated = e.at;
        match e.kind.as_str() {
            JOINED => {
                s.sessions.insert(e.id.clone(), e.from.clone());
            }
            TURN => {
                speaking.insert(&e.from, true);
                s.round = s.round.max(e.round);
            }
            NOTE => {
                s.last.insert(e.from.clone(), first_line(&e.text));
            }
            SAY | PAUSED => {
                speaking.insert(&e.from, false);
                if e.kind == SAY && e.from != USER {
                    s.last.insert(e.from.clone(), first_line(&e.text));
                }
            }
            CONTROL => match e.text.as_str() {
                "pause" => s.paused = true,
                "resume" | "verdict" | "stop" => s.paused = false,
                _ => {}
            },
            VERDICT => {
                s.verdict = true;
                s.final_ = e.text.clone();
            }
            END => s.over = e.text.clone(),
            _ => {}
        }
    }
    for m in &r.members {
        if speaking.get(m.name.as_str()).copied().unwrap_or(false) && !s.paused && s.over.is_empty()
        {
            s.speaking.push(m.name.clone());
        }
    }
    s
}

/// s's first line with words on it.
fn first_line(s: &str) -> String {
    s.split_inclusive('\n')
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::NoAgents;

    // room_test.go TestReady
    #[test]
    fn ready_lines() {
        for (input, text, r) in [
            ("I agree.\n\nREADY: yes", "I agree.", "yes"),
            ("Reliable enough. READY: no", "Reliable enough.", "no"),
            ("Done.\n**READY:** yes", "Done.", "yes"),
            ("No marker here", "No marker here", ""),
            (
                "READY: no at first\nthen READY: yes",
                "READY: no at first\nthen",
                "yes",
            ),
        ] {
            assert_eq!(ready(input), (text.to_string(), r.to_string()), "{input:?}");
        }
    }

    fn member(name: &str) -> Member {
        Member {
            name: name.into(),
            ..Member::default()
        }
    }

    // room_test.go TestParse
    #[test]
    fn parses_what_you_say() {
        let ms = [member("Opus"), member("Codex")];
        let e = parse("@codex, why?", &ms);
        assert_eq!((e.to.as_str(), e.kind.as_str()), ("Codex", SAY));
        assert_eq!(parse("@nobody hi", &ms).to, "");
        for (input, want) in [
            (" /Verdict ", "verdict"),
            ("/pause", "pause"),
            ("/resume", "resume"),
            ("/stop", "stop"),
        ] {
            let e = parse(input, &ms);
            assert_eq!(
                (e.kind.as_str(), e.text.as_str()),
                (CONTROL, want),
                "{input:?}"
            );
        }
    }

    // room_test.go TestName: no adapters are registered here: the
    // harness's name is the kind's.
    #[test]
    fn names_members() {
        for (kind, model, want) in [
            ("codex", "gpt-6-luna", "Luna"),
            ("codex", "gpt-5.5", "codex"),
            ("codex", "", "codex"),
        ] {
            assert_eq!(name(&NoAgents, kind, model, &[]), want, "{kind} {model}");
        }
        assert_eq!(
            name(&NoAgents, "codex", "gpt-6-luna", &["luna".into()]),
            "Luna2"
        );
    }

    #[test]
    fn rooms_on_disk() {
        let home = tempfile::tempdir().unwrap();
        let rooms = Rooms::at(home.path());
        assert!(rooms.create(Room::default(), || "x".into()).is_err());
        let cfg =
            |kind: &str| RawJson::from_value(&serde_json::json!({"id": "", "kind": kind})).unwrap();
        let r = Room {
            topic: "Tabs or spaces".into(),
            cwd: "/w".into(),
            members: vec![
                Member {
                    name: "Opus".into(),
                    config: cfg("claude"),
                    context: 0,
                },
                Member {
                    name: "Codex".into(),
                    config: cfg("codex"),
                    context: 200000,
                },
            ],
            ..Room::default()
        };
        let r = rooms.create(r, || "r1".into()).unwrap();
        assert_eq!((r.id.as_str(), r.rounds), ("r1", DEFAULT_ROUNDS));
        let back = rooms.load("r1").unwrap();
        assert_eq!(back, r);
        assert_eq!(back.members[1].kind(), "codex");
        assert_eq!(rooms.load("../x").unwrap_err(), "no room \"../x\"");
        assert_eq!(rooms.list().len(), 1);
        rooms
            .post(
                "r1",
                Entry {
                    from: "Opus".into(),
                    kind: TURN.into(),
                    round: 1,
                    ..Entry::default()
                },
            )
            .unwrap();
        // A partial last line waits for its newline.
        let mut f = OpenOptions::new()
            .append(true)
            .open(rooms.log_path("r1"))
            .unwrap();
        f.write_all(b"{\"from\":\"Codex\"").unwrap();
        let (es, off) = rooms.read("r1", 0).unwrap();
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].round, 1);
        f.write_all(b",\"kind\":\"say\",\"text\":\"Spaces.\"}\n")
            .unwrap();
        let (es, _) = rooms.read("r1", off).unwrap();
        assert_eq!(
            (es[0].from.as_str(), es[0].text.as_str()),
            ("Codex", "Spaces.")
        );
        assert!(!rooms.running("r1"));
        let mut l = Lister::default();
        let s = l.list(&rooms);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].speaking, ["Opus"]);
        assert_eq!(s[0].last["Codex"], "Spaces.");
    }

    // An entry as Go writes it reads and writes back the same.
    #[test]
    fn entries_as_go_writes_them() {
        let go = r#"{"at":"2026-10-29T10:00:00.5+01:00","from":"Opus","kind":"tool","text":"running go test","detail":"go test ./...","id":"c1","failed":true,"round":2,"call":{"ID":"c1","Name":"Bash","Kind":1,"Title":"","Input":{"Command":"go test ./...","Cwd":"","Background":false,"Timeout":0,"Path":"","To":"","Pattern":"","Query":"","URL":"","Description":"","Prompt":"","Agent":"","Child":"","Content":"","Edits":null,"Offset":0,"Limit":0,"Server":"","Tool":"","Todos":null}}}"#;
        let e: Entry = rush_state::json::from_slice(go.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(rush_state::json::to_vec(&e).unwrap()).unwrap(),
            go
        );
        let s = summarise(
            &Room::default(),
            &[
                Entry {
                    kind: CONTROL.into(),
                    text: "pause".into(),
                    ..Entry::default()
                },
                Entry {
                    kind: VERDICT.into(),
                    text: "Opus: tabs".into(),
                    ..Entry::default()
                },
                Entry {
                    kind: END.into(),
                    text: "stopped by you".into(),
                    ..Entry::default()
                },
            ],
        );
        assert!(s.paused && s.verdict);
        assert_eq!(
            (s.final_.as_str(), s.over.as_str()),
            ("Opus: tabs", "stopped by you")
        );
    }
}
