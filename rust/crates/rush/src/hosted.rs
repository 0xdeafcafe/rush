//! The TUI's sessions through rush's host module: what rush-ui's
//! session_core asks of its `Hosts` (dial a session's host and follow what
//! it says, send it messages and answers, start, resume, wake and adopt
//! sessions), answered by rush-host's client.
//!
//! Ported from internal/ui/rushmode.go (openHost :1866, applyHostLine
//! :1937, next :1958, startHosted :5341 — its naming and host.Spawn —
//! sendHostedID :5419, resume :5306, sendOffline's resume :5262,
//! moveToRushWith :5459 with its moved-to-rush events, cdHosted and
//! retitleHosted from picker.go:242 and rushmode.go:5440), sleep_host.go:65 (reconnectSleeping: Ensure,
//! Dial, SendImages), hosted.go:202 (stopTurn's Interrupt), localqueue.go:61
//! (reply), outside.go:19-22 and :42 (stopOutside with host.StopAll as of
//! e0f011a4, which moving one to rush mode stops it through,
//! rushmode.go:5513; replyOutside), machinecare.go:105-109 (a host's
//! Stop) and :318-320 (StartedBy, Meta's why), and setup.go:420-503
//! (handOverMessage's work: the conversation carried whole or told, the
//! new host spawned, its taking the history waited for, the source
//! stopped once nothing new reached it), handoff_ready.go:12-40
//! (handoffQuiet, waitCarried) and events.go:4 (clipLog). The cards' asks
//! (cards_host, carry.rs): rewindsheet.go (rewindHost :438, hangUp :463,
//! the branches the rewind sheet reads :100-105), compactsheet.go:256
//! (carry's CompactedIfUnchanged), forksheet.go:350-414 (start's config
//! and host.Spawn), handoff.go:80-95 (the hand-off's host.Spawn),
//! ollama.Keep and DecisionModels (#compact's pruner), room.go:177-200
//! (a room set up: each member's configAs and UseAgent, room.Create and
//! room.Start) and the client's DismissDecisions,
//! SetWithout and Ask; a control request's Reply and the context's count
//! as the UI takes them (rushmode.go applyHostLine).

use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_core::adapter::{Feature, Gone, Profile, Registry};
use rush_core::agent::Agent;
use rush_core::event::{Commands, Event};
use rush_host::lines::{Decoder, Line};
use rush_host::{Client, Config};
use rush_room::room::Room;
use rush_state::json::RawJson;
use rush_ui::screens::list_extras::away::{Away, Held};
use rush_ui::screens::session_cards::Mode;
use rush_ui::screens::session_cards::fork::Fork;
use rush_ui::screens::session_cards::rewind::Branch;
use rush_ui::screens::session_core::host::{
    Began, HandOver, HandedOver, Hosts, Info, Limit, Link, Now, Op, Retry, Said, SendErr, Spawned,
    Start, Started,
};
use serde_json::json;

/// How many lines a batch holds at most (rushmode.go next's 4096).
const BATCH: usize = 4096;

/// What starting and waking a host does: rush-host's own (a detached
/// `rush host run`), or a test's fake host in its own process.
#[derive(Clone, Copy)]
pub struct Starts {
    /// Writes a config and starts its host (host.Spawn).
    pub spawn: fn(&Registry, Config) -> Result<Config, String>,
    /// Wakes a saved host for an explicit action (host.Ensure).
    pub ensure: fn(&Registry, &str) -> Result<(), String>,
    /// A session's info as its host last wrote it (host.ReadInfo).
    pub info: fn(&str) -> Result<rush_host::Info, String>,
    /// Waits for a new session to take the history it carries
    /// (waitCarried).
    pub carried: fn(&str) -> Result<(), String>,
    /// Stops a session's host, keeping its conversation: whether it did.
    pub stop: fn(&str) -> bool,
}

fn spawn_real(_: &Registry, cfg: Config) -> Result<Config, String> {
    rush_host::spawn(cfg).map_err(|e| e.to_string())
}

fn ensure_real(_: &Registry, id: &str) -> Result<(), String> {
    rush_host::ensure(id, "").map_err(|e| e.to_string())
}

impl Default for Starts {
    fn default() -> Starts {
        Starts {
            spawn: spawn_real,
            ensure: ensure_real,
            info: info_real,
            carried: carried_real,
            stop: stop_real,
        }
    }
}

/// rush's hosts, as the TUI reaches them.
pub struct RushHosts {
    reg: Registry,
    starts: Starts,
}

impl RushHosts {
    /// The hosts of this machine, run on reg's agents.
    pub fn new(reg: Registry) -> RushHosts {
        RushHosts::with(reg, Starts::default())
    }

    /// Hosts started and woken by starts (tests: rush-host's fake host).
    pub fn with(reg: Registry, starts: Starts) -> RushHosts {
        RushHosts { reg, starts }
    }

