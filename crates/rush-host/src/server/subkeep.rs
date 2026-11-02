//! Keeping a working subagent's prompt cache from lapsing while it waits
//! on a long call (Rust-only; Go has none). The host follows each
//! subagent from its agent's own events (its messages carry the call that
//! started it as their parent): when it last made a request, how long its
//! cache lives, how big its context is, and the calls it waits on. Just
//! before its cache would lapse, rush_core::subkeep decides, and a long
//! call is moved to the background through the agent's SubagentKeeper,
//! off the host's lock. Each step-in, and each time rush lets a cache go
//! where it could have stepped in, is a session event; the subagent's row
//! says so through the info's `subKept`.
//!
//! It also notes how long each shell command took (took.json), for the
//! PreToolUse hook to run a known-long one in the background from the
//! start (rush-gate's gate_hook, rush_core::subkeep::ahead).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use rush_core::event::{self, PartKind};
use rush_core::subkeep::{self, Doing, Step, Sub};
use rush_core::time::Ms;
use rush_core::tool;
use serde_json::json;

use super::{Host, State};
use crate::events::clip;
use crate::timer::TimerId;

/// What the subagent's row says once rush has moved its call.
pub const MOVED_NOTE: &str = "moved to background to keep its cache";

/// A subagent followed: the call that started it is its key.
#[derive(Clone, Debug, Default)]
struct Followed {
    /// When its last request was (its message came), how long its cache
    /// lives (0 until a request wrote it), and the tokens it sent.
    last: Ms,
    ttl: Ms,
    context: i64,
    steps: u32,
    /// Why its cache was let go this stretch, once said.
    let_said: bool,
}

/// A call a subagent waits on.
#[derive(Clone, Debug, Default)]
struct Open {
    sub: String,
    command: String,
    /// A shell command that runs while the subagent waits.
    shell: bool,
    at: Ms,
}

/// What a look decided for one subagent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decided {
    pub sub: String,
    pub step: Step,
    /// Its calls to move, for a Move.
    pub calls: Vec<String>,
    pub seen: Sub,
}

/// Every subagent of the session's being followed. Pure: the host hands
/// in its events and the time.
#[derive(Debug, Default)]
pub(crate) struct SubWatch {
    /// Whether the session keeps its subagents' caches: Settings ›
    /// Usage › Prompt cache, read as its agent starts, and its agent can.
    pub on: bool,
    /// The smallest context worth it and the most step-ins, as set.
    pub settings: rush_core::cachekeep::Settings,
    /// Where its commands' times are noted: the session's folder as its
    /// agent started.
    pub project: String,
    subs: HashMap<String, Followed>,
    calls: HashMap<String, Open>,
    /// The calls its agent can move (a foreground task of a shell).
    tasks: HashSet<String>,
    /// The calls moved, for their time once they end.
    moved: HashMap<String, Open>,
    pub timer: Option<TimerId>,
    pub busy: bool,
}

/// A shell command that ended, and how long it took.
pub(crate) type Took = (String, Ms);

