//! The driver of a room (rush room run): it starts the panel, runs turns
//! until the room converges, hits its cap or you end it, and posts each
//! member's final position. Members are ordinary rush sessions, reached
//! through [`Sessions`]: the host module starts and dials them, and hands
//! their events in as [`Msg`]s.
//! Ported from internal/room/run.go:1-738.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use rush_core::event::{Event, PartKind};
use rush_core::tool::{Call, Kind};
use serde_json::{Map, Value, json};

use crate::Agents;
use crate::doing::doing;
use crate::room::{
    CONTROL, DONE, END, Entry, INFO, JOINED, Member, NOTE, PAUSED, Room, Rooms, SAY, TOOL, TURN,
    USER, USES, VERDICT, ready,
};

/// The longest one turn may run before it's stopped.
const TURN_LIMIT: Duration = Duration::from_secs(10 * 60);

/// Caps what one turn's prompt carries of the room, in bytes; a member
/// with a small context window caps it lower (Member::context).
const PROMPT_BUDGET: usize = 60_000;

/// How often a turn's time is looked at.
const TICK: Duration = Duration::from_secs(10);

/// What a session's info says, as far as the room reads it
/// (host.InfoEvent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionInfo {
    pub kind: String,
    /// The model it runs.
    pub model: String,
    /// working, idle, stopped, …
    pub state: String,
    pub retry: Option<RetryInfo>,
    /// How many background tasks it has going.
    pub background: usize,
    /// How many messages wait for it.
    pub queue: usize,
    /// Whether a usage limit stopped it.
    pub limited: bool,
}

/// A session retrying an API error.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetryInfo {
    pub gave_up: bool,
    pub last_step: String,
    pub attempt: i64,
}

/// One thing a member's session said.
#[derive(Clone, Debug, PartialEq)]
pub enum MemberEvent {
    /// Its agent's event, neutral already (agent.Neutral's).
    Event(Box<Event>),
    /// Its session's info, sent on connect and whenever it changes.
    Info(SessionInfo),
    /// An error its host reports (host.ErrorEvent).
    Error(String),
}

/// What the driver hears: what you post to the room, and its members'
/// sessions' events, a batch at a time; closed once a session's lines
/// end.
#[derive(Debug)]
pub enum Msg {
    User(Box<Entry>),
    Member {
        i: usize,
        evs: Vec<MemberEvent>,
        closed: bool,
    },
}

/// A member's session, dialled.
pub trait Session: Send {
    fn send(&self, text: &str) -> Result<(), String>;
    /// Hands text into the turn under way, for an agent that takes it.
    fn send_guide(&self, text: &str) -> Result<(), String>;
    fn interrupt(&self) -> Result<(), String>;
    fn allow(&self, id: &str) -> Result<(), String>;
    fn deny(&self, id: &str, why: &str) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    fn close(&self);
}

/// What the driver asks of rush's host module.
pub trait Sessions: Send + Sync {
    /// Starts a session with config cfg (a host.Config as JSON) and is its
    /// id (host.Spawn).
    fn spawn(&self, cfg: Value) -> Result<String, String>;
    /// Follows session id (host.Dial): its events go to tx as member i's,
    /// a last closed Msg once its lines end.
    fn dial(&self, id: &str, i: usize, tx: SyncSender<Msg>) -> Result<Box<dyn Session>, String>;
    /// Records an event of session id's (host.LogEvent).
    fn log_event(&self, id: &str, kind: &str, fields: Map<String, Value>);
    /// Records an event in rush's own log (events.Log).
    fn log(&self, kind: &str, fields: Map<String, Value>) {
        rush_events::log(kind, fields);
    }
    /// Why a turn failed, in words (host.FailReport).
    fn fail_report(&self, kind: &str, err: &str, step: &str, tries: i64) -> String;
}

/// One member of the panel, as the driver keeps it.
struct MemberState {
    m: Member,
    kind: String,
    c: Option<Box<dyn Session>>,
    /// The room's log entries already shown to it.
    seen: usize,
    /// It has had a turn: it knows the rules.
    spoke: bool,
    /// Its last turn was paused: it picks it up again.
    resumed: bool,
    /// Its latest READY.
    ready: String,
    /// The turn number it said so at.
    ready_n: usize,
    /// Your entries handed into a turn already.
    told: HashSet<usize>,
    /// Its last word, once the room is over.
    final_: String,
    /// The model it says it runs, once it has.
    model: String,
    /// Its session went away.
    out: bool,
}

