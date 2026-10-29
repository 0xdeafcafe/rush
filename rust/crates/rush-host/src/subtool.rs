//! The agent tools for one rush session: each agent it starts is a rush
//! session of its own, its child by Meta spawnedBy, which the UI draws
//! under the call that started it. The child's host keeps its answer
//! (answer.json) and, when asked to (report), sends it to the parent as a
//! message once it's done. One start_agent starts is no child but a
//! sibling, by Meta startedFrom: top-level in the list, stopped and tidied
//! on its own, its answers kept but sent to no one unless asked. See
//! docs/subagents.md.
//!
//! The MCP server that offers these as tools (agtools) isn't ported yet:
//! SubAgents is what it calls.
//!
//! Ported from internal/host/subtool.go (all but said and turnDone, which
//! are in server/part2.rs).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rush_core::adapter::{Registry, migrated};
use rush_core::event::{Exchange, Peer};
use rush_core::failure;
use rush_state::Time;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agentmatch::{find, no_agent};
use crate::agents::{Pick, WORK_PROMPT, own_def, picks};
use crate::client::{deliver, read_config, read_info, spawn};
use crate::events::{clip, log_event};
use crate::paths::dir;
use crate::views::{View, drop_view, save_view};
use crate::wire::{Config, Info};

/// How long a waited call waits for its agent; past it, the agent works
/// on and agent_result asks again.
pub const FOREGROUND: Duration = Duration::from_secs(50 * 60);

/// What spawn_agent is asked.
#[derive(Clone, Debug, Default)]
pub struct SpawnInput {
    pub agent: String,
    pub model: String,
    pub effort: String,
    pub prompt: String,
    pub background: bool,
    /// The call that asked: the UI draws it there.
    pub step: String,
}

/// What start_agent is asked.
#[derive(Clone, Debug, Default)]
pub struct StartInput {
    pub agent: String,
    pub model: String,
    pub effort: String,
    pub prompt: String,
}

/// What agent_result is asked.
#[derive(Clone, Debug, Default)]
pub struct ResultInput {
    pub id: String,
    /// Seconds to wait, at most 600.
    pub wait: i64,
}

/// What agent_send is asked.
#[derive(Clone, Debug, Default)]
pub struct SendInput {
    pub id: String,
    pub prompt: String,
    pub background: bool,
}

/// The agent tools for the rush session parent, of kind.
#[derive(Clone)]
pub struct SubAgents {
    reg: Registry,
    parent: String,
    kind: String,
    headless: bool,
}

impl SubAgents {
    /// The agent tools for rush session id, or None when there's no
    /// session to start them from.
    pub fn of(reg: Registry, id: &str) -> Option<SubAgents> {
        if id.is_empty() {
            return None;
        }
        let cfg = read_config(id).ok()?;
        Some(SubAgents {
            reg,
            parent: id.into(),
            kind: migrated(&cfg.kind).0,
            headless: cfg.headless,
        })
    }

    /// The session they're the tools of.
    pub fn parent(&self) -> &str {
        &self.parent
    }

    /// Whether no one watches the session in rush (Config.headless).
    pub fn headless(&self) -> bool {
        self.headless
    }

    /// Whether p is one of the session's own subagent types (agent_defs),
    /// which its Agent tool starts rather than spawn_agent.
    fn own(&self, p: &Pick) -> bool {
        self.reg.reads_as_claude(&self.kind) && own_def(p, &self.kind)
    }

    fn leader(&self) -> String {
        self.reg.harness_of(&self.kind).0
    }

    /// Shows the agent's view of name in rush's panel.
    pub fn set_view(&self, name: &str, raw: &str) -> Result<String, String> {
        let mut v: View = serde_json::from_str(raw).map_err(|e| format!("its view: {e}"))?;
        if name.contains("--") {
            return Err("a view's name has no -- in it: those are plugins'".into());
        }
        v.name = name.into();
        v.by.clear();
        save_view(&self.parent, v)?;
        Ok(format!(
            "Shown beside the conversation as {name}. Set it again as things change; clear it once it's no use."
        ))
    }