impl SubWatch {
    /// Takes one of its agent's messages at now: says the shell commands
    /// it shows ending.
    pub fn message(&mut self, m: &event::Message, now: Ms) -> Vec<Took> {
        let mut ended = Vec::new();
        if m.parent.is_empty() {
            // The main conversation: an Agent call that returned ends its
            // subagent.
            for p in &m.parts {
                if p.kind == PartKind::ToolResult
                    && let Some(o) = &p.output
                {
                    self.finished(&o.call_id);
                }
            }
            return ended;
        }
        if m.role == "assistant" {
            let f = self.subs.entry(m.parent.clone()).or_default();
            f.last = now;
            f.let_said = false;
            if let Some(t) = &m.tokens {
                // As the main conversation's: any 5-minute write makes it
                // 5 minutes, the newest part lapsing first.
                if t.cache_write_5m > 0 {
                    f.ttl = subkeep_short();
                } else if t.cache_write_1h > 0 {
                    f.ttl = rush_core::subcache::LONG;
                }
                f.context = t.input + t.cache_read + t.cache_write_5m + t.cache_write_1h;
            }
            for p in &m.parts {
                if p.kind == PartKind::ToolCall
                    && let Some(c) = &p.call
                {
                    let shell = c.kind == tool::Kind::Shell && !c.input.background;
                    self.calls.insert(
                        c.id.clone(),
                        Open {
                            sub: m.parent.clone(),
                            command: c.input.command.clone(),
                            shell,
                            at: now,
                        },
                    );
                }
            }
            return ended;
        }
        for p in &m.parts {
            if p.kind == PartKind::ToolResult
                && let Some(o) = &p.output
            {
                self.tasks.remove(&o.call_id);
                if self.moved.contains_key(&o.call_id) {
                    continue; // its result now says it moved; its time comes at its end
                }
                if let Some(c) = self.calls.remove(&o.call_id)
                    && c.shell
                {
                    ended.push((c.command, now - c.at));
                }
            }
        }
        ended
    }

    /// A task its agent started: a subagent's shell call the turn waits
    /// on can be moved.
    pub fn task_started(&mut self, e: &event::TaskStarted) {
        if !e.background
            && e.kind == event::TaskKind::Shell
            && self.calls.get(&e.call_id).is_some_and(|c| c.shell)
        {
            self.tasks.insert(e.call_id.clone());
        }
    }

    /// A task its agent says ended: a moved call's time, or a subagent's
    /// end.
    pub fn task_done(&mut self, e: &event::TaskDone, now: Ms) -> Option<Took> {
        self.finished(&e.call_id);
        self.moved
            .remove(&e.call_id)
            .map(|c| (c.command, now - c.at))
    }

    fn finished(&mut self, call: &str) {
        if self.subs.remove(call).is_some() {
            self.calls.retain(|_, c| c.sub != call);
            self.tasks.retain(|id| self.calls.contains_key(id));
        }
    }

    /// The agent ended: nothing of its is running.
    pub fn clear(&mut self) {
        self.subs.clear();
        self.calls.clear();
        self.tasks.clear();
        self.moved.clear();
    }

    /// What a subagent is doing, by its open calls: all of them movable,
    /// or not.
    fn doing(&self, sub: &str) -> (Doing, Vec<String>) {
        let mut open: Vec<(&String, &Open)> =
            self.calls.iter().filter(|(_, c)| c.sub == sub).collect();
        if open.is_empty() {
            return (Doing::Working, Vec::new());
        }
        open.sort_by(|a, b| a.1.at.cmp(&b.1.at).then(a.0.cmp(b.0)));
        let ids: Vec<String> = open.iter().map(|(id, _)| (*id).clone()).collect();
        let movable = ids.iter().all(|id| self.tasks.contains(id));
        (
            Doing::Calling {
                call: ids[0].clone(),
                movable,
            },
            ids,
        )
    }

    /// When the next look is due: a minute before the soonest cache of a
    /// subagent waiting on a call lapses. None when nothing waits.
    pub fn due(&self, now: Ms) -> Option<Ms> {
        if !self.on {
            return None;
        }
        self.subs
            .iter()
            .filter(|(k, f)| f.ttl > 0 && f.last > 0 && self.calls.values().any(|c| &c.sub == *k))
            .map(|(_, f)| f.last + f.ttl - rush_core::cachekeep::LEAD)
            .filter(|&at| at + rush_core::cachekeep::LEAD > now)
            .min()
    }

