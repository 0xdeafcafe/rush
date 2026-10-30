//! What rush says to a session gone quiet: a subagent run quiet past its
//! prompt cache is moved to the background and the session told to look
//! at what it did; a session silent at work is asked what it waits on.
//! After Go's internal/ui/stucksubs.go (nudgeStuckSubs :21, nudgeStuckSub
//! :72, askStuck :104), sent through the session's host (host.Dial).
//!
//! Beyond Go, from what went wrong in real use: only the rush that leads
//! nudges; a run the session didn't start itself (one another run
//! started) isn't the session's to wait on, so it isn't told of it; a run
//! a restart killed isn't a running tile at all (rush-fleet's loader);
//! a run waiting on its own background command waits as one in a tool
//! does (rush-fleet's stuck_subs); and the session is told rush moved the
//! run to the background only when its host did.

use std::collections::HashSet;
use std::sync::Arc;

use rush_core::agent::{Agent, SubagentTile};
use rush_core::time::Ms;
use rush_fleet::{Predicates, STUCK_SUB_AFTER};
use serde_json::json;

use crate::model::Model;
use crate::screens::session_core::host::{Hosts, Op, hosts_of};

/// What the session is told of its subagent what (its description, else
/// its type) gone quiet; moved, that rush moved the call it waits on to
/// the background.
pub fn nudge_text(what: &str, moved: bool) -> String {
    let mins = STUCK_SUB_AFTER / 60_000;
    if moved {
        return format!(
            "Your subagent {what:?} has written nothing for {mins} minutes. rush moved it to the background so you can go on: \
             look at what it has done so far. If it's good, continue; if not, stop it and do the work another way."
        );
    }
    format!(
        "Your subagent {what:?} has written nothing for {mins} minutes: \
         look at what it has done so far. If it's good, continue; if not, stop it and do the work another way."
    )
}

/// Why the nudge was sent, for the session's log.
pub fn nudge_why() -> String {
    format!(
        "its subagent wrote nothing for {} minutes",
        STUCK_SUB_AFTER / 60_000
    )
}

/// What rush says once it has nudged name; moved, as nudge_text.
pub fn nudge_done(name: &str, moved: bool) -> String {
    if moved {
        return format!("{name}'s subagent went quiet · moved to the background, {name} checks it");
    }
    format!("{name}'s subagent went quiet · {name} checks it")
}

/// What a session silent for quiet is asked, mid-turn, without stopping
/// it (askStuck).
pub fn ask_text(quiet: Ms) -> String {
    format!(
        "You have written nothing for {} minutes. What are you waiting on? \
         If a command or subagent hung, move it to the background or stop it, and carry on another way.",
        quiet / 60_000
    )
}

/// What rush says once it has asked name.
pub fn ask_done(name: &str) -> String {
    format!("asked {name} what it's waiting on")
}

/// The runs already told of, by agent key and run id, and whether a look
/// is under way (Go's subNudged, subChecking).
#[derive(Default)]
pub struct Nudged {
    told: HashSet<String>,
    checking: bool,
}

/// How telling a session of one quiet run went.
#[derive(Debug, PartialEq, Eq)]
enum Told {
    /// Told; moved, that its call went to the background.
    Yes {
        name: String,
        moved: bool,
    },
    /// Not its session's to wait on: another run started it.
    Nested,
    Failed(String),
}

fn told_key(key: &str, run: &str) -> String {
    format!("{key}\x00{run}")
}