    fn dial_id(&self, id: &str) -> Result<Arc<dyn Link>, String> {
        let c = Client::dial(id).map_err(|e| e.to_string())?;
        Ok(Arc::new(HostLink::new(c)))
    }

    /// The config a's host keeps, or what the list says of it (resume's
    /// fallback).
    fn config_of(&self, a: &Agent) -> Config {
        rush_host::read_config(&a.job.id).unwrap_or_else(|_| Config {
            id: a.job.id.clone(),
            session_id: a.job.session_id.clone(),
            account: a.acct.clone(),
            cwd: a.job.cwd.clone(),
            name: a.display_name.clone(),
            ..Config::default()
        })
    }
}

/// Whether rush-mode sessions start lean, as rush's settings say.
fn lean() -> bool {
    rush_state::Store::load().config.dispatch.quick_start()
}

impl Hosts for RushHosts {
    fn hosts(&self, a: &Agent) -> bool {
        a.rush
    }

    fn dial(&self, a: &Agent) -> Result<Arc<dyn Link>, String> {
        self.dial_id(&a.job.id)
    }

    fn reply(&self, a: &Agent, text: &str) -> Result<(), SendErr> {
        if a.rush {
            return self
                .send_to(&a.job.id, text, false)
                .map_err(SendErr::Failed);
        }
        let ad = self.reg.get(a.kind.as_str());
        let Some(r) = ad.as_ref().and_then(|ad| ad.replier()) else {
            let h = self.reg.harness_label(a.kind.as_str());
            return Err(SendErr::Failed(format!(
                "{h} takes no messages from outside it"
            )));
        };
        r.reply(&a.acct, &a.job.id, text).map_err(|e| {
            if e.downcast_ref::<Gone>().is_some() {
                SendErr::Gone
            } else {
                SendErr::Failed(e.to_string())
            }
        })
    }

    fn resume(&self, a: &Agent, text: &str, images: &[String]) -> Result<(), String> {
        let mut cfg = self.config_of(a);
        cfg.resume = true;
        cfg.prompt = text.to_string();
        cfg.images = images.to_vec();
        cfg.lean = lean();
        (self.starts.spawn)(&self.reg, cfg).map(|_| ())
    }

    fn adopt(&self, a: &Agent, text: &str) -> Result<Started, String> {
        if a.job.session_id.is_empty() {
            return Err(format!(
                "can't find {}'s conversation to resume",
                a.display_name
            ));
        }
        let mut cfg = Config {
            session_id: a.job.session_id.clone(),
            resume: true,
            prompt: text.to_string(),
            account: a.acct.clone(),
            cwd: a.job.cwd.clone(),
            name: a.display_name.clone(),
            ..Config::default()
        };
        if a.kind.as_str() == rush_state::LOGINS_KIND {
            // Dispatch's own settings are this agent's; another carries on
            // with its own model and mode.
            let d = rush_state::Store::load().config.dispatch;
            cfg.model = d.model.clone();
            cfg.effort = d.effort.clone();
            cfg.permission_mode = d.permission.clone();
            cfg.limit_mode = d.on_limit.clone();
            cfg.lean = d.quick_start();
        }
        cfg.use_agent(&self.reg, a.kind.as_str())?;
        let old = a.key.as_str();
        let how = if a.interactive {
            // Its terminal keeps the original; rush carries on with a copy.
            cfg.fork = true;
            cfg.from = a.job.session_id.clone();
            "copy"
        } else {
            if (a.pid != 0 || a.job.live())
                && let Err(e) = stop_outside(&self.reg, a)
            {
                rush_host::events::log(
                    "moved-to-rush",
                    json!({"from": old, "how": "move", "err": e}),
                );
                return Err(e);
            }
            "move"
        };
        let (from, prompt) = (cfg.from.clone(), clip_log(&cfg.prompt));
        let c = match (self.starts.spawn)(&self.reg, cfg) {
            Ok(c) => c,
            Err(e) => {
                rush_host::events::log("moved-to-rush", json!({"from": old, "how": how, "err": e}));
                return Err(e);
            }
        };
        let said = if a.interactive {
            json!({"from": old, "how": how, "conversation": from})
        } else {
            json!({"from": old, "how": how, "conversation": c.session_id, "prompt": prompt})
        };
        rush_host::events::log_event(&c.id, "moved-to-rush", said);
        Ok(Started {
            id: c.id,
            name: c.name,
            acct: c.account.name,
        })
    }

    fn cd(&self, id: &str, dir: &str, retitle: bool) -> Result<(), String> {
        let c = Client::dial(id).map_err(|e| e.to_string())?;
        // Best effort: an older host follows its cds instead, and ignores
        // a retitle.
        let _ = c.cd(dir);
        if retitle {
            let _ = c.retitle(dir);
        }
        c.close();
        Ok(())
    }

    fn can_start(&self) -> bool {
        true
    }

