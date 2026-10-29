//! The host's per-agent watching: the idle wait and the host's sleep,
//! check-ins while you're away, a stalled turn, hung shells, long
//! background tasks, a message the agent dropped, the answers a spawned
//! agent owes, what it left outside its project, and following it to the
//! checkout it works in.
//!
//! Where Go runs a ticker for each of these for the host's whole life,
//! here each is a timer armed only while there's something to watch: an
//! idle host with no turn running wakes for nothing.
//!
//! Ported from internal/host/sleep.go:15-185 (idleFor, armIdle,
//! disarmIdle, runtimeIdleSafe, sleepSafe, answerGiven, rest,
//! stopRestingAgent), away.go:117-200 (hold, awaySwitches, watchAway,
//! checkAway, setAway, endAway), stall.go, stuck.go:30-117 (shellCalls,
//! watchStuck), longtask.go:30-133 (watchLongTasks, askable),
//! heard.go, left.go:26-67 (noteLeft, projectOf), edits.go:19-55
//! (followEdits), agent.go:186-245 (followCwd), startctx.go and
//! subtool.go:371-452 (said, turnDone, reportAnswer).

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use rush_core::adapter::{Conn, Input};
use rush_core::event::{self, Event, PartKind};
use rush_core::tool;
use rush_core::usage;
use rush_state::Time;
use serde_json::{Value, json};

use super::{Host, State};
use crate::away::{AWAY_HELD, DEFAULT_AWAY_EVERY, away_due, away_note, keep_awake};
use crate::events::clip;
use crate::longtask::{LONG_TASK_LAST, last_output, long_task_note, overdue_tasks, task_dirs};
use crate::paths::dir;
use crate::stuck::{STUCK_AFTER, STUCK_LOOK, Waited, agent_command, short, stuck_note};
use crate::subtool::{
    Answer, agent_name, answer_path, fail_report, finished, report, report_path, send_result,
    titled,
};
use crate::wire::{Away, Held, Task};

/// How long a session you started waits idle before its host sleeps.
pub const ROOT_IDLE: Duration = Duration::from_secs(30 * 60);
/// A spawned agent waits as long as its prompt cache lasts: there's no
/// warm cache to keep after that.
pub const SPAWNED_IDLE: Duration = Duration::from_secs(5 * 60);

/// A turn that has heard nothing from its agent this long, with no tool
/// running and nothing asked, is marked stalled. rush never ends an agent
/// on its own: it only says so.
pub const STALL_AFTER: Duration = Duration::from_secs(5 * 60);

/// How soon a turn silent long enough, but with a tool running or
/// something asked, is looked at again (Go's stall ticker).
const STALL_RELOOK: Duration = Duration::from_secs(60);

/// How often an away session is looked at.
const AWAY_LOOK: Duration = Duration::from_secs(30);

/// How often the background tasks are looked at, while there are some.
const LONG_LOOK: Duration = Duration::from_secs(60);

/// How often, at most, the host looks at where the agent works.
const CWD_EVERY: Duration = Duration::from_secs(5);

/// What rush gives the start under way, and the start before, to say what
/// changed (startctx.go).
const START_FILE: &str = "context-start.json";

/// A message as it went to the agent, and when (heard.go's sent).
#[derive(Clone, Debug)]
pub(crate) struct Unheard {
    pub input: Input,
    pub at: rush_core::time::Ms,
    /// Sent a second time already.
    pub again: bool,
    /// Which send it is, to tell it from a later one.
    pub n: u64,
}

/// The per-agent part of the host's state.
#[derive(Default)]
pub(crate) struct Watching {
    pub idle: Option<crate::timer::TimerId>,
    pub idle_gen: u64,
    /// An answer held for a queued turn's, and answers being sent.
    pub owed: String,
    pub reporting: i64,
    /// caffeinate, while you're away.
    pub awake: Option<crate::away::Awake>,
    pub away_timer: Option<crate::timer::TimerId>,
    /// Which arming the away look is: one cancelled as it fired does nothing.
    pub away_gen: u64,
    /// The tool calls open, and the message that made each.
    pub open: HashMap<String, String>,
    pub stall_timer: Option<crate::timer::TimerId>,
    pub waits_on: HashMap<String, Waited>,
    pub stuck_timer: Option<crate::timer::TimerId>,
    pub stuck_seen: HashMap<rush_sys::proc::Pid, crate::stuck::TreeUse>,
    pub nudged: HashMap<String, Time>,
    pub check_in_held: String,
    pub long_timer: Option<crate::timer::TimerId>,
    pub edited: Vec<String>,
    pub proj_for: String,
    pub proj: String,
    pub unheard: Option<Unheard>,
    pub sends: u64,
    pub last_start: Option<usage::Start>,
    pub this_start: Option<usage::Start>,
    pub cwd_at: Option<Instant>,
}

impl Host {
    // ---- Sleep (sleep.go) ----

    /// How long this host waits idle before it sleeps.
    fn idle_for(&self, st: &State) -> Duration {
        if st.cfg.meta.get("spawnedBy").is_some_and(|s| !s.is_empty()) {
            return self.hooks.spawned_idle;
        }
        self.hooks.root_idle
    }