    /// Looks at every subagent waiting on a call at now: what to do for
    /// each whose cache is about to lapse. A cache let go is said once a
    /// stretch.
    pub fn look(&mut self, now: Ms) -> Vec<Decided> {
        if !self.on {
            return Vec::new();
        }
        let mut keys: Vec<String> = self.subs.keys().cloned().collect();
        keys.sort();
        let mut out = Vec::new();
        for k in keys {
            let (doing, calls) = self.doing(&k);
            if doing == Doing::Working {
                continue;
            }
            let f = &self.subs[&k];
            let seen = Sub {
                last_request: f.last,
                ttl: f.ttl,
                context: f.context,
                doing,
                steps: f.steps,
            };
            let step = subkeep::decide(&self.settings, &seen, now);
            match &step {
                Step::Wait => continue,
                Step::Let(_) if f.let_said => continue,
                Step::Let(_) => {
                    if let Some(f) = self.subs.get_mut(&k) {
                        f.let_said = true;
                    }
                }
                Step::Move(_) => {}
            }
            out.push(Decided {
                sub: k,
                step,
                calls,
                seen,
            });
        }
        out
    }

    /// The step-in for sub went: these of its calls moved.
    pub fn stepped(&mut self, sub: &str, moved: &[String]) {
        if let Some(f) = self.subs.get_mut(sub) {
            f.steps += 1;
        }
        for id in moved {
            self.tasks.remove(id);
            if let Some(c) = self.calls.remove(id) {
                self.moved.insert(id.clone(), c);
            }
        }
    }
}

/// A short cache's life, as rush_core::subcache counts it.
fn subkeep_short() -> Ms {
    rush_core::subcache::SHORT
}

impl Host {
    /// Follows a message for the subagents' caches, and notes how long its
    /// shell commands took.
    pub(super) fn sub_message(&self, st: &mut State, m: &event::Message) {
        let now = rush_core::time::now();
        let ended = st.sk.message(m, now);
        if m.parent.is_empty() {
            for p in &m.parts {
                if p.kind == PartKind::ToolResult
                    && let Some(o) = &p.output
                {
                    st.info.sub_kept.remove(&o.call_id);
                }
            }
        }
        self.note_took(st, ended);
        self.arm_sub_keep(st);
    }

    pub(super) fn sub_task(&self, st: &mut State, ev: &event::Event) {
        let now = rush_core::time::now();
        match ev {
            event::Event::TaskStarted(e) => st.sk.task_started(e),
            event::Event::TaskDone(e) => {
                st.info.sub_kept.remove(&e.call_id);
                let t = st.sk.task_done(e, now);
                self.note_took(st, t.into_iter().collect());
            }
            _ => return,
        }
        self.arm_sub_keep(st);
    }

    fn note_took(&self, st: &State, ended: Vec<Took>) {
        if ended.is_empty() || st.sk.project.is_empty() {
            return;
        }
        let project = st.sk.project.clone();
        let now = rush_core::time::now();
        self.go("took", move |_| {
            let p = rush_state::took::path();
            for (cmd, took) in ended {
                rush_state::took::record(&p, &project, &cmd, took, now);
            }
        });
    }

    /// Arms the look at the subagents' caches for when the soonest is due.
    pub(super) fn arm_sub_keep(&self, st: &mut State) {
        if st.sk.busy || st.quit {
            return;
        }
        if let Some(t) = st.sk.timer.take() {
            self.timers.cancel(t);
        }
        let now = rush_core::time::now();
        let Some(at) = st.sk.due(now) else {
            return;
        };
        let wait = Duration::from_millis((at - now).max(1_000) as u64);
        let h = self.clone();
        st.sk.timer = Some(self.timers.after(wait, move || {
            let h2 = h.clone();
            h.go("sub-keep", move |_| h2.sub_keep());
        }));
    }