    /// Puts the agent's view of name away among the past ones.
    pub fn clear_view(&self, name: &str) -> Result<(), String> {
        drop_view(&self.parent, name)
    }

    /// The agents spawn_agent can start, a line each.
    pub fn catalogue(&self) -> String {
        let mut lines = Vec::new();
        let mut own = Vec::new();
        for p in picks(&self.reg, &self.leader()) {
            if self.own(&p) {
                own.push(p.name);
                continue;
            }
            let mut line = format!("- {}", p.name);
            if !p.model.is_empty() && p.model != p.name {
                line += &format!(" ({})", p.model);
            }
            line += &format!(
                ": {} {} in {}.",
                p.desc,
                self.reg.provider_label(&p.r.prov),
                self.reg.harness_label(&p.r.k)
            );
            if !p.r.ready {
                line += " Not available right now.";
            }
            lines.push(line);
        }
        if !own.is_empty() {
            lines.push(format!(
                "Your own models ({}) aren't listed: they're your subagent types, for your Agent tool.",
                own.join(", ")
            ));
        }
        lines.join("\n")
    }

    /// Starts an agent on in.prompt, and waits for its answer unless it
    /// goes in the background.
    pub fn spawn(&self, input: &SpawnInput) -> Result<String, String> {
        if input.prompt.trim().is_empty() {
            return Err("spawn_agent needs a prompt: the whole task".into());
        }
        let (p, model) = self.choose(&input.agent, &input.model)?;
        let mut cfg = self.config(&p, &model, &input.prompt, &input.effort, false);
        if !input.step.is_empty() {
            cfg.meta.insert("spawnStep".into(), input.step.clone()); // the call that asked: the UI draws it there
        }
        self.use_agent(&mut cfg, &p.r.k)?;
        cfg.fill_ids();
        cfg.prompt_exchange = Some(Box::new(self.exchange(&cfg.id, "request", &input.prompt)));
        if input.background {
            report(&cfg.id, &self.parent).map_err(|e| e.to_string())?;
        }
        let since = Time::now();
        // Logged before its host starts, whose lines it causes.
        cfg.cause = rush_events::id();
        let _ = std::fs::create_dir_all(dir(&cfg.id)); // its own log is in its folder
        log_event(
            &cfg.id,
            "spawned",
            json!({"ev": cfg.cause, "by": "spawn_agent", "spawnedBy": self.parent, "agent": cfg.kind, "model": cfg.model, "background": input.background, "prompt": clip(&input.prompt)}),
        );
        let (id, cause) = (cfg.id.clone(), cfg.cause.clone());
        let started = match spawn(cfg) {
            Ok(c) => c,
            Err(e) => {
                log_event(
                    &id,
                    "spawn-failed",
                    json!({"by": "spawn_agent", "spawnedBy": self.parent, "agent": p.name, "err": e.to_string(), "cause": cause}),
                );
                return Err(format!("{} couldn't start: {e}", p.name));
            }
        };
        if input.background {
            return Ok(format!(
                "{} started in the background as agent {}. Its answer is sent to you as a message when it finishes; agent_result with this id asks sooner.",
                p.name, started.id
            ));
        }
        await_answer(&self.reg, &started.id, &self.parent, since, FOREGROUND)
    }

    /// Starts an agent on in.prompt as a session of its own beside the
    /// caller's, as if the user had started it, and says how to reach it.
    pub fn start(&self, input: &StartInput) -> Result<String, String> {
        if input.prompt.trim().is_empty() {
            return Err("start_agent needs a prompt: the whole task".into());
        }
        let (p, model) = self.choose(&input.agent, &input.model)?;
        let mut cfg = self.config(&p, &model, &input.prompt, &input.effort, true);
        self.use_agent(&mut cfg, &p.r.k)?;
        self.start_beside(cfg, &p.name)
    }