    /// Starts the idle wait again, from now.
    pub(super) fn arm_idle(&self, st: &mut State) {
        self.disarm_idle(st);
        let generation = st.w.idle_gen;
        let h = self.clone();
        st.w.idle = Some(self.timers.after(self.idle_for(st), move || {
            let h2 = h.clone();
            h.go("rest", move |_| h2.rest(generation));
        }));
    }

    /// Stops the idle wait: work has started again. It also makes a rest
    /// already running give up.
    pub(super) fn disarm_idle(&self, st: &mut State) {
        st.w.idle_gen += 1;
        if let Some(t) = st.w.idle.take() {
            self.timers.cancel(t);
        }
    }

    /// Whether the agent's program may be stopped: idle, with nothing
    /// asked of you, no task or side question of its own running, and no
    /// move to another account under way.
    pub(super) fn runtime_idle_safe(&self, st: &State) -> bool {
        st.info.state == "idle"
            && st.info.needs.is_empty()
            && st.pending.is_empty()
            && !self.still_working(st)
            && !st.info.relogin
            && !st.quiet_wait
    }

    /// Whether the host may exit: nothing at all is left for it to do.
    pub(super) fn sleep_safe(&self, st: &State) -> bool {
        self.runtime_idle_safe(st) && st.info.quiet() && self.answer_given(st)
    }

    /// Whether a spawned agent owes no answer.
    fn answer_given(&self, st: &State) -> bool {
        if st.cfg.meta.get("spawnedBy").is_none_or(|s| s.is_empty()) {
            return true;
        }
        if !st.w.owed.is_empty() || st.w.reporting > 0 {
            return false;
        }
        !report_path(&st.cfg.id).exists()
    }