    fn start(&self, s: &Start) -> Result<Started, String> {
        let mut name = s.name.clone();
        let from = rush_host::name::name_from(&s.prompt);
        let mut name_first = from.is_empty();
        if name.is_empty() {
            name = from;
            if name.is_empty()
                && let Some(img) = s.images.first()
            {
                let base = std::path::Path::new(img)
                    .file_name()
                    .map(|b| b.to_string_lossy().into_owned())
                    .unwrap_or_default();
                name = format!("about {base}");
                name_first = false;
            }
            if name.is_empty() {
                name = rush_host::name::fresh_name(&s.cwd);
            }
        }
        let mut cfg = start_config(s);
        cfg.name = name;
        cfg.name_first = name_first;
        cfg.use_agent(&self.reg, &s.kind)?;
        let c = (self.starts.spawn)(&self.reg, cfg)?;
        Ok(Started {
            id: c.id,
            name: c.name,
            acct: c.account.name,
        })
    }

    fn hand_over(&self, h: &HandOver) -> Result<HandedOver, String> {
        let s = &h.start;
        let whole = self.reg.supports(&s.kind, Feature::Port);
        let mut cfg = start_config(s);
        cfg.name = s.name.clone();
        cfg.use_agent(&self.reg, &s.kind)?;
        let (from, to) = (h.from.as_str(), s.kind.as_str());
        let mut retire = h.retire;
        let mut stamp = None;
        if retire {
            let info = (self.starts.info)(from)?;
            if !quiet(&info) {
                return Err(
                    "source session is busy or has queued messages; switch after it finishes"
                        .into(),
                );
            }
            stamp = Some(info.updated_at);
        }
        let c = &h.conv;
        if whole {
            cfg.carry = Some(c.history.iter().map(carry_line).collect());
            if c.history.is_empty() {
                // There is no history to confirm; the source stays.
                retire = false;
            }
            let sp = format!("{}\n\n{}", cfg.system_prompt, self.reg.carried(c));
            cfg.system_prompt = sp.trim().to_string();
        } else {
            let input = self.reg.handoff(c);
            if !rush_ui::screens::session_cards::handoff::told_of(c) {
                // Nothing of it could be told to carry on: the source is
                // the only place it still is.
                retire = false;
            }
            cfg.prompt = input.text;
            cfg.images = input.images;
        }
        if !h.message.is_empty() {
            if whole {
                cfg.prompt = h.message.clone();
            } else {
                cfg.prompt += &format!("\n\nThe user's next instruction:\n{}", h.message);
            }
        }
        let account = s.account.as_ref().map(|a| a.0.clone()).unwrap_or_default();
        let new = match (self.starts.spawn)(&self.reg, cfg) {
            Ok(c) => c,
            Err(e) => {
                rush_host::events::log("handed-over", json!({"from": from, "to": to, "err": e}));
                return Err(e);
            }
        };
        if retire
            && whole
            && let Err(e) = (self.starts.carried)(&new.id)
        {
            rush_host::events::log_event(
                &new.id,
                "handed-over",
                json!({"from": from, "to": to, "whole": whole, "retired": false,
                    "err": format!("not confirmed: {e}")}),
            );
            return Err(format!(
                "destination {} did not confirm the handoff; source kept available: {e}",
                new.id
            ));
        }
        if retire {
            // Another client may have sent the source work while the new
            // one was starting: then it's left as it is.
            retire = match (self.starts.info)(from) {
                Ok(i) if quiet(&i) && Some(i.updated_at) == stamp => (self.starts.stop)(from),
                _ => false,
            };
        }
        rush_host::events::log_event(
            &new.id,
            "handed-over",
            json!({"from": from, "to": to, "account": account, "whole": whole,
                "retired": retire, "message": clip_log(&h.message)}),
        );
        Ok(HandedOver {
            started: Started {
                id: new.id,
                name: new.name,
                acct: new.account.name,
            },
            retired: retire,
        })
    }

    fn sleeping(&self, a: &Agent) -> bool {
        rush_host::read_info(&a.job.id).is_ok_and(|i| i.sleeping)
    }

    fn saved(&self, a: &Agent) -> Option<Info> {
        // openSavedHost (sleep_host.go:172-190).
        rush_host::read_info(&a.job.id).ok().map(|i| info(&i))
    }

    fn stop_all(&self, id: &str) -> Result<(), String> {
        rush_host::client::stop_all(id).map_err(|e| e.to_string())
    }

    fn restart(&self, id: &str, name: &str, text: &str) -> Result<(), String> {
        // restart's run (logins.go:715-736).
        let info = rush_host::list().into_iter().rfind(|i| i.id == id);
        let Some(info) = info.filter(|i| i.state != "stopped") else {
            return Err(format!("{name} isn't running; a message starts it"));
        };
        let c = Client::dial(id).map_err(|e| e.to_string())?;
        let r = rush_ui::screens::accounts::logins::restart_claude(&c, &info, text);
        c.close();
        r
    }