    /// Starts cfg as a session beside the caller's, and says how to reach
    /// it.
    pub(crate) fn start_beside(&self, mut cfg: Config, name: &str) -> Result<String, String> {
        cfg.fill_ids();
        cfg.prompt_exchange = Some(Box::new(self.exchange(&cfg.id, "request", &cfg.prompt)));
        // Logged before its host starts, whose lines it causes.
        cfg.cause = rush_events::id();
        let _ = std::fs::create_dir_all(dir(&cfg.id));
        log_event(
            &cfg.id,
            "spawned",
            json!({"ev": cfg.cause, "by": "start_agent", "from": self.parent, "agent": cfg.kind, "model": cfg.model, "prompt": clip(&cfg.prompt)}),
        );
        let (id, cause) = (cfg.id.clone(), cfg.cause.clone());
        match spawn(cfg) {
            Ok(started) => Ok(format!(
                "{name} started as session {}, beside yours in the user's list. Its answer isn't sent to you: agent_send with this id messages it, agent_result asks how it stands.",
                started.id
            )),
            Err(e) => {
                log_event(
                    &id,
                    "spawn-failed",
                    json!({"by": "start_agent", "from": self.parent, "agent": name, "err": e.to_string(), "cause": cause}),
                );
                Err(format!("{name} couldn't start: {e}"))
            }
        }
    }

    /// The agent a call names, on the model it runs, once it may take
    /// work.
    fn choose(&self, name: &str, model: &str) -> Result<(Pick, String), String> {
        let ps = picks(&self.reg, &self.leader());
        let Some((p, model)) = find(&self.reg, &ps, name, model) else {
            return Err(no_agent(&self.reg, name, model, &ps));
        };
        let st = rush_state::Store::load().config;
        if !model.is_empty()
            && !p.r.cell.is_empty()
            && !st.calls(&p.r.cell, self.reg.harness_of(&p.r.k).as_str(), &model)
        {
            return Err(format!(
                "{} may not call {model} in {}: Settings › Config keeps it to others",
                self.reg.provider_label(&p.r.prov),
                self.reg.harness_label(&p.r.k)
            ));
        }
        if !p.r.ready {
            return Err(format!(
                "{} needs its account checked or refreshed before taking work: in rush, Settings, {}",
                p.name, p.r.name
            ));
        }
        Ok((p, model))
    }

    /// A new session of p's on model, for the task in prompt, in the
    /// caller's folder with its trust: the caller's subagent, or with
    /// sibling a session of its own beside it, which works as one you
    /// started does.
    pub(crate) fn config(
        &self,
        p: &Pick,
        model: &str,
        prompt: &str,
        effort: &str,
        sibling: bool,
    ) -> Config {
        let st = rush_state::Store::load().config;
        let parent = read_config(&self.parent).unwrap_or_default();
        let info = read_info(&self.parent).unwrap_or_default();
        let start = st.dispatch.resolved_for(&p.r.k);
        let or = |a: &str, b: &str| {
            if a.is_empty() {
                b.to_string()
            } else {
                a.to_string()
            }
        };
        let mut cfg = Config {
            cwd: or(&info.cwd, &parent.cwd),
            prompt: prompt.into(),
            name: crate::name::name_from(prompt),
            model: or(model, &or(&p.model, &start.model)),
            effort: or(effort, &start.effort),
            permission_mode: start.mode.clone(),
            system_prompt: WORK_PROMPT.into(),
            ..Config::default()
        };
        if sibling {
            cfg.system_prompt.clear();
            cfg.meta.insert("startedFrom".into(), self.parent.clone());
        } else {
            cfg.meta.insert("spawnedBy".into(), self.parent.clone());
        }
        if migrated(&parent.kind).as_str() == p.r.k {
            cfg.permission_mode = or(&info.permission_mode, &cfg.permission_mode); // it works for its parent, with its trust
        } else if cfg.permission_mode.is_empty() {
            // On another harness it has the same trust in that harness's
            // words: a parent that doesn't ask doesn't hand you a child
            // that asks before every tool. A start mode set for it wins.
            let modes = self.reg.choices_of(&p.r.k).unwrap_or_default().modes;
            cfg.permission_mode = rush_core::adapter::trusted_as(&info.permission_mode, &modes);
        }
        if p.r.k == rush_state::LOGINS_KIND {
            let f = st.active_account();
            cfg.account = rush_core::adapter::Profile {
                kind: rush_core::adapter::Kind::from(rush_state::LOGINS_KIND),
                name: f.name,
                dir: f.config_dir,
            };
        }
        cfg
    }