    /// The idle wait running out: it stops the agent's program once
    /// nothing needs it, and puts the host to sleep once nothing needs
    /// that either. Anything left to do waits another idle time. A send
    /// since the wait began (a newer generation) makes it do nothing.
    pub(super) fn rest(&self, generation: u64) {
        let mut st = self.lock();
        if st.w.idle_gen != generation || st.info.sleeping || st.info.state != "idle" {
            return;
        }
        if !self.runtime_idle_safe(&st) {
            self.arm_idle(&mut st);
            return;
        }
        let spawned = st.cfg.meta.get("spawnedBy").is_some_and(|s| !s.is_empty());
        if spawned && !self.answer_given(&st) && st.w.owed.is_empty() && st.w.reporting == 0 {
            self.report_answer(&mut st); // asked for as a message and not sent yet: send it, then sleep next time
        }
        let conn = st.conn.clone();
        drop(st);
        // Its own shells (a build it left running) are read from the
        // process table, which isn't done holding the lock.
        if let Some(c) = &conn
            && super::runs_shells(c.pid().unwrap_or(0) as i64)
        {
            let mut st = self.lock();
            if st.w.idle_gen == generation && st.info.state == "idle" {
                self.arm_idle(&mut st);
            }
            return;
        }
        let mut st = self.lock();
        if st.w.idle_gen != generation
            || !super::same_conn(&st.conn, &conn)
            || !self.runtime_idle_safe(&st)
        {
            return;
        }
        if conn.is_some() {
            self.detach(&mut st);
            st.stopping = true;
            self.publish(&mut st);
        }
        drop(st);
        if let Some(c) = &conn {
            stop_resting_agent(c);
            let mut st = self.lock();
            st.stopping = false;
            drop(st);
            self.cv.notify_all();
        }
        let mut st = self.lock();
        while st.stopping && !st.quit {
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        if st.w.idle_gen != generation || st.conn.is_some() || st.quit {
            return;
        }
        let idle = self.idle_for(&st);
        let idle_s = rush_gate::godur::string(idle);
        if !self.sleep_safe(&st) {
            if conn.is_some() {
                self.event(
                    &st,
                    "rest",
                    json!({"why": "idle, but the host still has work waiting", "idleFor": idle_s}),
                );
            }
            self.arm_idle(&mut st);
            return;
        }
        // The CLI shim owns its host until it has collected the final
        // reply. Its owner watcher, or an explicit stop, ends it after
        // returning that reply.
        if st.cfg.owner > 0 {
            return;
        }
        st.cfg.prompt.clear();
        st.cfg.images.clear();
        st.cfg.prompt_exchange = None;
        st.cfg.resume = st.cfg.resume || (st.began && !st.cfg.session_id.is_empty());
        self.save_config(&st);
        self.event(
            &st,
            "sleep",
            json!({"why": "idle with nothing to do", "idleFor": idle_s, "spawned": spawned}),
        );
        st.info.sleeping = true;
        self.publish(&mut st);
        // Published before the socket closes. The UI also reads the saved
        // info should the socket's end reach it first.
        self.quit(&mut st);
    }

    // ---- Away (away.go) ----

    /// Saves what the agent asks while you're away and refuses it now,
    /// saying why, so it carries on. Never an allow. It reports whether it
    /// held it.
    pub(super) fn hold(&self, st: &mut State, id: &str, kind: &str, text: &str) -> bool {
        let Some(a) = &mut st.info.away else {
            return false;
        };
        if !a.gone() {
            return false;
        }
        if !a.held.iter().any(|h| h.kind == kind && h.text == text) {
            a.held.push(Held {
                kind: kind.into(),
                text: text.into(),
                at: Time::now(),
            });
        }
        let (h, id) = (self.clone(), id.to_string());
        self.go("held", move |_| {
            let mut o = crate::op::Op::named("deny");
            o.id = id;
            o.message = AWAY_HELD.into();
            let _ = h.do_op(o);
        });
        true
    }

    /// Whether this session's profile moves on at a usage limit rather
    /// than waiting for the reset.
    pub(super) fn away_switches(&self, st: &State) -> bool {
        rush_state::Store::load()
            .config
            .profile_for(&st.cfg.cwd, &st.cfg.profile)
            .limit()
            != rush_state::profiles::LIMIT_WAIT
    }

    /// Starts (Some) or ends being away, keeping the machine from sleeping
    /// while you're gone. What it already waits on is held, as if asked
    /// now. The caller publishes.
    pub(super) fn set_away(&self, st: &mut State, a: Option<Away>) {
        if let Some(a) = st.w.awake.take() {
            a.end();
        }
        if let Some(t) = st.w.away_timer.take() {
            self.timers.cancel(t);
        }
        st.w.away_gen += 1;
        st.info.away = a;
        let Some(a) = &mut st.info.away else { return };
        if !a.on() {
            return;
        }
        if a.every.is_zero() {
            a.every = DEFAULT_AWAY_EVERY;
        }
        if a.next.is_zero() {
            a.next = Time::now(); // idle now, it's told straight away
        }
        let gone = a.gone();
        if gone && Time::now().before(a.until) {
            st.w.awake = keep_awake(a.until);
        }
        let held: Vec<(String, bool)> = st
            .pending
            .iter()
            .map(|(id, q)| (id.clone(), q.question))
            .collect();
        let needs = st.info.needs.clone();
        for (id, question) in held {
            let kind = if question { "question" } else { "permission" };
            self.hold(st, &id, kind, &needs);
        }
        self.arm_away(st);
    }

    /// Looks at an away session every AWAY_LOOK while it's on.
    fn arm_away(&self, st: &mut State) {
        if st.info.away.as_ref().is_none_or(|a| !a.on()) {
            return;
        }
        let (h, generation) = (self.clone(), st.w.away_gen);
        st.w.away_timer = Some(self.timers.after(AWAY_LOOK, move || {
            let h2 = h.clone();
            // Off the timers' thread: a check-in sends to the agent.
            h.go("away", move |_| {
                let mut st = h2.lock();
                if st.w.away_gen != generation || st.quit {
                    return; // set again since: that arming looks instead
                }
                st.w.away_timer = None;
                h2.check_away(&mut st, Time::now());
                if st.w.away_gen == generation && st.w.away_timer.is_none() {
                    h2.arm_away(&mut st);
                }
            });
        }));
    }

    /// A look at an away session at now: over, or due a check-in.
    pub(super) fn check_away(&self, st: &mut State, now: Time) {
        // Not while a limit or a retry carries on by itself: one given up
        // on, the check-in is the retry.
        let idle = st.info.state == "idle"
            && st.pending.is_empty()
            && st.info.queue.is_empty()
            && st.info.limit.is_none()
            && st.info.retry.as_ref().is_none_or(|r| r.gave_up)
            && !st.info.sleeping;
        let (over, send) = away_due(st.info.away.as_ref(), idle, now);
        if over {
            self.event(st, "away-over", Value::Null);
            self.end_away(st);
            self.publish(st);
        } else if send {
            let Some(a) = &mut st.info.away else { return };
            a.next = now + a.every;
            a.nudges += 1;
            let (note, n) = (away_note(a, now), a.nudges);
            let err = self.send_locked(st, &note).err().unwrap_or_default();
            self.event(
                st,
                "check-in",
                json!({"why": "away", "nudge": n, "err": err}),
            );
        }
    }

    /// Ends the check-ins: a loop goes, while an away is kept for you to
    /// see what happened once you're back.
    pub(super) fn end_away(&self, st: &mut State) {
        match st.info.away.clone() {
            Some(mut a) if a.gone() => {
                a.ended = Time::now();
                self.set_away(st, Some(a));
            }
            _ => self.set_away(st, None),
        }
    }

    // ---- Stalls (stall.go) ----

    /// Notes that the agent said something, and which tool calls it has
    /// open: one running can be silent as long as it likes. Only the main
    /// agent's count, and a new message from the model closes those of
    /// earlier ones.
    pub(super) fn heard_from(&self, st: &mut State, ev: &Event) {
        st.heard = Some(Instant::now());
        if !st.info.stalled.is_zero() {
            st.info.stalled = Time::default();
            self.event(st, "stall-ended", Value::Null);
            self.publish(st);
        }
        self.arm_stall(st);
        let Event::Message(m) = ev else { return };
        if !m.parent.is_empty() {
            return;
        }
        if m.role == "assistant" && !m.id.is_empty() {
            st.w.open.retain(|_, by| by.is_empty() || *by == m.id);
        }
        for p in &m.parts {
            match p.kind {
                PartKind::ToolCall => {
                    if let Some(c) = p.call.as_ref().filter(|c| !c.id.is_empty()) {
                        // A harness giving each call a message of its own:
                        // parallel ones overlap.
                        let by = if m.id == c.id {
                            String::new()
                        } else {
                            m.id.clone()
                        };
                        st.w.open.insert(c.id.clone(), by);
                    }
                }
                PartKind::ToolResult => {
                    if let Some(o) = &p.output {
                        st.w.open.remove(&o.call_id);
                    }
                }
                _ => {}
            }
        }
    }

    /// Ends what heard_from keeps for a turn: a new one starts unheard and
    /// unstalled.
    pub(super) fn quiet_turn(&self, st: &mut State) {
        st.heard = Some(Instant::now());
        st.w.open.clear();
        st.info.stalled = Time::default();
        self.arm_stall(st);
    }

    /// Whether the turn has heard nothing for STALL_AFTER with nothing
    /// running or asked.
    pub(super) fn silent(&self, st: &State, now: Instant) -> bool {
        st.conn.is_some()
            && st.info.state == "working"
            && st.w.open.is_empty()
            && st.pending.is_empty()
            && st
                .heard
                .is_some_and(|h| now.saturating_duration_since(h) >= STALL_AFTER)
    }

    /// Looks again once the turn could have gone silent: STALL_AFTER after
    /// it last heard anything. One look is pending at most.
    fn arm_stall(&self, st: &mut State) {
        self.arm_stall_in(st, None);
    }

    /// arm_stall, after d when given.
    fn arm_stall_in(&self, st: &mut State, d: Option<Duration>) {
        if st.w.stall_timer.is_some() || st.quit {
            return;
        }
        let Some(heard) = st.heard else { return };
        let d =
            d.unwrap_or_else(|| (heard + STALL_AFTER).saturating_duration_since(Instant::now()));
        let h = self.clone();
        st.w.stall_timer = Some(self.timers.after(d, move || {
            let mut st = h.lock();
            st.w.stall_timer = None;
            let now = Instant::now();
            h.mark_stall(&mut st, now);
            if st.info.stalled.is_zero() && st.conn.is_some() && st.info.state == "working" {
                // Heard since: look again once that could have gone silent.
                // Silent long enough already, but a tool runs or something
                // is asked: look again a minute on, as Go's ticker does,
                // rather than straight away and on and on.
                let quiet_enough = st
                    .heard
                    .is_some_and(|h| now.saturating_duration_since(h) >= STALL_AFTER);
                h.arm_stall_in(&mut st, quiet_enough.then_some(STALL_RELOOK));
            }
        }));
    }

    /// Marks the turn stalled once it has gone silent. It never stops the
    /// turn or its agent: you decide.
    pub(super) fn mark_stall(&self, st: &mut State, now: Instant) {
        if st.info.stalled.is_zero() && self.silent(st, now) {
            let heard = st.heard.unwrap_or(now);
            let ago = now.saturating_duration_since(heard);
            st.info.stalled = Time::now().add_nanos(-(ago.as_nanos() as i64));
            let quiet = rush_gate::godur::string(Duration::from_secs(
                (ago.as_millis() as u64 + 500) / 1000,
            ));
            self.event(st, "stalled", json!({"quiet": quiet}));
            self.publish(st);
        }
    }

    // ---- Hung shells (stuck.go) ----

    /// Keeps the shell commands the main turn waits on, by call: those
    /// started in the background aren't waited on.
    pub(super) fn shell_calls(&self, st: &mut State, m: &event::Message) {
        if !m.parent.is_empty() {
            return;
        }
        for p in &m.parts {
            match p.kind {
                PartKind::ToolCall => {
                    if let Some(c) = &p.call
                        && c.kind == tool::Kind::Shell
                        && !c.input.background
                        && m.role == "assistant"
                    {
                        st.w.waits_on.entry(c.id.clone()).or_insert(Waited {
                            cmd: c.input.command.clone(),
                            at: SystemTime::now(),
                        });
                    }
                }
                PartKind::ToolResult => {
                    if let Some(o) = &p.output {
                        st.w.waits_on.remove(&o.call_id);
                    }
                }
                _ => {}
            }
        }
        self.arm_stuck(st);
    }

    /// Looks at the shells the turn waits on every STUCK_LOOK, while it
    /// waits on some.
    fn arm_stuck(&self, st: &mut State) {
        if st.w.waits_on.is_empty() {
            st.w.stuck_seen.clear();
            return;
        }
        if st.w.stuck_timer.is_some() || st.quit {
            return;
        }
        let h = self.clone();
        st.w.stuck_timer = Some(self.timers.after(STUCK_LOOK, move || {
            let h2 = h.clone();
            h.go("stuck", move |_| {
                h2.look_stuck();
                let mut st = h2.lock();
                st.w.stuck_timer = None;
                h2.arm_stuck(&mut st);
            });
        }));
    }

    /// One look at the shells the turn waits on: a hung one is stopped,
    /// and the agent told.
    fn look_stuck(&self) {
        let (pid, from, mut cmd, n, mut seen) = {
            let mut st = self.lock();
            let mut first: Option<&Waited> = None;
            for w in st.w.waits_on.values() {
                if first.is_none_or(|f| w.at < f.at) {
                    first = Some(w);
                }
            }
            let (from, cmd) = first
                .map(|w| (w.at, w.cmd.clone()))
                .unwrap_or((SystemTime::UNIX_EPOCH, String::new()));
            let n = st.w.waits_on.len();
            (
                st.info.claude_pid,
                from,
                cmd,
                n,
                std::mem::take(&mut st.w.stuck_seen),
            )
        };
        if pid <= 0 || n == 0 {
            return;
        }
        let mut tab = rush_sys::proc::snapshot();
        let shells = crate::stuck::waited_shells(&tab, pid as i32, from);
        let gate = rush_gate::Gate::new();
        let hung = crate::stuck::idle_shells(&mut tab, &shells, &mut seen, Instant::now(), &|p| {
            gate.waiting(p)
        });
        for sh in hung {
            cmd = if n > 1 {
                crate::stuck::shell_command(sh)
            } else {
                agent_command(&cmd) // as the agent wrote it, not as rush ran it
            };
            let timers = self.timers.clone();
            crate::stuck::stop_tree(&tab, sh, |d, f| {
                timers.after(d, f);
            });
            seen.remove(&sh);
            let st = self.lock();
            self.event(
                &st,
                "stuck-stopped",
                json!({"pid": sh, "cmd": short(&cmd), "idleFor": rush_gate::godur::string(STUCK_AFTER)}),
            );
            if st.info.inbox {
                let _ = crate::inbox::tell(
                    &st.cfg.id,
                    crate::inbox::MAIN_NOTICE,
                    &stuck_note(&short(&cmd), STUCK_AFTER),
                );
            }
        }
        self.lock().w.stuck_seen = seen;
    }

    // ---- Long background tasks (longtask.go) ----

    /// Looks at the background tasks every LONG_LOOK while it has some.
    pub(super) fn arm_long_tasks(&self, st: &mut State) {
        if st.w.long_timer.is_some() || st.quit || st.info.background.is_empty() {
            return;
        }
        let h = self.clone();
        st.w.long_timer = Some(self.timers.after(LONG_LOOK, move || {
            let h2 = h.clone();
            h.go("long-tasks", move |_| {
                h2.check_long_tasks(Time::now());
                let mut st = h2.lock();
                st.w.long_timer = None;
                if st.conn.is_some() {
                    h2.arm_long_tasks(&mut st);
                }
            });
        }));
    }

    /// Its background tasks it may be asked about: not a subagent's, which
    /// looks after its own, nor one known to have ended that the agent's
    /// list still has.
    pub(super) fn askable(&self, st: &State) -> Vec<Task> {
        st.info
            .background
            .iter()
            .filter(|t| t.parent.is_empty() && !st.not_asked.contains(&t.id))
            .cloned()
            .collect()
    }

    /// Asks the agent about its long-running background tasks.
    pub(super) fn check_long_tasks(&self, now: Time) {
        let (tmp, sid, ids) = {
            let st = self.lock();
            let ids: Vec<String> = st.info.background.iter().map(|t| t.id.clone()).collect();
            (
                crate::tmp::temp_dir(&st.cfg.id),
                st.info.session_id.clone(),
                ids,
            )
        };
        let wrote = last_output(&task_dirs(&tmp, &sid), &ids);
        let mut st = self.lock();
        // Not while it asks you something: the message would answer it.
        // Nor at a usage limit: it can't answer, and each try is a failed
        // turn.
        let mut due = Vec::new();
        let mut held = "";
        if st.conn.is_none() || st.info.sleeping {
        } else if st.info.state == "blocked" || !st.pending.is_empty() {
            held = "it's asking you something";
        } else if st.info.limit.is_some() {
            held = "it's at a usage limit";
        } else {
            due = overdue_tasks(&self.askable(&st), &wrote, &st.w.nudged, now);
        }
        if !held.is_empty()
            && overdue_tasks(&self.askable(&st), &wrote, &st.w.nudged, now).is_empty()
        {
            held = ""; // nothing it would be asked about anyway
        }
        if held != st.w.check_in_held {
            st.w.check_in_held = held.to_string();
            if !held.is_empty() {
                self.event(&st, "check-in-held", json!({"why": held}));
            }
        }
        for t in &due {
            st.w.nudged.insert(t.id.clone(), now);
        }
        if due.is_empty() {
            return;
        }
        st.w.nudged.insert(LONG_TASK_LAST.into(), now);
        drop(st);
        let err = self
            .guide_text(&long_task_note(&due, &wrote, now))
            .err()
            .unwrap_or_default();
        let st = self.lock();
        self.event(
            &st,
            "check-in",
            json!({"why": "long background tasks", "tasks": due, "err": err}),
        );
    }

    // ---- A message the agent dropped (heard.go) ----

    /// Keeps the message just sent, until the agent is seen to take it.
    pub(super) fn sent(&self, st: &mut State, input: &Input) {
        st.w.sends += 1;
        st.w.unheard = Some(Unheard {
            input: input.clone(),
            at: rush_core::time::now(),
            again: false,
            n: st.w.sends,
        });
    }

    /// Looks again at a message the agent ended a turn without reading (no
    /// request made, nothing said). A Claude Code just resumed on a turn a
    /// restart cut off can end that turn and drop what came in as it
    /// started: unless its transcript has the message, it goes again,
    /// once.
    pub(super) fn check_heard(
        &self,
        st: &mut State,
        conn: &std::sync::Arc<dyn Conn>,
        e: &event::TurnEnd,
    ) {
        let Some(u) = st.w.unheard.clone() else {
            return;
        };
        if u.again
            || conn.hearer().is_none()
            || e.turns != 0
            || !e.err.is_empty()
            || !e.text.trim().is_empty()
        {
            return;
        }
        let (h, conn) = (self.clone(), conn.clone());
        let wait = self.hooks.heard_wait;
        self.go("heard", move |_| {
            // Long enough for it to have written it, had it taken it.
            {
                let st = h.lock();
                let _ = h.cv.wait_timeout_while(st, wait, |s| !s.quit);
            }
            let Some(heard) = conn.hearer().and_then(|r| r.heard(&u.input.text, u.at)) else {
                return; // it can't tell
            };
            let mut st = h.lock();
            let same = st.w.unheard.as_ref().is_some_and(|x| x.n == u.n && !x.again);
            if heard
                || !super::same_conn(&st.conn, &Some(conn.clone()))
                || !same
                || st.info.state != "idle"
                || st.info.sleeping
            {
                return;
            }
            if let Some(x) = &mut st.w.unheard {
                x.again = true;
            }
            let ev = rush_events::id();
            st.turn += 1;
            st.info.turn = st.turn;
            st.turn_ev = ev.clone();
            let f = json!({"ev": ev, "turn": st.turn, "text": clip(&u.input.text), "images": u.input.images.len(), "why": "the agent ended a turn without reading it"});
            h.event(&st, "redelivered", f);
            h.disarm_idle(&mut st);
            if let Err(err) = conn.send(u.input.clone()) {
                let f = json!({"ev": ev, "text": clip(&u.input.text), "err": err.to_string()});
                h.event(&st, "deliver-failed", f);
                h.arm_idle(&mut st);
                return;
            }
            st.info.state = "working".into();
            st.info.detail.clear();
            st.waiting = Some(Instant::now());
            h.publish(&mut st);
        });
    }

    // ---- What it left behind (left.go), where it works (edits.go) ----

    /// Adds to the ledger what m's tool calls write outside the project
    /// and scratch, whichever agent made them.
    pub(super) fn note_left(&self, st: &mut State, m: &event::Message) {
        let tmp = crate::tmp::temp_dir(&st.cfg.id)
            .to_string_lossy()
            .into_owned();
        let (cwd, harness) = (st.cfg.cwd.clone(), st.cfg.account.dir.clone());
        let w = &mut st.w;
        let found = crate::left::left_by(m, &cwd, &tmp, &harness, |c| {
            // Remembered for the session's folder: git runs once a folder,
            // not for every call.
            if w.proj_for != c {
                w.proj_for = c.to_string();
                w.proj = crate::tmp::project_of(c);
            }
            w.proj.clone()
        });
        for l in found {
            let list = std::mem::take(&mut st.info.left);
            st.info.left = crate::left::add_left(list, l);
        }
    }

    /// Moves a session to the checkout its agent works in, when that isn't
    /// its own: EDITS_TO_MOVE edits, or shell commands that cd there, in a
    /// row. git runs off the lock.
    pub(super) fn follow_edits(&self, st: &mut State, m: &event::Message) {
        for p in &m.parts {
            let d = crate::left::worked_in(p.call.as_deref(), &st.cfg.cwd, &st.start_cwd);
            if !d.is_empty() && !crate::tmp::is_temp(&d) {
                st.w.edited.push(d);
                let n = st.w.edited.len();
                if n > crate::left::EDITS_TO_MOVE {
                    st.w.edited.drain(..n - crate::left::EDITS_TO_MOVE);
                }
            }
        }
        if st.w.edited.len() < crate::left::EDITS_TO_MOVE {
            return;
        }
        let (dirs, was) = (st.w.edited.clone(), st.cfg.cwd.clone());
        let h = self.clone();
        self.go("edits", move |_| {
            let dirs: Vec<String> = dirs.iter().map(|d| crate::left::nearest(d)).collect();
            let Some(top) = crate::left::edited_in(&was, &dirs, rush_sys::actions::repo_root)
            else {
                return;
            };
            let mut st = h.lock();
            if was != st.cfg.cwd {
                return;
            }
            st.w.edited.clear();
            st.cfg.cwd = top.clone();
            st.info.cwd = top;
            st.info.cwd_at = Time::now();
            h.save_config(&st);
            h.publish(&mut st);
        });
    }

    /// Looks at the folder the agent says it works in, at most every
    /// CWD_EVERY unless now. Entering a worktree moves it: the list then
    /// shows where it is, and a restart resumes it there. Only between
    /// turns, and only a folder it still says cwd_settle later: never a
    /// passing cd.
    pub(super) fn follow_cwd(&self, st: &mut State, now: bool) {
        if st.info.claude_pid == 0 || (!now && st.w.cwd_at.is_some_and(|t| t.elapsed() < CWD_EVERY))
        {
            return;
        }
        st.w.cwd_at = Some(Instant::now());
        let Some(a) = self.reg.get(&st.cfg.kind) else {
            return;
        };
        if a.cwd_reader().is_none() {
            return;
        }
        let (p, pid) = (st.cfg.account.clone(), st.info.claude_pid as i32);
        let settle = self.hooks.cwd_settle;
        let h = self.clone();
        self.go("cwd", move |_| {
            let Some(r) = a.cwd_reader() else { return };
            let Some((sid, cwd)) = r.session_cwd(&p, pid).filter(|(_, c)| !c.is_empty()) else {
                return;
            };
            {
                let st = h.lock();
                let _ = h.cv.wait_timeout_while(st, settle, |s| !s.quit);
            }
            if r.session_cwd(&p, pid) != Some((sid.clone(), cwd.clone())) {
                return; // it moved on, or back: no move
            }
            if !std::path::Path::new(&cwd).is_dir() {
                return;
            }
            let (was, from, idle) = {
                let st = h.lock();
                (
                    st.cfg.cwd.clone(),
                    st.start_cwd.clone(),
                    st.info.state == "idle",
                )
            };
            if !idle || crate::tmp::is_temp(&cwd) {
                return; // mid-turn its shell may be anywhere; scratch work is no move
            }
            if cwd == from && was != from {
                return; // its shell reset, not a move
            }
            let Some(to) = crate::left::follow_to(&was, &cwd, rush_sys::actions::repo_root) else {
                return;
            };
            let mut st = h.lock();
            if sid != st.info.session_id || was != st.cfg.cwd || st.info.state != "idle" {
                return;
            }
            st.cfg.cwd = to.clone();
            st.info.cwd = to.clone();
            st.info.cwd_at = Time::now();
            h.save_config(&st);
            h.publish(&mut st);
            drop(st);
            h.note_workspace(&to); // a worktree it went into: known by its branch from here
        });
    }

    // ---- What it was given at its start (startctx.go) ----

    /// Notes the prompt sections rush gives the start under way and reads
    /// the last start's, to compare once the agent says what it has.
    pub(super) fn begin_start(&self, st: &mut State, sections: Vec<usage::Section>) {
        st.w.last_start = std::fs::read(dir(&st.cfg.id).join(START_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        st.w.this_start = Some(usage::Start {
            at: rush_core::time::now(),
            sections,
            ..usage::Start::default()
        });
        st.info.context_change = None;
    }

    /// Takes the tools and MCP servers the agent said it has. Until its
    /// first count it's taken again from each init, as servers still
    /// connecting at the first have their tools by the next. The caller
    /// publishes.
    pub(super) fn start_init(&self, st: &mut State, e: &event::Init) {
        let Some(s) = &mut st.w.this_start else {
            return;
        };
        if s.parts.is_some() || e.tools.is_empty() {
            return;
        }
        let mut tools = e.tools.clone();
        tools.sort();
        let mut servers: Vec<String> = e.mcp.iter().map(|m| m.name.clone()).collect();
        servers.sort();
        if tools == s.tools && servers == s.servers {
            return;
        }
        s.tools = tools;
        s.servers = servers;
        self.note_start(st);
    }

    /// Takes the agent's first count by category; it publishes.
    pub(super) fn start_count(&self, st: &mut State, u: &usage::Context) {
        let Some(s) = &mut st.w.this_start else {
            return;
        };
        if s.parts.is_some() || u.parts.is_empty() {
            return;
        }
        s.parts = Some(usage::parts_of(u));
        self.note_start(st);
        self.publish(st);
    }

    /// Keeps this start for the next to compare with, and says what
    /// changed from the last.
    fn note_start(&self, st: &mut State) {
        if let Some(s) = &st.w.this_start
            && let Ok(b) = serde_json::to_vec(s)
        {
            let _ = crate::json::write_atomic(&dir(&st.cfg.id).join(START_FILE), &b);
        }
        st.info.context_change = usage::diff(st.w.last_start.as_ref(), st.w.this_start.as_ref());
    }

    // ---- The turn's answer (subtool.go) ----

    /// Keeps the latest words the agent itself (not a subagent of its)
    /// said, for the turn's answer and its decisions.
    pub(super) fn said(&self, st: &mut State, m: &event::Message) {
        if m.role != "assistant" || !m.parent.is_empty() {
            return;
        }
        for p in &m.parts {
            if p.kind == PartKind::Text && !p.text.trim().is_empty() {
                st.last_said = p.text.clone();
            }
        }
    }

    /// Keeps the turn's answer for whoever waits on it, and sends it to the
    /// session it reports to once there's nothing more to do. Called once
    /// on_turn_end has settled the state.
    pub(super) fn turn_done(&self, st: &mut State, e: &event::TurnEnd) {
        if st.cfg.meta.get("spawnedBy").is_none_or(|s| s.is_empty()) {
            st.last_said.clear(); // the next turn's words are its own
            return;
        }
        // A turn that ends with more to do (a queued message) is answered
        // with the next, not lost to it.
        let mut text = if st.last_said.is_empty() {
            e.text.clone()
        } else {
            st.last_said.clone()
        };
        if !st.w.owed.is_empty() {
            text = format!("{}\n\n{text}", st.w.owed).trim().to_string();
        }
        let mut a = Answer {
            at: Time::now(),
            text,
            ..Answer::default()
        };
        if !e.err.is_empty() && e.reason != "interrupted" {
            // A "done" can carry an error too (Claude's is_error).
            a.err = fail_report(&self.reg, &st.cfg.kind, &e.err, &st.last_step, 0);
        } else {
            st.last_step.clear(); // the next failure's last step is the next run's
        }
        st.last_said.clear();
        st.w.owed.clear();
        if let Ok(b) = serde_json::to_vec(&a) {
            let _ = crate::json::write_private(&answer_path(&st.cfg.id), &b);
        }
        if !finished(&self.reg, &st.cfg.id, &st.info, a.at).1 {
            if !st.info.queue.is_empty() {
                st.w.owed = a.text;
            }
            return;
        }
        self.report_answer(st);
    }

    /// Sends the last answer to the session it reports to, once there's
    /// nothing more to do, and only once.
    pub(super) fn report_answer(&self, st: &mut State) {
        let (a, done) = finished(&self.reg, &st.cfg.id, &st.info, Time::default());
        if !done {
            return;
        }
        let to = std::fs::read_to_string(report_path(&st.cfg.id)).unwrap_or_default();
        if to.is_empty() {
            return;
        }
        let _ = report(&st.cfg.id, "");
        let mut text = format!(
            "{} (agent {})",
            agent_name(&self.reg, &rush_core::adapter::migrated(&st.cfg.kind).0),
            st.cfg.id
        );
        if !a.err.is_empty() {
            text += &format!("{} finished, failing: {}", titled(&st.info.name), a.err);
        } else {
            let t = a.text.trim();
            let t = if t.is_empty() { "(it said nothing)" } else { t };
            text += &format!(" finished:\n\n{t}");
        }
        let (child, parent) = (st.cfg.id.clone(), to);
        st.w.reporting += 1; // the host doesn't sleep until it's gone (answer_given)
        let h = self.clone();
        self.go("report", move |_| {
            if let Err(e) = send_result(&child, &parent, &text) {
                eprintln!("rush: couldn't send the answer to {parent}: {e}");
            }
            h.lock().w.reporting -= 1;
        });
    }

    /// A message from rush into the turn under way, or the next, as Go's
    /// guide(text, nil).
    fn guide_text(&self, text: &str) -> Result<(), String> {
        self.guide_exchange(text, &[], None, "")
    }
}

/// Stops the agent's program and waits, a moment at most, for its process
/// to go: close can return before it has, and a wake mustn't start a
/// second one on the same conversation.
fn stop_resting_agent(conn: &std::sync::Arc<dyn Conn>) {
    let pid = conn.pid().unwrap_or(0);
    super::stop_agent(conn);
    if pid <= 0 || pid as u32 == std::process::id() {
        return;
    }
    if crate::sys::wait_exit(pid as i64, Duration::from_secs(1)) {
        return;
    }
    end_group(pid);
    crate::sys::wait_exit(pid as i64, Duration::from_secs(1));
}

/// Ends pid's process group when it leads one of its own (not ours), else
/// pid: asked, then made to a second on. pid 1 or below is never one
/// agent's: refused (pid 1 leads its own group, and kill(-1) would reach
/// every process the user has).
fn end_group(pid: i32) {
    if pid <= 1 {
        return;
    }
    // SAFETY: getpgid and getpgrp take and return plain integers.
    let group = unsafe { libc::getpgid(pid) == pid && pid != libc::getpgrp() };
    let signal = |sig| {
        let _ = if group {
            rush_sys::proc::kill_group(pid, sig)
        } else {
            rush_sys::proc::kill(pid, sig)
        };
    };
    signal(libc::SIGTERM);
    if crate::sys::wait_exit(pid as i64, Duration::from_secs(1)) {
        return;
    }
    signal(libc::SIGKILL);
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