    fn stop(&self, id: &str) -> Result<(), String> {
        // Gone already: stopped (machinecare.go:105-109).
        let Ok(c) = Client::dial(id) else {
            return Ok(());
        };
        let r = c.stop().map_err(|e| e.to_string());
        c.close();
        r
    }

    fn wake(
        &self,
        a: &Agent,
        text: &str,
        images: &[String],
        now: bool,
    ) -> Result<Arc<dyn Link>, String> {
        (self.starts.ensure)(&self.reg, &a.job.id)?;
        let c = Client::dial(&a.job.id).map_err(|e| e.to_string())?;
        if !text.is_empty() || !images.is_empty() {
            c.send_images(text, images, now)
                .map_err(|e| e.to_string())?;
        }
        Ok(Arc::new(HostLink::new(c)))
    }

    fn send_to(&self, id: &str, text: &str, now: bool) -> Result<(), String> {
        (self.starts.ensure)(&self.reg, id)?;
        let c = Client::dial(id).map_err(|e| e.to_string())?;
        // Once the replay's in (it ends with the host's info), sent, and
        // taken: a connection let go while the host still writes to it
        // can take unread ops with it, so the send waits to see its echo,
        // or it queued (as host.Deliver does).
        let wait = std::time::Duration::from_secs(10);
        let e = |e: rush_host::ClientError| e.to_string();
        rush_host::client::await_line(&c, wait, |l| matches!(l, Line::Info(_))).map_err(e)?;
        if now {
            c.send_now(text).map_err(e)?;
        } else {
            c.send(text).map_err(e)?;
        }
        let mut failed = None;
        rush_host::client::await_line(&c, wait, |l| match l {
            Line::Error(s) => {
                failed = Some(s.clone());
                true
            }
            Line::Sent(s) => s.text == text,
            Line::Info(i) => i.queue.iter().any(|q| q == text),
            _ => false,
        })
        .map_err(e)?;
        c.close();
        failed.map_or(Ok(()), Err)
    }

    fn takes_mid_turn(&self, kind: &str) -> bool {
        rush_host::inbox::takes_mid_turn(&self.reg, kind)
    }

    fn spawned(&self, parent: &str) -> Vec<Spawned> {
        hosted_runs(&self.reg, &rush_host::list(), parent)
    }

    fn rewind(
        &self,
        id: &str,
        session_id: &str,
        resume: bool,
        left: &Branch,
    ) -> Result<bool, String> {
        // A host from an older rush doesn't know how: it's restarted on
        // this one, already rewound (rewindHost).
        if rush_host::read_info(id).is_ok_and(|i| i.proto < 1) {
            return rush_host::client::rewind_by_restart(id, session_id, resume, branch(left))
                .map(|()| true)
                .map_err(|e| e.to_string());
        }
        hang_up(id, |c| c.rewind(session_id, resume, branch(left))).map(|()| false)
    }

    fn compacted(
        &self,
        id: &str,
        session_id: &str,
        prompt: &str,
        left: &Branch,
        expected: &Info,
    ) -> Result<(), String> {
        let ns = expected.now.updated_ns;
        let exp = rush_host::Info {
            proto: expected.proto,
            session_id: expected.now.session_id.clone(),
            updated_at: rush_state::Time::from_unix(
                ns.div_euclid(1_000_000_000) as i64,
                ns.rem_euclid(1_000_000_000) as u32,
            ),
            ..rush_host::Info::default()
        };
        hang_up(id, |c| {
            c.compacted_if_unchanged(session_id, prompt, branch(left), &exp)
        })
    }

    fn branches(&self, a: &Agent) -> Vec<Branch> {
        let Ok(cfg) = rush_host::read_config(&a.job.id) else {
            return Vec::new();
        };
        let kind = if cfg.account.kind.is_empty() {
            cfg.kind.clone()
        } else {
            cfg.account.kind.to_string()
        };
        let Some(br) = self.reg.get(&kind) else {
            return Vec::new();
        };
        let Some(br) = br.brancher() else {
            return Vec::new();
        };
        cfg.branches
            .iter()
            .rev()
            .filter(|b| {
                let p = br.transcript_path(&cfg.account, &cfg.cwd, &b.session_id);
                std::fs::metadata(p).is_ok()
            })
            .map(|b| Branch {
                session_id: b.session_id.clone(),
                left: ms(b.left),
                from: b.from.max(0) as u32,
                turns: b.turns.max(0) as usize,
                last: b.last.clone(),
            })
            .collect()
    }