    /// Readies cfg's agent k to run it, signed in.
    fn use_agent(&self, cfg: &mut Config, k: &str) -> Result<(), String> {
        cfg.use_agent(&self.reg, k)?;
        let _ = crate::kind::sign_in_if_out(&self.reg, k, &cfg.account);
        Ok(())
    }

    /// How agent in.id stands, waiting up to in.wait seconds for its
    /// answer.
    pub fn result(&self, input: &ResultInput) -> Result<String, String> {
        self.mine(&input.id)?;
        // Waited on here, its answer comes here: the message it would also
        // send is held back, and owed again only if this wait ends without
        // it.
        let to = std::fs::read_to_string(report_path(&input.id)).unwrap_or_default();
        let owed = to == self.parent;
        if owed {
            let _ = report(&input.id, "");
        }
        let wait = Duration::from_secs(input.wait.clamp(0, 600) as u64);
        let (out, done) = wait_for(&self.reg, &input.id, &self.parent, Time::default(), wait);
        if owed && !done && !report_path(&input.id).exists() {
            let _ = report(&input.id, &self.parent);
        }
        out
    }

    /// Sends agent in.id a follow-up, and waits for its answer unless it
    /// goes in the background.
    pub fn send(&self, input: &SendInput) -> Result<String, String> {
        self.mine(&input.id)?;
        // Only a background follow-up reports back; a waited one is
        // answered here.
        let to = if input.background {
            self.parent.as_str()
        } else {
            ""
        };
        report(&input.id, to).map_err(|e| e.to_string())?;
        let since = Time::now();
        deliver(
            &input.id,
            &self.exchange(&input.id, "message", &input.prompt),
            false,
        )
        .map_err(|e| e.to_string())?;
        let note = stuck_behind(&self.reg, &input.id);
        if !note.is_empty() {
            // Waited on, the call would wait out a turn that won't end for it.
            report(&input.id, &self.parent).map_err(|e| e.to_string())?;
            return Ok(note);
        }
        if input.background {
            return Ok("Sent. Its answer is sent to you as a message when it finishes.".into());
        }
        await_answer(&self.reg, &input.id, &self.parent, since, FOREGROUND)
    }

    /// Refuses an id that isn't one of the session's agents: its
    /// children, and the sessions it started beside it.
    fn mine(&self, id: &str) -> Result<(), String> {
        match read_config(id) {
            Ok(c)
                if c.meta.get("spawnedBy") == Some(&self.parent)
                    || c.meta.get("startedFrom") == Some(&self.parent) =>
            {
                Ok(())
            }
            _ => Err(format!(
                "no agent {} was started by this session",
                crate::reload::go_quote(id)
            )),
        }
    }

    /// The parent's message to its child, so the child's conversation says
    /// who it's from.
    pub fn exchange(&self, child: &str, phase: &str, text: &str) -> Exchange {
        Exchange {
            id: hex_id(),
            direction: "received".into(),
            phase: phase.into(),
            text: text.into(),
            sender: peer(&self.parent),
            receiver: peer(child),
            ..Exchange::default()
        }
    }
}

/// 16 random bytes in hex: an exchange's id.
fn hex_id() -> String {
    crate::paths::rand_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Rush session id as one end of an exchange.
pub fn peer(id: &str) -> Peer {
    let mut p = Peer {
        session_id: id.into(),
        ..Peer::default()
    };
    if let Ok(c) = read_config(id) {
        p.kind = migrated(&c.kind).0;
        p.name = c.name;
    }
    p
}

/// What a session's host keeps of its last turn (answer.json).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Answer {
    pub at: Time,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub err: String,
}

pub fn answer_path(id: &str) -> PathBuf {
    dir(id).join("answer.json")
}

pub fn report_path(id: &str) -> PathBuf {
    dir(id).join("report")
}