/// Tells each rush session at work whose subagent run has newly gone
/// quiet for STUCK_SUB_AFTER to look at it, once per run
/// (nudgeStuckSubs). Only the rush that leads, as Go's skips following:
/// two rushes would tell it twice. Whether a quiet run is still in a
/// tool, and whose call started it, is read from transcripts, off the UI
/// thread.
pub fn nudge_stuck_subs(m: &mut Model) {
    if !m.leads() || m.kept::<Nudged>().checking {
        return;
    }
    let Some(hosts) = hosts_of(m) else {
        return;
    };
    let now = m.now;
    let told = m.kept.peek::<Nudged>().map(|n| &n.told);
    let mut ask: Vec<Agent> = Vec::new();
    for a in &m.agents {
        if !a.rush || a.past || a.done || !a.job.live() {
            continue;
        }
        let subs: Vec<SubagentTile> = a
            .subagents
            .iter()
            .filter(|s| {
                !s.tool_use_id.is_empty()
                    && s.mod_time != 0
                    && now - s.mod_time >= STUCK_SUB_AFTER
                    && !told.is_some_and(|t| t.contains(&told_key(&a.key, &s.id)))
            })
            .cloned()
            .collect();
        if !subs.is_empty() {
            let mut q = a.clone();
            q.subagents = subs;
            ask.push(q);
        }
    }
    if ask.is_empty() {
        return;
    }
    m.kept::<Nudged>().checking = true;
    m.later(
        move || {
            let mut out = Vec::new();
            for a in &ask {
                for s in a.stuck_subs(now) {
                    // A dry run moves nothing and sends nothing.
                    if rush_core::dryrun::would(
                        "nudge-stuck-sub",
                        &a.job.id,
                        &format!("subagent run {} quiet", s.id),
                    ) {
                        out.push((told_key(&a.key, &s.id), Told::Nested));
                        continue;
                    }
                    let t = nudge_one(&hosts, a, &s);
                    out.push((told_key(&a.key, &s.id), t));
                }
            }
            out
        },
        |m, out| {
            let n = m.kept::<Nudged>();
            n.checking = false;
            for (k, _) in &out {
                n.told.insert(k.clone());
            }
            for (_, t) in out {
                match t {
                    Told::Yes { name, moved } => m.news(&nudge_done(&name, moved), ""),
                    Told::Failed(e) => m.fail(&e),
                    Told::Nested => {}
                }
            }
            crate::frame::Out::Draw
        },
    );
}

/// Tells a's session its subagent run s went quiet, moving the call it
/// waits on to the background first (nudgeStuckSub). A run whose call
/// isn't in the session's own transcript was started by another run: the
/// session doesn't wait on it and can't move it, and that run can't be
/// reached, so nothing is sent. Waits on the host: off the UI thread.
fn nudge_one(hosts: &Arc<dyn Hosts>, a: &Agent, s: &SubagentTile) -> Told {
    let what = if s.description.is_empty() {
        &s.kind
    } else {
        &s.description
    };
    let mut f = json!({
        "subagent": s.id,
        "type": s.kind,
        "toolUseId": s.tool_use_id,
        "why": nudge_why(),
    });
    let log = |f: &serde_json::Value| {
        rush_host::events::log_event(&a.job.id, "stuck-subagent", f.clone());
    };
    if s.depth > 1 || !rush_fleet::calls_in(&a.job.transcript_path, &s.tool_use_id) {
        f["skipped"] = "started by another subagent, not the session".into();
        log(&f);
        return Told::Nested;
    }
    let link = match hosts.dial(a) {
        Ok(l) => l,
        Err(e) => {
            f["err"] = e.clone().into();
            log(&f);
            return Told::Failed(e);
        }
    };
    // Already in the background, or a host that can't: the note still
    // helps, worded without the move.
    let bg = link.op(Op::Background {
        id: s.tool_use_id.clone(),
    });
    let moved = bg.is_ok();
    f["backgroundErr"] = bg.err().unwrap_or_default().into();
    let text = nudge_text(what, moved);
    let sent = link
        .op(Op::Guide {
            text: text.clone(),
            images: Vec::new(),
        })
        .or_else(|_| {
            link.op(Op::Send {
                text,
                images: Vec::new(),
                now: false,
            })
        });
    link.close();
    if let Err(e) = sent {
        f["err"] = e.clone().into();
        log(&f);
        return Told::Failed(e);
    }
    f["moved"] = moved.into();
    log(&f);
    Told::Yes {
        name: a.display_name.clone(),
        moved,
    }
}