    fn fork(&self, a: &Agent, f: &Fork) -> Result<Started, String> {
        let disp = rush_state::Store::load().config.dispatch;
        let d = disp.resolved_for(a.kind.as_str());
        // "As now" is what it runs with, or what its host was started with.
        let was = rush_host::read_config(&a.job.id).unwrap_or_default();
        let now = rush_host::read_info(&a.job.id).unwrap_or_default();
        let first = |v: [&str; 4]| {
            v.into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or("")
                .to_string()
        };
        let cfg = Config {
            account: f.acct.clone(),
            cwd: f.cwd.clone(),
            name: f.name.clone(),
            resume: true,
            model: first([&f.model, &now.model, &was.model, &d.model]),
            effort: first([&f.effort, &now.effort, &was.effort, &d.effort]),
            permission_mode: first([&f.perm, &now.permission_mode, &was.permission_mode, &d.mode]),
            limit_mode: first([&was.limit_mode, &disp.on_limit, "", ""]),
            flags: was.flags.clone(),
            lean: disp.quick_start(),
            prompt: f.prompt.clone(),
            session_id: f.session_id.clone(),
            fork: f.fork,
            from: f.from.clone(),
            ..Config::default()
        };
        let c = (self.starts.spawn)(&self.reg, cfg)?;
        Ok(Started {
            id: c.id,
            name: c.name,
            acct: c.account.name,
        })
    }

    fn handoff(&self, kind: &str, cwd: &str, name: &str, prompt: &str) -> Result<Started, String> {
        let mut cfg = Config {
            cwd: cwd.to_string(),
            name: name.to_string(),
            prompt: prompt.to_string(),
            ..Config::default()
        };
        cfg.use_agent(&self.reg, kind)?;
        let c = (self.starts.spawn)(&self.reg, cfg)?;
        Ok(Started {
            id: c.id,
            name: c.name,
            acct: c.account.name,
        })
    }

    fn keep(&self, model: &str, goal: &str, steps: &[String]) -> Result<Vec<f64>, String> {
        rush_ollama::Local::new(self.reg.clone())
            .keep(model, goal, steps)
            .map_err(|e| e.to_string())
    }

    fn decision_models(&self) -> Result<Vec<String>, String> {
        rush_ollama::shared(&self.reg)
            .decision_models()
            .map_err(|e| e.to_string())
    }

    fn room_member(&self, s: &Start) -> Result<RawJson, String> {
        let mut cfg = start_config(s);
        cfg.use_agent(&self.reg, &s.kind)?;
        RawJson::from_value(&cfg).map_err(|e| e.to_string())
    }

    fn open_room(&self, r: Room) -> Result<Room, String> {
        let rooms = rush_room::Rooms::new();
        let r = rooms.create(r, || rush_host::new_session_id().1)?;
        let exe = rush_sys::selfexe::path().map_err(|e| e.to_string())?;
        rooms.start(&exe, &r.id)?;
        Ok(r)
    }
}

/// A rewind's path as its host keeps it (host.Branch).
fn branch(b: &Branch) -> rush_host::Branch {
    rush_host::Branch {
        session_id: b.session_id.clone(),
        left: rush_state::Time::from_ms(b.left),
        from: b.from as i64,
        turns: b.turns as i64,
        last: b.last.clone(),
    }
}

/// How long a host is given to hang up once asked to switch (hangUp).
const HANG_UP_FOR: Duration = Duration::from_secs(15);

/// Asks session id's host to do something it hangs up after, and waits
/// until it has, or says why it won't (rewindsheet.go:465 hangUp).
fn hang_up(
    id: &str,
    run: impl FnOnce(&Client) -> Result<(), rush_host::ClientError>,
) -> Result<(), String> {
    let c = Client::dial(id).map_err(|e| e.to_string())?;
    let r = wait_hang_up(&c, run);
    c.close();
    r
}