    /// Looks at the subagents' caches, and steps in where the rule says.
    fn sub_keep(&self) {
        let mut st = self.lock();
        st.sk.timer = None;
        let now = rush_core::time::now();
        let decided = st.sk.look(now);
        let conn = st.conn.clone();
        let kind = st.cfg.kind.clone();
        let model = st.info.model.clone();
        let mut moves = Vec::new();
        for d in decided {
            match &d.step {
                Step::Let(why) => {
                    let f = json!({
                        "agent": d.sub,
                        "why": why,
                        "context": d.seen.context,
                        "ttl": d.seen.ttl / 1000,
                        "steps": d.seen.steps,
                    });
                    self.event(&st, "subagent-cache-let", f);
                }
                Step::Move(_) => moves.push(d),
                Step::Wait => {}
            }
        }
        let Some(c) = conn.filter(|_| !moves.is_empty()) else {
            self.arm_sub_keep(&mut st);
            return;
        };
        st.sk.busy = true;
        let cmds: HashMap<String, String> = moves
            .iter()
            .flat_map(|d| d.calls.iter())
            .filter_map(|id| st.sk.calls.get(id).map(|o| (id.clone(), o.command.clone())))
            .collect();
        drop(st);
        let mut done = Vec::new();
        for d in moves {
            let mut moved = Vec::new();
            let mut err = String::new();
            for id in &d.calls {
                match c.subagent_keeper().map(|k| k.move_call(id)) {
                    Some(Ok(true)) => moved.push(id.clone()),
                    Some(Ok(false)) => {}
                    Some(Err(e)) => err = e.to_string(),
                    None => err = "its agent can't move a call".into(),
                }
            }
            done.push((d, moved, err));
        }
        let mut st = self.lock();
        st.sk.busy = false;
        for (d, moved, err) in done {
            let (step_tokens, lapse_tokens) = subkeep::worth(d.seen.ttl, d.seen.context);
            // Both in the context's input tokens' worth.
            let price = |input: f64| {
                self.reg
                    .price(
                        &kind,
                        &model,
                        &rush_core::agent::TokenUsage {
                            input: input as i64,
                            ..Default::default()
                        },
                    )
                    .unwrap_or(0.0)
            };
            let mut f = json!({
                "agent": d.sub,
                "calls": d.calls,
                "command": clip(d.calls.first().and_then(|id| cmds.get(id)).map_or("", String::as_str)),
                "context": d.seen.context,
                "ttl": d.seen.ttl / 1000,
                "sinceRequest": (now - d.seen.last_request) / 1000,
                "steps": d.seen.steps + 1,
                "costUsd": price(step_tokens),
                "savesUsd": price(lapse_tokens),
            });
            if moved.is_empty() {
                f["err"] = if err.is_empty() {
                    "its call had ended or wasn't found".into()
                } else {
                    err.into()
                };
                self.event(&st, "subagent-cache-keep-failed", f);
                continue;
            }
            st.sk.stepped(&d.sub, &moved);
            st.info.sub_kept.insert(d.sub.clone(), MOVED_NOTE.into());
            self.event(&st, "subagent-cache-kept", f);
        }
        self.publish(&mut st);
        self.arm_sub_keep(&mut st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::agent::TokenUsage;
    use rush_core::event::{Message, Part, TaskDone, TaskKind, TaskStarted};

    const MIN: Ms = 60_000;

    fn said(parent: &str, tokens: Option<TokenUsage>, calls: &[(&str, &str)]) -> Message {
        Message {
            role: "assistant".into(),
            parent: parent.into(),
            tokens,
            parts: calls
                .iter()
                .map(|(id, cmd)| Part {
                    kind: PartKind::ToolCall,
                    call: Some(Box::new(tool::Call {
                        id: (*id).into(),
                        name: "Bash".into(),
                        kind: tool::Kind::Shell,
                        input: tool::Input {
                            command: (*cmd).into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn result(parent: &str, role: &str, call: &str) -> Message {
        Message {
            role: role.into(),
            parent: parent.into(),
            parts: vec![Part {
                kind: PartKind::ToolResult,
                output: Some(Box::new(tool::Output {
                    call_id: call.into(),
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn wrote(n: i64) -> Option<TokenUsage> {
        Some(TokenUsage {
            cache_read: n,
            cache_write_5m: 500,
            ..Default::default()
        })
    }

    fn task(call: &str, background: bool) -> TaskStarted {
        TaskStarted {
            id: format!("b-{call}"),
            call_id: call.into(),
            kind: TaskKind::Shell,
            background,
            ..Default::default()
        }
    }

    fn watch() -> SubWatch {
        SubWatch {
            on: true,
            ..Default::default()
        }
    }

    /// A subagent's long test run: due a minute before its cache lapses,
    /// moved then, noted, and its time taken when it ends.
    #[test]
    fn a_long_call_is_moved_before_the_cache_lapses() {
        let mut w = watch();
        let t0 = 1_000 * MIN;
        w.message(
            &said("agent-1", wrote(100_000), &[("tu-1", "cargo test")]),
            t0,
        );
        assert_eq!(w.due(t0), Some(t0 + 4 * MIN));
        // Not yet a task its agent can move: let go, said once.
        let at = t0 + 4 * MIN + 10_000;
        let d = w.look(at);
        assert_eq!(
            d.iter().map(|d| d.step.clone()).collect::<Vec<_>>(),
            [Step::Let("its call can't be moved to the background")]
        );
        assert!(w.look(at).is_empty(), "said once a stretch");
        // It returns, a fresh request, and the next call is a foreground
        // task now.
        let t1 = t0 + 5 * MIN;
        assert_eq!(
            w.message(&result("agent-1", "user", "tu-1"), t1 - 1_000),
            [("cargo test".to_string(), 5 * MIN - 1_000)]
        );
        w.message(
            &said("agent-1", wrote(100_000), &[("tu-2", "cargo bench")]),
            t1,
        );
        w.task_started(&task("tu-2", false));
        assert!(w.look(t1 + MIN).is_empty(), "not about to lapse");
        let d = w.look(t1 + 4 * MIN + 10_000);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].step, Step::Move("tu-2".into()));
        assert_eq!(d[0].calls, ["tu-2"]);
        w.stepped("agent-1", &d[0].calls);
        assert_eq!(w.subs["agent-1"].steps, 1);
        // Its result says it moved: no time yet; its end gives it.
        assert!(
            w.message(&result("agent-1", "user", "tu-2"), t1 + 5 * MIN)
                .is_empty()
        );
        let took = w.task_done(
            &TaskDone {
                call_id: "tu-2".into(),
                ..Default::default()
            },
            t1 + 12 * MIN,
        );
        assert_eq!(took, Some(("cargo bench".to_string(), 12 * MIN)));
    }

    #[test]
    fn what_ends_a_subagent_and_what_isnt_watched() {
        let cases: &[(&str, bool, i64)] = &[
            ("its Agent call returns", true, 100_000),
            ("off", false, 100_000),
        ];
        for &(name, on, ctx) in cases {
            let mut w = watch();
            w.on = on;
            let t0 = 1_000 * MIN;
            w.message(&said("agent-1", wrote(ctx), &[("tu-1", "make")]), t0);
            w.task_started(&task("tu-1", false));
            if on {
                assert!(w.due(t0).is_some(), "{name}");
                w.message(&result("", "user", "agent-1"), t0 + MIN);
            }
            assert_eq!(w.due(t0), None, "{name}");
            assert!(w.look(t0 + 4 * MIN + 10_000).is_empty(), "{name}");
        }
        // A quick shell call is timed as it ends; a background one never
        // is a task to move.
        let mut w = watch();
        w.message(&said("a", wrote(100_000), &[("tu-1", "ls")]), 0);
        w.task_started(&task("tu-1", true));
        assert!(w.tasks.is_empty());
        assert_eq!(
            w.message(&result("a", "user", "tu-1"), 2_000),
            [("ls".to_string(), 2_000)]
        );
    }
}
