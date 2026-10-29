//! The TUI's sessions through rush's host module: what rush-ui's
//! session_core asks of its `Hosts` (dial a session's host and follow what
//! it says, send it messages and answers, start, resume, wake and adopt
//! sessions), answered by rush-host's client.
//!
//! Ported from internal/ui/rushmode.go (openHost :1866, applyHostLine
//! :1937, next :1958, startHosted :5341 — its naming and host.Spawn —
//! sendHostedID :5419, resume :5306, sendOffline's resume :5262,
//! moveToRushWith :5459), sleep_host.go:65 (reconnectSleeping: Ensure,
//! Dial, SendImages), hosted.go:202 (stopTurn's Interrupt), localqueue.go:61
//! (reply), outside.go:19-22 and :42 (stopOutside with host.StopAll as of
//! e0f011a4, which moving one to rush mode stops it through,
//! rushmode.go:5513; replyOutside) and machinecare.go:105-109 (a host's
//! Stop) and :318-320 (StartedBy, Meta's why).

use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};

use rush_core::adapter::{Gone, Profile, Registry};
use rush_core::agent::Agent;
use rush_core::event::{Commands, Event};
use rush_host::lines::{Decoder, Line};
use rush_host::{Client, Config};
use rush_ui::screens::list_extras::away::{Away, Held};
use rush_ui::screens::session_core::host::{
    Began, Hosts, Info, Limit, Link, Op, Retry, Said, SendErr, Spawned, Start, Started,
};

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

    fn adopt(&self, a: &Agent, text: &str) -> Result<(), String> {
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
        if a.interactive {
            // Its terminal keeps the original; rush carries on with a copy.
            cfg.fork = true;
            cfg.from = a.job.session_id.clone();
        } else if a.pid != 0 || a.job.live() {
            stop_outside(&self.reg, a)?;
        }
        (self.starts.spawn)(&self.reg, cfg).map(|_| ())
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
        let mut cfg = Config {
            cwd: s.cwd.clone(),
            model: s.model.clone(),
            effort: s.effort.clone(),
            permission_mode: s.permission_mode.clone(),
            billing: s.billing.clone(),
            prompt: s.prompt.clone(),
            images: s.images.clone(),
            name,
            name_first,
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
        cfg.use_agent(&self.reg, &s.kind)?;
        let c = (self.starts.spawn)(&self.reg, cfg)?;
        Ok(Started {
            id: c.id,
            name: c.name,
            acct: c.account.name,
        })
    }

    fn sleeping(&self, a: &Agent) -> bool {
        rush_host::read_info(&a.job.id).is_ok_and(|i| i.sleeping)
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
        let r = restart_claude(&c, &info, text);
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
}

/// Stops a session's agent program and sends text, if there is any,
/// which starts a fresh one resuming the conversation: signed in as
/// whoever is in use now, with the settings and MCP servers as they are
/// now. Without text it starts again with the next message
/// (logins.go:672 restartClaude).
fn restart_claude(c: &Client, info: &rush_host::Info, text: &str) -> Result<(), String> {
    let r = (|| {
        let pid = info.claude_pid as libc::pid_t;
        if pid > 0 {
            // SAFETY: kill only signals.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            // SAFETY: as above; signal 0 only asks whether it's there.
            let gone = (0..50).any(|_| {
                let g = unsafe { libc::kill(pid, 0) } != 0;
                if !g {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                g
            });
            if !gone {
                // SAFETY: as above.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
        if text.is_empty() {
            return Ok(());
        }
        c.send(text).map_err(|e| e.to_string())
    })();
    rush_host::events::log_event(
        &info.id,
        "claude-restart",
        serde_json::json!({
            "claudePid": info.claude_pid,
            "text": rush_host::events::clip(text),
            "err": r.as_ref().err().cloned().unwrap_or_default(),
        }),
    );
    r
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
            Line::Reply { .. } | Line::Context(_) | Line::Native(_) => continue,
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
        began: Box::new(Began {
            id: i.id.clone(),
            kind: i.kind.clone(),
            effort: i.effort.clone(),
            cwd: i.cwd.clone(),
            started_at: ms(i.started_at),
            started_by: i.started_by.clone(),
            spawned_by: i.meta.get("spawnedBy").cloned().unwrap_or_default(),
            started_from: i.meta.get("startedFrom").cloned().unwrap_or_default(),
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

/// A host's time as the UI keeps it: 0 for none.
fn ms(t: rush_state::Time) -> rush_core::time::Ms {
    if t.is_zero() { 0 } else { t.to_ms() }
}

#[cfg(test)]
mod tests;