fn wait_hang_up(
    c: &Client,
    run: impl FnOnce(&Client) -> Result<(), rush_host::ClientError>,
) -> Result<(), String> {
    run(c).map_err(|e| e.to_string())?;
    let until = Instant::now() + HANG_UP_FOR;
    let mut d = Decoder::default();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match c.lines.recv_timeout(left) {
            Ok(l) => {
                for l in d.decode(&l).unwrap_or_default() {
                    if let Line::Error(e) = l {
                        return Err(e);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {
                return Err("the agent's host didn't answer the rewind".into());
            }
        }
    }
}

/// The config a new session starts with, as the screens start it: its
/// name is the caller's (startHosted's, configAs).
fn start_config(s: &Start) -> Config {
    let mut cfg = Config {
        cwd: s.cwd.clone(),
        model: s.model.clone(),
        effort: s.effort.clone(),
        permission_mode: s.permission_mode.clone(),
        billing: s.billing.clone(),
        prompt: s.prompt.clone(),
        images: s.images.clone(),
        profile: s.profile.clone(),
        own_worktree: s.own_worktree,
        system_prompt: s.system_prompt.clone(),
        limit_mode: s.limit_mode.clone(),
        lean: s.lean,
        // rush's own (the cleanup agent): the list says so, and finds
        // it again after a reload (machinecare.go:320).
        started_by: s.started_by.clone(),
        ..Config::default()
    };
    if !s.why.is_empty() {
        cfg.meta.insert("why".into(), s.why.clone());
    }
    if let Some((name, dir)) = &s.account {
        cfg.account = Profile {
            kind: s.kind.as_str().into(),
            name: name.clone(),
            dir: dir.clone(),
        };
    }
    cfg
}

/// Whether a session is quiet enough to carry: active or queued work
/// isn't left behind by a copy taken before it ends (handoffQuiet).
fn quiet(i: &rush_host::Info) -> bool {
    rush_ui::screens::session_cards::handoff::handoff_quiet(
        &i.state,
        i.queue.len(),
        &i.needs,
        i.background.len(),
        i.retry.is_some(),
    )
}

/// One line of a conversation as a config carries it (agent.Line).
fn carry_line(l: &rush_core::agent::PreviewLine) -> rush_host::wire::CarryLine {
    rush_host::wire::CarryLine {
        role: l.role.clone(),
        text: l.text.clone(),
        at: rush_state::Time::from_ms(l.at),
        doing: l.doing.clone(),
    }
}

fn info_real(id: &str) -> Result<rush_host::Info, String> {
    rush_host::read_info(id).map_err(|e| e.to_string())
}

/// Waits for session id's host to take the history it carries: its
/// config's carry is cleared once its agent has started on it; an error,
/// or its stopping, says it didn't (waitCarried).
fn carried_real(id: &str) -> Result<(), String> {
    rush_ui::screens::session_cards::handoff::await_carried(
        || {
            let info = info_real(id)?;
            if !info.error.is_empty() {
                return Err(info.error);
            }
            if info.state == "stopped" {
                return Err("destination stopped".into());
            }
            let cfg = rush_host::read_config(id).map_err(|e| e.to_string())?;
            Ok(cfg.carry.is_none())
        },
        std::time::Duration::from_secs(30),
    )
}

/// Stops session id's host, keeping its conversation: whether it did.
fn stop_real(id: &str) -> bool {
    let Ok(old) = Client::dial(id) else {
        return false;
    };
    let stopped = old.stop().is_ok();
    old.close();
    stopped
}

/// text kept short for the log: its first 300 characters (clipLog).
fn clip_log(text: &str) -> String {
    match text.char_indices().nth(300) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

/// The sessions rush hosted for rush session parent's shell or its
/// spawn_agent calls, each with where its agent writes (spawns.go:215
/// hostedRuns). One whose transcript can't be found is left out.
fn hosted_runs(reg: &Registry, infos: &[rush_host::Info], parent: &str) -> Vec<Spawned> {
    let mut out = Vec::new();
    for i in infos {
        if i.meta.get("spawnedBy").map(String::as_str) != Some(parent) {
            continue;
        }
        let live = rush_ui::screens::list_extras::spawns::hosted_live(&i.state);
        if i.session_id.is_empty() || i.started_at.is_zero() {
            continue;
        }
        let Ok(mut cfg) = rush_host::read_config(&i.id) else {
            continue;
        };
        let k = rush_core::adapter::migrated(if cfg.kind.is_empty() {
            &i.kind
        } else {
            &cfg.kind
        });
        // A host going to sleep forgets its first message, and one started
        // again begins its clock again; its request exchange keeps both. A
        // revived one's prompt is rush's note to carry on, not what it was
        // asked.
        if rush_host::revive::is_rush_note(&cfg.prompt) {
            cfg.prompt.clear();
        }
        let mut born = i.started_at.to_ms();
        if let Some(e) = rush_host::exchange_journal::read_exchanges(&i.id)
            .into_iter()
            .find(|e| e.phase == "request")
        {
            if !e.text.is_empty() {
                cfg.prompt = e.text;
            }
            if e.at != 0 {
                born = e.at;
            }
        }
        let Some(transcript) = hosted_transcript(reg, k.as_str(), &cfg, &i.session_id) else {
            continue;
        };
        let name = reg
            .get(k.as_str())
            .map_or_else(|| k.0.clone(), |a| a.name());
        out.push(Spawned {
            id: i.id.clone(),
            step: i.meta.get("spawnStep").cloned().unwrap_or_default(),
            kind: k.0.clone(),
            name,
            prompt: cfg.prompt,
            model: if cfg.model.is_empty() {
                i.model.clone()
            } else {
                cfg.model
            },
            cwd: cfg.cwd,
            transcript,
            born,
            live,
        });
    }
    out
}

/// Where a hosted run's agent writes session sid: where its agent keeps
/// sessions, else where its own list has it (spawns.go:266
/// hostedSession).
fn hosted_transcript(reg: &Registry, k: &str, cfg: &Config, sid: &str) -> Option<String> {
    let mut t = reg.transcript_path(k, &cfg.account, &cfg.cwd, sid);
    if t.is_empty() && reg.reads_as_claude(k) {
        t = reg.transcript_path(rush_core::adapter::LEGACY_KIND, &cfg.account, &cfg.cwd, sid);
    }
    if !t.is_empty() {
        return Some(t);
    }
    let a = reg.get(k)?;
    let d = a.discoverer()?;
    let mut profiles = vec![cfg.account.clone()];
    profiles.extend(reg.profiles_of(&a));
    for p in &profiles {
        for f in d.live(p).into_iter().chain(d.past(p)) {
            if f.id == sid && !f.transcript.is_empty() {
                return Some(f.transcript);
            }
        }
    }
    None
}

/// Ends a's process the agent's own way, keeping its conversation; a rush
/// session stops with the agents it spawned (stopOutside, outside.go:19-22,
/// as rushmode.go:5513 moves one to rush mode).
fn stop_outside(reg: &Registry, a: &Agent) -> Result<(), String> {
    rush_ui::screens::cmdbar::outside::stop_outside(reg, a, |id| {
        rush_host::client::stop_all(id).map_err(|e| e.to_string())
    })
    .map_err(|e| {
        format!(
            "couldn't stop the {} copy: {e}",
            reg.harness_label(a.kind.as_str())
        )
    })
}

/// One connection to a session's host: its ops go over the socket, and
/// what it says is read a batch at a time.
struct HostLink {
    client: Mutex<Client>,
    lines: Mutex<Option<Receiver<Vec<u8>>>>,
}

impl HostLink {
    fn new(mut c: Client) -> HostLink {
        let (_, none) = sync_channel(0);
        let lines = std::mem::replace(&mut c.lines, none);
        HostLink {
            client: Mutex::new(c),
            lines: Mutex::new(Some(lines)),
        }
    }
}

impl Link for HostLink {
    fn op(&self, op: Op) -> Result<(), String> {
        let c = self.client.lock().unwrap_or_else(|e| e.into_inner());
        let r = match op {
            Op::Send { text, images, now } => c.send_images(&text, &images, now),
            Op::Guide { text, images } => c.send_guide(&text, &images),
            Op::Interrupt => c.interrupt(),
            Op::Allow { id, always } => c.allow(&id, None, always),
            Op::Deny { id, message } => c.deny(&id, &message, false),
            Op::EditQueued { i, was, text } => c.edit_queued(i as i64, &was, &text),
            Op::RemoveQueued { i, was } => c.remove_queued(i as i64, &was),
            Op::MoveQueued { i, was, to } => c.move_queued(i as i64, &was, to as i64),
            Op::MergeQueued { i, was } => c.merge_queued(i as i64, &was),
            Op::SendQueued { i, was } => c.send_queued(i as i64, &was),
            Op::SteerQueued { i, was } => c.steer_queued(i as i64, &was),
            Op::HoldQueue(on) => c.hold_queue(on),
            Op::QueueSeparately(on) => c.queue_separately(on),
            Op::Tell { sub, text } => c.tell(&sub, &text),
            Op::StopTask { id } => c.stop_task(&id),
            Op::Background { id } => c.background(&id),
            Op::Answer { id, input, always } => match rush_core::wire::RawJson::new(&input) {
                Ok(input) => c.allow(&id, Some(input), always),
                Err(e) => return Err(e.to_string()),
            },
            Op::Permission(mode) => c.set_permission_mode(&mode),
            Op::DismissDecisions(id) => c.dismiss_decisions(&id),
            Op::Without(w) => c.set_without(&w),
            Op::Ask { id, req } => match rush_core::wire::RawJson::new(&req) {
                Ok(req) => c.ask(&id, req),
                Err(e) => return Err(e.to_string()),
            },
            Op::Model(id) => c.set_model(&id),
            Op::Effort(e) => c.set_effort(&e),
            Op::ContinueAtReset(yes) => c.continue_at_reset(yes),
            Op::Away(a) => c.set_away(a.map(|a| wire_away(&a))),
        };
        r.map_err(|e| e.to_string())
    }

    fn follow(&self, each: &mut dyn FnMut(Vec<Said>) -> bool) {
        let Some(lines) = self.lines.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        let mut d = Decoder::default();
        // A batch is what's come in by the time the last was taken: a
        // busy session costs one batch per frame the UI draws, not one a
        // line.
        while let Ok(first) = lines.recv() {
            let mut batch = Vec::new();
            said(&mut d, &first, &mut batch);
            let mut n = 1;
            while n < BATCH {
                match lines.try_recv() {
                    Ok(l) => said(&mut d, &l, &mut batch),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if !batch.is_empty() {
                            each(batch);
                        }
                        return;
                    }
                }
                n += 1;
            }
            if !batch.is_empty() && !each(batch) {
                return;
            }
        }
    }

    fn close(&self) {
        self.client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .close();
    }
}

/// What one of the host's lines says to the Session (applyHostLine, the
/// UI's half of host.Decode): its events, the host's info, echoes, stamps
/// and answers; nothing for replies, context counts and native lines no
/// agent reads.
fn said(d: &mut Decoder, l: &[u8], out: &mut Vec<Said>) {
    let Ok(ls) = d.decode(l) else { return };
    for l in ls {
        out.push(match l {
            Line::Event(e) => Said::Event(e),
            Line::Info(i) => Said::Info(info(&i)),
            Line::Answered(id) => Said::Answered(id),
            Line::Error(e) => Said::Error(e),
            Line::Sent(s) => Said::Sent {
                text: s.text,
                images: s.images,
            },
            Line::Stamp(t) => Said::Stamp(t.to_ms()),
            Line::Commands(list) => Said::Event(Event::Commands(Commands { list })),
            Line::Reply { id, body, error } => Said::Reply {
                id,
                body: if !error.is_empty() {
                    Err(error)
                } else {
                    Ok(body
                        .and_then(|b| serde_json::from_str(b.get()).ok())
                        .unwrap_or(serde_json::Value::Null))
                },
            },
            Line::Context(c) => Said::Context(c),
            Line::Native(_) => continue,
        });
    }
}

/// The host's info as the Session keeps it (host.Info's queue, state and
/// what the conversation takes from it).
fn info(i: &rush_host::Info) -> Info {
    Info {
        queue: i.queue.clone(),
        queue_images: i.queue_images.clone(),
        queue_held: i.queue_held,
        queue_stuck: if i.queue_stuck.is_zero() {
            0
        } else {
            i.queue_stuck.to_ms()
        },
        queue_separate: i.queue_separate,
        permission_mode: i.permission_mode.clone(),
        state: i.state.clone(),
        inbox: i.inbox,
        model: i.model.clone(),
        agent_pid: i.claude_pid,
        proto: i.proto,
        error: i.error.clone(),
        sleeping: i.sleeping,
        billing: i.billing.clone(),
        exe_mod: ms(i.exe.mod_),
        login: i.login.clone(),
        limit: i.limit.as_ref().map(|l| Limit {
            resets_at: ms(l.resets_at),
            window: l.window.clone(),
            continue_: l.continue_,
            ask: l.ask,
        }),
        retry: i.retry.as_ref().map(|r| Retry {
            attempt: r.attempt,
            max: r.max,
            next: ms(r.next),
            gave_up: r.gave_up,
            why: r.why.clone(),
            offline: r.offline,
            proof: r.proof,
        }),
        cache_warm: ms(i.cache_warm),
        now: Box::new(Now {
            effort: i.effort.clone(),
            without: i.without.clone(),
            permission_modes: i
                .permission_modes
                .iter()
                .map(|p| Mode {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    description: p.description.clone(),
                })
                .collect(),
            decisions: i.decisions.clone(),
            session_id: i.session_id.clone(),
            updated_ns: if i.updated_at.is_zero() {
                0
            } else {
                i.updated_at.unix_nanos()
            },
        }),
        began: Box::new(Began {
            id: i.id.clone(),
            kind: i.kind.clone(),
            effort: i.effort.clone(),
            cwd: i.cwd.clone(),
            started_at: ms(i.started_at),
            started_by: i.started_by.clone(),
            spawned_by: i.meta.get("spawnedBy").cloned().unwrap_or_default(),
            started_from: i.meta.get("startedFrom").cloned().unwrap_or_default(),
            temp_dir: i.temp_dir.clone(),
            context_change: i.context_change.clone().map(Box::new),
        }),
        away: i.away.as_ref().map(|a| {
            Box::new(Away {
                from: ms(a.from),
                until: ms(a.until),
                every: a.every.as_millis() as i64,
                next: ms(a.next),
                nudges: a.nudges.max(0) as usize,
                looping: a.r#loop,
                ended: ms(a.ended),
                held: a
                    .held
                    .iter()
                    .map(|h| Held {
                        kind: h.kind.clone(),
                        text: h.text.clone(),
                        at: ms(h.at),
                    })
                    .collect(),
            })
        }),
    }
}

/// An away as the host takes it (client.go SetAway's host.Away).
fn wire_away(a: &Away) -> rush_host::wire::Away {
    let t = rush_state::Time::from_ms;
    rush_host::wire::Away {
        from: t(a.from),
        until: t(a.until),
        every: Duration::from_millis(a.every.max(0) as u64),
        next: t(a.next),
        nudges: a.nudges as i64,
        r#loop: a.looping,
        ended: t(a.ended),
        held: a
            .held
            .iter()
            .map(|h| rush_host::wire::Held {
                kind: h.kind.clone(),
                text: h.text.clone(),
                at: t(h.at),
            })
            .collect(),
    }
}

/// A host's time as the UI keeps it: 0 for none.
fn ms(t: rush_state::Time) -> rush_core::time::Ms {
    if t.is_zero() { 0 } else { t.to_ms() }
}

#[cfg(test)]
mod tests;