/// Has child send its answers to parent from now on; to none, "".
pub fn report(child: &str, parent: &str) -> std::io::Result<()> {
    if parent.is_empty() {
        return match std::fs::remove_file(report_path(child)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(dir(child))?;
    crate::json::write_private(&report_path(child), parent.as_bytes())
}

/// Waits up to wait for agent id to finish a turn ended after since (any,
/// when zero), and says how it stands.
pub fn await_answer(
    reg: &Registry,
    id: &str,
    parent: &str,
    since: Time,
    wait: Duration,
) -> Result<String, String> {
    wait_for(reg, id, parent, since, wait).0
}

/// await_answer, saying too whether the turn's answer was given (Go's
/// await). It waits on the agent's folder, where its host writes its info
/// and answer, rather than looking every half second as Go does; a look
/// every few seconds at most catches a host that died writing nothing.
fn wait_for(
    reg: &Registry,
    id: &str,
    parent: &str,
    since: Time,
    wait: Duration,
) -> (Result<String, String>, bool) {
    let name = match read_config(id) {
        Ok(c) => format!("{} ({id})", agent_name(reg, &migrated(&c.kind).0)),
        Err(_) => id.to_string(),
    };
    let w = rush_sys::fswait::Watcher::new();
    w.watch(&[dir(id)]);
    let deadline = Instant::now() + wait;
    loop {
        let info = match read_info(id) {
            Ok(i) => i,
            Err(e) => return (Err(format!("no agent {id}: {e}")), false),
        };
        let (a, done) = finished(reg, id, &info, since);
        if done && !a.err.is_empty() {
            let _ = report(id, ""); // answered here, so not as a message too
            return (
                Err(format!("{name}{} failed: {}", titled(&info.name), a.err)),
                true,
            );
        }
        if done {
            let _ = report(id, "");
            let t = a.text.trim();
            let t = if t.is_empty() { "(it said nothing)" } else { t };
            return (Ok(format!("{name} finished:\n\n{t}")), true);
        }
        if !info.sleeping && info.host_pid > 0 && !crate::alive(info.host_pid) {
            let why = if info.error.is_empty() {
                "its host went away"
            } else {
                &info.error
            };
            return (Err(format!("{name} stopped: {why}")), false);
        }
        if let Some(l) = info.limit.as_ref().filter(|l| l.continue_) {
            if let Err(e) = report(id, parent) {
                return (Err(e.to_string()), false);
            }
            return (
                Ok(format!(
                    "{name} hit a usage limit and carries on when it resets at {}. Its answer is sent to you as a message when it finishes.",
                    clock(l.resets_at)
                )),
                false,
            );
        }
        if info.state == "blocked" && !info.needs.is_empty() {
            return (
                Ok(format!(
                    "{name} is waiting for the user to allow {}. Ask again with agent_result and wait_seconds once they have.",
                    info.needs
                )),
                false,
            );
        }
        let now = Instant::now();
        if now >= deadline {
            let what = if info.detail.is_empty() {
                &info.state
            } else {
                &info.detail
            };
            return (
                Ok(format!(
                    "{name} is still working ({what}). Ask again with agent_result and wait_seconds."
                )),
                false,
            );
        }
        w.wait(None, Some((deadline - now).min(Duration::from_secs(5))));
    }
}

/// t as the local clock's 15:04.
fn clock(t: Time) -> String {
    let secs = t.unix_nanos().div_euclid(1_000_000_000) as libc::time_t;
    // SAFETY: localtime_r fills the tm it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// Agent id's last answer, when it ended a turn after since and has
/// nothing more to do: idle with nothing queued or running beside it, or
/// stopped by a limit or by errors it gave up retrying.
pub fn finished(reg: &Registry, id: &str, info: &Info, since: Time) -> (Answer, bool) {
    let a = std::fs::read(answer_path(id))
        .ok()
        .and_then(|b| serde_json::from_slice::<Answer>(&b).ok());
    let Some(mut a) = a else {
        return (Answer::default(), false);
    };
    if a.at.before(since) {
        return (a, false);
    }
    let retrying = info.retry.as_ref().is_some_and(|r| !r.gave_up);
    let continues = info.limit.as_ref().is_some_and(|l| l.continue_);
    if retrying || continues {
        return (a, false); // rush tries it again: that answer isn't its last
    }
    if info.limit.is_some() {
        a.err = "it hit a usage limit".into();
    } else if let Some(r) = info.retry.as_ref().filter(|r| r.gave_up) {
        let reason = if r.reason.is_empty() {
            "it gave up retrying"
        } else {
            &r.reason
        };
        a.err = fail_report(reg, &info.kind, reason, &r.last_step, r.attempt - 1);
    } else if info.state == "stopped" || info.sleeping {
    } else if info.state != "idle" || !info.queue.is_empty() || !info.background.is_empty() {
        return (a, false);
    }
    (a, true)
}

/// How a run failed, as the session or room it reports to is told: the
/// error (as failure::summary says it, when it's the provider's), the last
/// step the run took, the retries rush made, and for a login that stopped
/// working, how to sign in again.
pub fn fail_report(reg: &Registry, kind: &str, err: &str, step: &str, retries: i64) -> String {
    let k = failure::kind_of(err);
    let mut out = if k == failure::Kind::Fatal {
        first_line(err)
    } else {
        format!("provider error: {}", failure::summary(err))
    };
    if !step.is_empty() {
        out += &format!(". Its last step: {step}");
    }
    if retries > 0 {
        out += &format!(". rush retried it {retries} times");
    }
    if k == failure::Kind::Auth {
        out += &format!(". To sign in again, {}", login_hint(reg, &migrated(kind).0));
    }
    out
}

/// The first line of s, cut to 200 characters (host.go's firstLine).
fn first_line(s: &str) -> String {
    let s = s.trim();
    let s = s.split('\n').next().unwrap_or("");
    match s.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// How to sign agent k in again: its own command, where it has one, else
/// rush's settings.
fn login_hint(reg: &Registry, k: &str) -> String {
    reg.get(k)
        .and_then(|a| a.login_hinter().map(|h| h.login_hint()))
        .unwrap_or_else(|| "use rush's Settings → Providers".into())
}

/// A failed run's title, for the session it reports to to know which of
/// its runs it was: ` "Fix the login form"`, or nothing.
pub fn titled(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    format!(" \"{name}\"")
}

pub fn agent_name(reg: &Registry, k: &str) -> String {
    reg.get(k)
        .map(|a| a.name())
        .unwrap_or_else(|| k.to_string())
}

/// Gives parent child's answer as a message from child, waking parent if
/// it rests.
pub fn send_result(child: &str, parent: &str, text: &str) -> Result<(), String> {
    let e = Exchange {
        id: hex_id(),
        direction: "received".into(),
        phase: "result".into(),
        text: text.into(),
        sender: peer(child),
        receiver: peer(parent),
        ..Exchange::default()
    };
    deliver(parent, &e, false).map_err(|e| e.to_string())
}

/// What to tell the sender of a message to session id that went into its
/// queue behind a turn that can't take it: it waits for that turn to end,
/// or for the user to stop it. Empty when it went in (subtool.go:283).
pub fn stuck_behind(reg: &Registry, id: &str) -> String {
    let Ok(info) = read_info(id) else {
        return String::new();
    };
    if info.state != "working"
        || info.queue.is_empty()
        || crate::inbox::takes_mid_turn(reg, &info.kind)
    {
        return String::new();
    }
    let name = if info.name.is_empty() {
        id
    } else {
        info.name.as_str()
    };
    format!(
        "Queued, not delivered yet: {name} is mid-turn and can't take a message until that turn ends. \
Yours waits behind it; after a couple of minutes rush shows the user it's waiting, and they can stop the turn to send it. \
Its answer is sent to you as a message when it has one."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;
    use crate::wire::{Limit, Retry};

    // failreport_test.go:11
    #[test]
    fn fail_reports() {
        let reg = Registry::new();
        let r = fail_report(&reg, "claude", "boom\nmore", "Bash (ls)", 2);
        assert_eq!(r, "boom. Its last step: Bash (ls). rush retried it 2 times");
        assert!(titled("").is_empty() && titled("Fix") == " \"Fix\"");
    }

    #[test]
    fn finished_waits_for_a_retry_or_a_queue() {
        let _h = Home::new();
        let reg = Registry::new();
        let id = "answer01";
        std::fs::create_dir_all(dir(id)).unwrap();
        let a = Answer {
            at: Time::now(),
            text: "hi".into(),
            ..Answer::default()
        };
        std::fs::write(answer_path(id), serde_json::to_vec(&a).unwrap()).unwrap();
        let idle = Info {
            state: "idle".into(),
            ..Info::default()
        };
        assert!(finished(&reg, id, &idle, Time::default()).1);
        for info in [
            Info {
                state: "idle".into(),
                retry: Some(Retry::default()),
                ..Info::default()
            },
            Info {
                state: "idle".into(),
                limit: Some(Limit {
                    continue_: true,
                    ..Limit::default()
                }),
                ..Info::default()
            },
            Info {
                state: "idle".into(),
                queue: vec!["next".into()],
                ..Info::default()
            },
        ] {
            assert!(!finished(&reg, id, &info, Time::default()).1, "{info:?}");
        }
        let limited = Info {
            state: "idle".into(),
            limit: Some(Limit::default()),
            ..Info::default()
        };
        let (a, done) = finished(&reg, id, &limited, Time::default());
        assert!(done && a.err == "it hit a usage limit");
        // An answer from before since isn't this turn's.
        assert!(!finished(&reg, id, &idle, Time::now() + Duration::from_secs(5)).1);
        report(id, "p").unwrap();
        assert_eq!(std::fs::read_to_string(report_path(id)).unwrap(), "p");
        report(id, "").unwrap();
        report(id, "").unwrap();
        assert!(!report_path(id).exists());
    }

    fn echo() -> Pick {
        Pick {
            name: "echo".into(),
            r: crate::agents::Runner {
                k: "echo".into(),
                ready: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // subtool_test.go:140 (TestStartedBesideIsASibling), its config and
    // reach: an agent started beside its caller (start_agent) is a session
    // of its own, no parent link, which the caller can still ask and
    // message. (Its live run, Go's, needs the echo agent's binary.)
    #[test]
    fn started_beside_is_a_sibling() {
        let home = Home::new();
        let caller = "caller01";
        std::fs::create_dir_all(dir(caller)).unwrap();
        let cwd = home.path().to_string_lossy().into_owned();
        std::fs::write(
            dir(caller).join("config.json"),
            format!(r#"{{"id":"{caller}","kind":"echo","cwd":"{cwd}"}}"#),
        )
        .unwrap();
        let sa = SubAgents::of(Registry::new(), caller).unwrap();
        let cfg = sa.config(&echo(), "", "find the bug", "", true);
        assert!(
            !cfg.meta.contains_key("spawnedBy")
                && cfg.meta.get("startedFrom").map(String::as_str) == Some(caller)
                && cfg.system_prompt.is_empty()
                && cfg.cwd == cwd,
            "config {cfg:?}"
        );
        std::fs::create_dir_all(dir("beside01")).unwrap();
        std::fs::write(
            dir("beside01").join("config.json"),
            format!(r#"{{"id":"beside01","meta":{{"startedFrom":"{caller}"}}}}"#),
        )
        .unwrap();
        assert!(sa.mine("beside01").is_ok(), "the caller can't reach it");
        assert!(sa.mine("nobody01").is_err());
        assert!(
            sa.start(&StartInput::default())
                .unwrap_err()
                .contains("start_agent needs a prompt")
        );
    }

    // subtool_test.go:191 (TestSpawnedIsAChild): spawn_agent's agent is
    // still the caller's child, working for it.
    #[test]
    fn spawned_is_a_child() {
        let _home = Home::new();
        let sa = SubAgents {
            reg: Registry::new(),
            parent: "caller01".into(),
            kind: "echo".into(),
            headless: false,
        };
        let cfg = sa.config(&echo(), "", "find the bug", "", false);
        assert!(
            cfg.meta.get("spawnedBy").map(String::as_str) == Some("caller01")
                && !cfg.meta.contains_key("startedFrom")
                && cfg.system_prompt == WORK_PROMPT,
            "config {cfg:?}"
        );
    }
}