/// One member's turn under way.
struct Turn {
    m: usize,
    n: usize,
    round: i64,
    /// Why the room is over, when this is its last word.
    final_: String,
    said: String,
    failed: String,
    ended: bool,
    done: bool,
    paused: bool,
    /// Tool calls and results posted, by ID: agents repeat them.
    posted: HashMap<String, String>,
    started: Instant,
}

struct Driver<'a> {
    rooms: &'a Rooms,
    sessions: &'a dyn Sessions,
    agents: &'a dyn Agents,
    r: Room,
    ms: Vec<MemberState>,
    /// What's been said, in order: what prompts are made of.
    log: Vec<Entry>,
    rx: Receiver<Msg>,
    tx: SyncSender<Msg>,
    /// Members' events heard while taking in what you said.
    pending: VecDeque<Msg>,
    /// Turns taken, numbering them.
    turns: usize,
    /// The turn number you last spoke at.
    user_n: usize,
    /// What speaks next: members together (blind openings), else one at a
    /// time.
    queue: Vec<Vec<usize>>,
    stop: bool,
    verdict: bool,
    paused: bool,
}

fn fields(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Runs room id until it's over, and returns then.
pub fn run(
    rooms: &Rooms,
    id: &str,
    sessions: &dyn Sessions,
    agents: &dyn Agents,
) -> Result<(), String> {
    let r = rooms.load(id)?;
    if rooms.running(id) {
        return Err(format!("room {id} is already running"));
    }
    crate::room::write_file(
        &rooms.pid_path(id),
        std::process::id().to_string().as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    struct Unpid(std::path::PathBuf);
    impl Drop for Unpid {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _unpid = Unpid(rooms.pid_path(id));
    let (tx, rx) = sync_channel(4096);
    let (entries, off) = rooms.read(id, 0).unwrap_or((Vec::new(), 0));
    if entries.iter().any(|e| e.kind == END) {
        return Err(format!("room {id} is over"));
    }
    let stop = Arc::new(rush_sys::fswait::Stop::new().map_err(|e| e.to_string())?);
    let watcher = {
        let (stop, tx, path, rooms, id) = (
            stop.clone(),
            tx.clone(),
            rooms.log_path(id),
            rooms.clone(),
            id.to_string(),
        );
        std::thread::spawn(move || watch(&rooms, &id, path, off, &stop, &tx))
    };
    let mut d = Driver {
        rooms,
        sessions,
        agents,
        r: r.clone(),
        ms: Vec::new(),
        log: Vec::new(),
        rx,
        tx,
        pending: VecDeque::new(),
        turns: 0,
        user_n: 0,
        queue: Vec::new(),
        stop: false,
        verdict: false,
        paused: false,
    };
    sessions.log(
        "room-started",
        fields(json!({"room": id, "topic": truncate(&r.topic, 300), "members": r.members.len(), "rounds": r.rounds})),
    );
    d.post(Entry {
        from: "room".into(),
        kind: INFO.into(),
        text: format!("opening the room: {}", r.topic),
        ..Entry::default()
    });
    for (i, m) in r.members.iter().enumerate() {
        d.ms.push(MemberState {
            m: m.clone(),
            kind: m.kind(),
            c: None,
            seen: 0,
            spoke: false,
            resumed: false,
            ready: String::new(),
            ready_n: 0,
            told: HashSet::new(),
            final_: String::new(),
            model: String::new(),
            out: false,
        });
        if let Err(e) = d.open(i) {
            sessions.log(
                "room-member-failed",
                fields(json!({"room": id, "member": m.name, "err": e})),
            );
            d.ms[i].out = true;
            d.post(Entry {
                from: "room".into(),
                kind: INFO.into(),
                text: format!("{} couldn't join: {e}", m.name),
                ..Entry::default()
            });
        }
    }
    let why = d.run_loop();
    sessions.log("room-ended", fields(json!({"room": id, "why": why})));
    d.post(Entry {
        from: "room".into(),
        kind: END.into(),
        text: why,
        ..Entry::default()
    });
    for m in &d.ms {
        if let Some(c) = &m.c {
            let _ = c.stop();
            c.close();
        }
    }
    stop.close();
    drop(d);
    let _ = watcher.join();
    Ok(())
}

/// Hands the driver what you post to the log.
fn watch(
    rooms: &Rooms,
    id: &str,
    path: std::path::PathBuf,
    mut off: u64,
    stop: &rush_sys::fswait::Stop,
    tx: &SyncSender<Msg>,
) {
    let mut file = rush_sys::fswait::File { path, size: off };
    loop {
        if !rush_sys::fswait::grown(stop, std::slice::from_ref(&file)) {
            return;
        }
        let Ok((es, next)) = rooms.read(id, off) else {
            continue;
        };
        off = next;
        file.size = next;
        for e in es {
            if e.from == USER && tx.send(Msg::User(Box::new(e))).is_err() {
                return;
            }
        }
    }
}

impl Driver<'_> {
    /// Starts member i's session and follows it.
    fn open(&mut self, i: usize) -> Result<(), String> {
        let m = &self.ms[i].m;
        let mut cfg: Map<String, Value> =
            serde_json::from_str(m.config.get()).map_err(|e| e.to_string())?;
        cfg.insert("cwd".into(), Value::String(self.r.cwd.clone()));
        cfg.remove("prompt");
        cfg.remove("images");
        cfg.insert(
            "name".into(),
            Value::String(format!("Room · {} · {}", m.name, self.r.topic)),
        );
        cfg.insert(
            "meta".into(),
            json!({"room": self.r.id, "roomMember": m.name}),
        );
        cfg.insert("systemPrompt".into(), Value::String(SYSTEM.into()));
        cfg.insert("owner".into(), json!(std::process::id())); // the panel ends with its room
        let kind = cfg
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = m.name.clone();
        let started = self.sessions.spawn(Value::Object(cfg))?;
        self.sessions.log_event(
            &started,
            "spawned",
            fields(json!({"by": "room", "room": self.r.id, "member": name, "agent": kind, "cwd": self.r.cwd})),
        );
        let c = self.sessions.dial(&started, i, self.tx.clone())?;
        self.ms[i].c = Some(c);
        self.post(Entry {
            from: name,
            kind: JOINED.into(),
            id: started,
            ..Entry::default()
        });
        Ok(())
    }

    fn post(&mut self, mut e: Entry) {
        if e.at.is_zero() {
            e.at = rush_state::Time::now();
        }
        if let Err(err) = self.rooms.post(&self.r.id, e.clone()) {
            eprintln!("room: post: {err}");
        }
        if e.kind == SAY || e.kind == TOOL {
            self.log.push(e);
        }
    }

    fn info(&mut self, text: String) {
        self.post(Entry {
            from: "room".into(),
            kind: INFO.into(),
            text,
            ..Entry::default()
        });
    }

    fn live(&self) -> Vec<usize> {
        (0..self.ms.len()).filter(|&i| !self.ms[i].out).collect()
    }

    /// The next message: one held back first, else one to come, waiting
    /// up to timeout (None for as long as it takes).
    fn next(&mut self, timeout: Option<Duration>) -> Option<Msg> {
        if let Some(m) = self.pending.pop_front() {
            return Some(m);
        }
        match timeout {
            None => self.rx.recv().ok(),
            Some(t) => self.rx.recv_timeout(t).ok(),
        }
    }

    /// Runs turns until the room is over, and says why it is.
    fn run_loop(&mut self) -> String {
        let mut round = 0i64;
        loop {
            self.take_user(&mut []);
            if self.paused && !self.stop {
                self.wait_resume();
            }
            let live = self.live();
            if self.stop {
                return "stopped by you".into();
            }
            if live.len() < 2 {
                if live.len() == 1 && !self.log.is_empty() {
                    let why = format!("only {} is left in the room", self.ms[live[0]].m.name);
                    self.conclude(&why);
                }
                return "not enough agents left to argue".into();
            }
            if self.verdict {
                return self.conclude("you called for the verdict");
            }
            if self.queue.is_empty() {
                if round >= 2 && self.agreed(&live) {
                    return self.conclude("everyone said READY: yes in the same round");
                }
                if round >= self.r.rounds {
                    let why = format!("the room hit its cap of {} rounds", self.r.rounds);
                    return self.conclude(&why);
                }
                round += 1;
                self.queue = live.iter().map(|&i| vec![i]).collect();
                if round == 1 {
                    self.queue = vec![live.clone()]; // blind openings: together, none seeing the others'
                }
            }
            let batch = self.queue.remove(0);
            let ms: Vec<usize> = batch.into_iter().filter(|&i| !self.ms[i].out).collect();
            let paused = self.speak(&ms, round, "");
            if !paused.is_empty() {
                self.queue.insert(0, paused); // they go again first, in the same round
            }
        }
    }

    /// Holds the room while you've paused it: what you say is taken in,
    /// and nothing new starts until /resume or /stop.
    fn wait_resume(&mut self) {
        while self.paused && !self.stop {
            match self.next(None) {
                Some(Msg::User(e)) => self.on_user(*e, &mut []),
                Some(m @ Msg::Member { .. }) => self.idle_event(m),
                None => return,
            }
        }
    }

    /// A member's events when it isn't speaking.
    fn idle_event(&mut self, m: Msg) {
        let Msg::Member { i, evs, closed } = m else {
            return;
        };
        if closed {
            self.ms[i].out = true;
            return;
        }
        for ev in evs {
            self.answer(i, &ev);
        }
    }

    /// Whether every live member's latest word, since you last spoke, was
    /// READY: yes.
    fn agreed(&self, live: &[usize]) -> bool {
        live.iter().all(|&i| {
            let m = &self.ms[i];
            m.ready == "yes" && m.ready_n > self.user_n
        })
    }

    /// Takes in what you've posted; speaking are the turns under way.
    fn take_user(&mut self, speaking: &mut [Turn]) {
        let mut held = VecDeque::new();
        while let Some(m) = self.pending.pop_front() {
            match m {
                Msg::User(e) => self.on_user(*e, speaking),
                m => held.push_back(m),
            }
        }
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::User(e) => self.on_user(*e, speaking),
                m => held.push_back(m),
            }
        }
        self.pending = held;
    }

    fn on_user(&mut self, e: Entry, speaking: &mut [Turn]) {
        if e.kind == CONTROL {
            match e.text.as_str() {
                "stop" => self.stop = true,
                "verdict" => {
                    self.verdict = true;
                    self.paused = false; // the final positions go ahead, paused or not
                    self.guide(
                        speaking,
                        "",
                        None,
                        "[The user has called for the verdict]: finish this turn now, in a sentence or two.",
                    );
                }
                "pause" if !self.paused => {
                    self.paused = true;
                    for t in speaking.iter_mut() {
                        if !t.done && !t.paused {
                            t.paused = true;
                            if let Some(c) = &self.ms[t.m].c {
                                let _ = c.interrupt();
                            }
                        }
                    }
                    self.info("paused: no turn starts until /resume".into());
                }
                "resume" if self.paused => {
                    self.paused = false;
                    self.info("resumed".into());
                }
                _ => {}
            }
            return;
        }
        if e.kind != SAY {
            return;
        }
        self.log.push(e.clone());
        self.user_n = self.turns;
        if !e.to.is_empty() {
            for i in 0..self.ms.len() {
                let m = &self.ms[i];
                if m.m.name.to_lowercase() == e.to.to_lowercase()
                    && !m.out
                    && !speaking.iter().any(|t| t.m == i)
                {
                    // Straight after this turn, in place of its turn this round.
                    for b in &mut self.queue {
                        b.retain(|&x| x != i);
                    }
                    self.queue.retain(|b| !b.is_empty());
                    self.queue.insert(0, vec![i]);
                }
            }
        }
        if !self.paused {
            let entry = self.log.len() - 1;
            self.guide(
                speaking,
                &e.to,
                Some(entry),
                &format!(
                    "[The user, while you work]: {}\n(Answer the user first.)",
                    e.text
                ),
            );
        }
    }

    /// Hands text into the turns under way whose agents take a message
    /// mid-turn, those it's to; the rest read it at their next turn.
    fn guide(&mut self, speaking: &mut [Turn], to: &str, entry: Option<usize>, text: &str) {
        for t in speaking.iter() {
            let m = &self.ms[t.m];
            if t.done
                || t.paused
                || !to.is_empty() && to.to_lowercase() != m.m.name.to_lowercase()
                || !self.agents.supports_guide(&m.kind)
            {
                continue;
            }
            let Some(c) = &m.c else { continue };
            let sent = c.send_guide(text).is_ok();
            if sent && let Some(entry) = entry {
                self.ms[t.m].told.insert(entry);
            }
        }
    }

    /// Has ms speak at once (one, past the openings), and posts what they
    /// say; it returns those paused part way, to go again.
    fn speak(&mut self, ms: &[usize], round: i64, final_: &str) -> Vec<usize> {
        let mut ts: Vec<Turn> = Vec::new();
        for &i in ms {
            self.turns += 1;
            let t = Turn {
                m: i,
                n: self.turns,
                round,
                final_: final_.to_string(),
                said: String::new(),
                failed: String::new(),
                ended: false,
                done: false,
                paused: false,
                posted: HashMap::new(),
                started: Instant::now(),
            };
            let prompt = self.prompt(i, round, final_); // all made before any is sent: openings stay blind
            let m = &mut self.ms[i];
            m.seen = self.log.len();
            m.spoke = true;
            m.resumed = false;
            if final_.is_empty() {
                let name = m.m.name.clone();
                self.post(Entry {
                    from: name,
                    kind: TURN.into(),
                    round,
                    ..Entry::default()
                });
            }
            let sent = match &self.ms[i].c {
                Some(c) => c.send(&prompt),
                None => Err("its session went away".into()),
            };
            if let Err(e) = sent {
                self.fail(i, &e);
                continue;
            }
            ts.push(t);
        }
        let mut tick = Instant::now() + TICK;
        while ts.iter().any(|t| !t.done) {
            self.take_user(&mut ts);
            if self.stop {
                for t in &ts {
                    if !t.done
                        && let Some(c) = &self.ms[t.m].c
                    {
                        let _ = c.interrupt();
                    }
                }
                return Vec::new();
            }
            let wait = tick.saturating_duration_since(Instant::now());
            let got = self.next(Some(wait));
            match got {
                Some(Msg::User(e)) => {
                    self.on_user(*e, &mut ts);
                    if self.stop {
                        for t in &ts {
                            if !t.done
                                && let Some(c) = &self.ms[t.m].c
                            {
                                let _ = c.interrupt();
                            }
                        }
                        return Vec::new();
                    }
                }
                Some(Msg::Member { i, evs, closed }) => {
                    match ts.iter().position(|t| t.m == i && !t.done) {
                        Some(k) => self.event(&mut ts, k, evs, closed),
                        None => self.idle_event(Msg::Member { i, evs, closed }),
                    }
                }
                None => {}
            }
            if Instant::now() >= tick {
                tick = Instant::now() + TICK;
                for t in ts.iter_mut() {
                    if !t.done && !t.paused && t.started.elapsed() > TURN_LIMIT {
                        if let Some(c) = &self.ms[t.m].c {
                            let _ = c.interrupt();
                        }
                        let name = self.ms[t.m].m.name.clone();
                        self.info(format!(
                            "{name} ran past {} and was stopped",
                            go_duration(TURN_LIMIT)
                        ));
                        t.said = format!("{}\n\n(cut off)", t.said).trim().to_string();
                        self.finish(t);
                    }
                }
            }
        }
        let mut paused = Vec::new();
        for t in &ts {
            if t.paused && !self.ms[t.m].out {
                self.ms[t.m].resumed = true;
                paused.push(t.m);
            }
        }
        paused
    }

    /// Takes one batch of events from the member of turn ts[k].
    fn event(&mut self, ts: &mut [Turn], k: usize, evs: Vec<MemberEvent>, closed: bool) {
        let mi = ts[k].m;
        if closed {
            ts[k].done = true;
            self.fail(mi, "its session went away");
            return;
        }
        let name = self.ms[mi].m.name.clone();
        for ev in evs {
            match ev {
                MemberEvent::Event(ev) if matches!(*ev, Event::Message(_)) => {
                    let Event::Message(e) = *ev else { continue };
                    if !e.parent.is_empty() {
                        continue; // a subagent's: its steps stay in its session
                    }
                    // Words with tool calls are on the way to an answer: a
                    // note before the calls. The last words without any are
                    // the turn's.
                    let mut text = String::new();
                    let mut calls = false;
                    for p in &e.parts {
                        if p.kind == PartKind::Text && e.role == "assistant" {
                            text.push_str(&p.text);
                        }
                        calls = calls || p.kind == PartKind::ToolCall;
                    }
                    let s = text.trim().to_string();
                    if !s.is_empty() || calls {
                        if !ts[k].said.is_empty() {
                            let said = ts[k].said.clone();
                            self.note(&name, said);
                        }
                        ts[k].said = s.clone();
                        if calls && !s.is_empty() {
                            self.note(&name, s);
                            ts[k].said.clear();
                        }
                    }
                    for p in &e.parts {
                        match p.kind {
                            PartKind::ToolCall => {
                                if let Some(c) = &p.call {
                                    self.post_call(&mut ts[k], (**c).clone());
                                }
                            }
                            PartKind::ToolResult => {
                                if let Some(o) = &p.output {
                                    let key = format!("done {}", o.call_id);
                                    if ts[k].posted.get(&key).is_none_or(String::is_empty) {
                                        ts[k].posted.insert(key, "y".into());
                                        self.post(Entry {
                                            from: name.clone(),
                                            kind: DONE.into(),
                                            id: o.call_id.clone(),
                                            failed: o.is_error,
                                            ..Entry::default()
                                        });
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                MemberEvent::Event(ev) if matches!(*ev, Event::CallUpdated(_)) => {
                    let Event::CallUpdated(e) = *ev else { continue };
                    self.post_call(&mut ts[k], e.call);
                }
                MemberEvent::Event(ev) if matches!(*ev, Event::TurnEnd(_)) => {
                    let Event::TurnEnd(e) = *ev else { continue };
                    let t = &mut ts[k];
                    t.ended = true;
                    if t.said.is_empty() {
                        t.said = e.text.trim().to_string();
                    }
                    if e.reason == "error" {
                        t.failed = if e.err.is_empty() {
                            "the turn failed".into()
                        } else {
                            e.err.clone()
                        };
                    }
                }
                MemberEvent::Info(i) => {
                    let m = &mut self.ms[mi];
                    if !i.model.is_empty() && i.model != m.model && m.m.model().is_empty() {
                        m.model = i.model.clone();
                        // the label names the model it actually runs
                        self.post(Entry {
                            from: name.clone(),
                            kind: USES.into(),
                            text: i.model.clone(),
                            ..Entry::default()
                        });
                    }
                    if i.state == "working" {
                        ts[k].ended = false;
                    }
                    let busy = i.retry.as_ref().is_some_and(|r| !r.gave_up) || i.background > 0;
                    let t = &ts[k];
                    if t.ended && (i.state == "idle" && i.queue == 0 && !busy || i.limited)
                        || i.state == "stopped"
                    {
                        if !t.failed.is_empty() && t.said.is_empty() && !t.paused {
                            let (step, tries) = match &i.retry {
                                Some(r) if r.gave_up => (r.last_step.clone(), r.attempt - 1),
                                _ => (String::new(), 0),
                            };
                            let report =
                                self.sessions.fail_report(&i.kind, &t.failed, &step, tries);
                            self.info(format!("{name}'s turn failed: {report}"));
                        }
                        self.finish(&mut ts[k]);
                    }
                }
                MemberEvent::Error(e) => self.info(format!("{name}: {e}")),
                ev => self.answer(mi, &ev),
            }
        }
    }

    fn note(&mut self, from: &str, text: String) {
        self.post(Entry {
            from: from.into(),
            kind: NOTE.into(),
            text,
            ..Entry::default()
        });
    }

    /// Posts t's turn: what it said, its words so far if you paused it, or
    /// its final position.
    fn finish(&mut self, t: &mut Turn) {
        t.done = true;
        let name = self.ms[t.m].m.name.clone();
        let (text, r) = ready(&t.said);
        if t.paused {
            if !text.is_empty() {
                self.note(&name, text);
            }
            self.post(Entry {
                from: name.clone(),
                kind: PAUSED.into(),
                text: format!("{name} was paused mid-turn: it picks it up again on /resume"),
                ..Entry::default()
            });
        } else if !t.final_.is_empty() {
            let said = if text.is_empty() {
                "(said nothing)"
            } else {
                &text
            };
            self.ms[t.m].final_ = said.split_whitespace().collect::<Vec<_>>().join(" ");
        } else {
            self.ms[t.m].ready = r.clone();
            self.ms[t.m].ready_n = t.n;
            self.post(Entry {
                from: name,
                kind: SAY.into(),
                text: if text.is_empty() {
                    "(said nothing)".into()
                } else {
                    text
                },
                ready: r,
                round: t.round,
                ..Entry::default()
            });
        }
    }

    fn fail(&mut self, i: usize, err: &str) {
        let name = self.ms[i].m.name.clone();
        self.sessions.log(
            "room-member-left",
            fields(json!({"room": self.r.id, "member": name, "err": err})),
        );
        self.ms[i].out = true;
        self.info(format!("{name} left the room: {err}"));
    }

    fn post_call(&mut self, t: &mut Turn, mut c: Call) {
        let inp = &c.input;
        let mut detail = inp.command.clone();
        if detail.is_empty() {
            detail = inp.path.clone();
        }
        if detail.is_empty() {
            detail = if inp.pattern.is_empty() {
                inp.url.clone()
            } else {
                inp.pattern.clone()
            };
        }
        if let Some(i) = detail.find('\n') {
            detail = format!("{} …", &detail[..i]);
        }
        if detail.is_empty() && c.title.is_empty() && c.kind != Kind::Other && c.kind != Kind::Mcp {
            return; // announced before its input is known: it comes again with it
        }
        // A room changes no files: what they would have written stays out of its log.
        c.raw = None;
        c.input.content.clear();
        c.input.edits.clear();
        let e = Entry {
            from: self.ms[t.m].m.name.clone(),
            kind: TOOL.into(),
            id: c.id.clone(),
            text: doing(&c),
            detail: truncate(&detail, 300),
            call: Some(c),
            ..Entry::default()
        };
        let key = format!("{}\x00{}", e.text, e.detail);
        if t.posted.get(&e.id) != Some(&key) {
            t.posted.insert(e.id.clone(), key);
            self.post(e);
        }
    }

    /// Settles what member i asks: reading and running are allowed,
    /// changing files is not, and questions go back unanswered.
    fn answer(&mut self, i: usize, ev: &MemberEvent) {
        let MemberEvent::Event(ev) = ev else { return };
        let Some(c) = &self.ms[i].c else { return };
        match &**ev {
            Event::Approval(e) if e.call.kind.changes() => {
                let _ = c.deny(
                    &e.id,
                    "This is a discussion room: don't change files. Say what you'd change instead.",
                );
                let name = self.ms[i].m.name.clone();
                self.sessions.log(
                    "room-denied",
                    fields(json!({"room": self.r.id, "member": name,
                        "call": truncate(&doing(&e.call), 300), "why": "a room changes no files"})),
                );
                self.info(format!("{name} wasn't allowed to {}", doing(&e.call)));
            }
            Event::Approval(e) => {
                let _ = c.allow(&e.id);
            }
            Event::Question(e) => {
                let _ = c.deny(
                    &e.id,
                    "No one answers questions mid-turn in a room: state your assumption and carry on.",
                );
            }
            _ => {}
        }
    }

    /// Has every member give its final position, and posts them as the
    /// room's verdict: one line each, agreeing or not.
    fn conclude(&mut self, why: &str) -> String {
        let mut pending = self.live();
        self.sessions.log(
            "room-concluding",
            fields(json!({"room": self.r.id, "why": why, "members": pending.len()})),
        );
        self.info(format!(
            "the discussion is over ({why}): each agent gives its final position"
        ));
        while !pending.is_empty() && !self.stop {
            pending = self.speak(&pending, 0, why);
            if !pending.is_empty() {
                self.wait_resume();
            }
        }
        let lines: Vec<String> = self
            .ms
            .iter()
            .filter(|m| !m.final_.is_empty())
            .map(|m| format!("{}: {}", m.m.name, m.final_))
            .collect();
        if !lines.is_empty() {
            self.post(Entry {
                from: "room".into(),
                kind: VERDICT.into(),
                text: lines.join("\n"),
                detail: why.into(),
                ..Entry::default()
            });
        }
        if self.stop {
            return "stopped by you".into();
        }
        why.into()
    }

    /// What member i is sent for its turn: the room's rules the first
    /// time, then everything said since its last turn.
    fn prompt(&self, i: usize, round: i64, final_: &str) -> String {
        let m = &self.ms[i];
        let mut b = String::new();
        if !m.spoke {
            b += &format!(
                "You are {}, one of {} AI agents in a rush room: a group discussion the user watches live and can join at any time.\n\n",
                m.m.name,
                self.ms.len()
            );
            b += "The panel:\n";
            for o in &self.ms {
                b += &format!("- {} ({})\n", o.m.name, o.m.label(self.agents));
            }
            b += &format!(
                "\nTopic: {}\nWorking folder: {}\n",
                self.r.topic, self.r.cwd
            );
            b += &if_set(
                "\nThe user opened this room from a chat with an agent. The latest of that chat, for context:\n\n",
                &self.r.brief,
            );
            b += &format!("\n{}\n", rules(self.r.rounds));
        }
        let (delta, to_you, from_user) = self.since(i);
        if !delta.is_empty() {
            b += if !m.spoke {
                "\nSo far in the room:\n"
            } else {
                "\nNew in the room since your last turn:\n"
            };
            b += &delta;
        }
        b += "\n";
        if !final_.is_empty() {
            b += &format!(
                "The discussion is over: {final_}. State your own final position on the topic in one line, as you hold it now, whether or not the others agree. No tools, and no READY line.\n"
            );
            return b;
        }
        if m.resumed {
            b += "The user paused the room in the middle of your last turn and has now resumed it: pick your turn up again.\n";
        }
        if to_you {
            b += "The user spoke to you directly: answer the user first.\n";
        } else if from_user {
            b += "The user spoke: answer the user first, then the others.\n";
        }
        if round == 1 {
            b += &format!(
                "Round 1: openings. Everyone writes theirs at once, without seeing the others'; give your own view, {}. End with READY: yes or READY: no.\n",
                m.m.name
            );
            return b;
        }
        b += &format!(
            "It's your turn, {} (round {round} of at most {}). Reply to the others by name, and end with READY: yes or READY: no.\n",
            m.m.name, self.r.rounds
        );
        b
    }

    /// The room since member i's last turn, as it reads it: whether you
    /// spoke to it, and whether you spoke at all.
    fn since(&self, i: usize) -> (String, bool, bool) {
        let m = &self.ms[i];
        let mut parts: Vec<String> = Vec::new();
        let (mut to_you, mut from_user) = (false, false);
        for (k, e) in self.log[m.seen.min(self.log.len())..].iter().enumerate() {
            if e.from == m.m.name || m.told.contains(&(m.seen + k)) {
                continue;
            }
            if e.from == USER {
                from_user = true;
                let mut who = "The user".to_string();
                if !e.to.is_empty() {
                    who += &format!(" (to {})", e.to);
                    to_you = to_you || e.to.to_lowercase() == m.m.name.to_lowercase();
                }
                parts.push(format!("\n{who}:\n{}\n", e.text));
            } else if e.kind == TOOL {
                parts.push(format!(
                    "  ({} ran a tool: {}{})\n",
                    e.from,
                    e.text,
                    if_set(" — ", &e.detail)
                ));
            } else if e.kind == SAY {
                parts.push(format!(
                    "\n{}:\n{}\n{}",
                    e.from,
                    e.text,
                    if_set("READY: ", &e.ready)
                ));
            }
        }
        // The newest is kept whole; the oldest goes first when it's long.
        let budget = self.budget();
        let mut total = 0;
        let mut from = parts.len();
        while from > 0 && total + parts[from - 1].len() <= budget {
            from -= 1;
            total += parts[from].len();
        }
        if from == parts.len() && from > 0 {
            // even the newest is too long: its tail
            let last = &parts[from - 1];
            let mut cut = last.len().saturating_sub(budget);
            while !last.is_char_boundary(cut) {
                cut += 1; // a character cut in two is left out
            }
            parts[from - 1] = format!("[shortened] {}", &last[cut..]);
            from -= 1;
        }
        let mut out = parts[from..].concat();
        if from > 0 {
            out = format!("(earlier turns left out for length)\n{out}");
        }
        (out, to_you, from_user)
    }

    /// PROMPT_BUDGET, or less where a member's context window is small:
    /// about a byte and a half per token of it, less the rules.
    fn budget(&self) -> usize {
        let mut b = PROMPT_BUDGET as i64;
        for m in &self.ms {
            if m.m.context > 0 {
                // Saturating: a context read from room.json can be anything.
                b = b.min(
                    (m.m.context.saturating_mul(3) / 2)
                        .saturating_sub(4096)
                        .max(2048),
                );
            }
        }
        b as usize
    }
}

fn if_set(lead: &str, s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    format!("{lead}{s}")
}

/// s cut to n bytes, never mid-character, marked when cut.
pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// d as Go's Duration.String writes whole minutes and seconds: "10m0s".
fn go_duration(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, m) => format!("{m}m{s}s"),
        (h, m) => format!("{h}h{m}m{s}s"),
    }
}

/// What every member's session is told first, after rush's own.
pub const SYSTEM: &str = "You are taking part in a rush room: a live group discussion between AI agents of different models, which the user watches and can join at any time. Everything you write is visible to everyone. Hold real opinions: test claims, challenge the others by name when you think they're wrong, and change your mind only when persuaded, saying what persuaded you. You may read files and run read-only commands to check facts; don't edit or create files. Keep each turn short.";

/// The room's rules, as its members first read them.
fn rules(rounds: i64) -> String {
    format!(
        "How the room works:
- Round 1 is openings: everyone writes theirs at once, without seeing the others'. From round 2 you take turns, and each turn you're shown everything said since your last, including the tools the others ran.
- Argue honestly. Form your own view, check claims against the code or by running read-only commands when it matters, and challenge the others by name when you think they're wrong. Don't agree to be polite.
- Don't edit or create files: this is a discussion.
- The user may speak at any time. When they do, answer them first.
- Keep each turn to a few short paragraphs.
- End every turn with exactly one last line: \"READY: yes\" when you hold a position you'd sign and nothing the others said changes it, or \"READY: no\" while something is still unsettled.
- The room ends when everyone says READY: yes in the same round (from round 2), or after {rounds} rounds; then each of you states a final position in one line."
    )
}

#[cfg(test)]
mod tests;
