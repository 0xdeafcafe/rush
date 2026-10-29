//! Stopping a task and moving one to the background: x (or ctrl+x) on a
//! picked task stops it, b moves one the turn waits on to the background,
//! ctrl+b moves whatever the turn waits on; on a running command picked
//! in the conversation b and x do the same; x on a subagent's row stops
//! that one alone. Each goes to the session's host (the stop_task and
//! background ops). After Go's internal/ui/jobs.go (jobKey :677,
//! pickedJob :745, stopJob :765, backgroundJob :785), shells.go
//! (pickedShell :114, shellKey's b and x :131-185), stopnote.go
//! (stoppable :48, stopPicked :60) and rushmode.go (paneKey's ctrl+b
//! :4003 and x on a run :4094-4099, pickedSub :6229, stopSub :6258).
//!
//! k and p on a running Bash call kill or pause the command of its chain
//! running now, and all it started (shells.go shellKey's k and p :160,
//! killPart :180, pausePart :224). A script held at a breakpoint
//! (scripts.go goOnFromBreak) isn't tracked per session yet, so p never
//! lets one go on.

use rush_convo::{Job, PartProc, RunningPart, Status, Subagent};

use super::host::Op;
use super::{conn, open_key};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::list_extras::spawns::SPAWN_PREFIX;

/// What's picked in the open session (c.sel).
fn sel(m: &Model) -> String {
    m.session
        .as_ref()
        .map(|v| v.sel.clone())
        .unwrap_or_default()
}

/// The task picked, in the dock or the background view; a subagent
/// picked in the dock counts as its task (pickedJob).
pub fn picked_job(m: &Model, key: &str) -> Option<Job> {
    let sel = sel(m);
    let s = &m.session.as_ref().filter(|v| v.key == key)?.sess;
    if let Some(id) = sel.strip_prefix("job:") {
        return s.job(id).cloned();
    }
    let id = sel.strip_prefix("run:")?;
    let r = &m.kept.peek::<super::Core>()?.conns.get(key)?.runs;
    let sa = r.subs.iter().find(|sa| sa.id == id)?;
    s.running_jobs()
        .find(|j| !j.tool_use_id.is_empty() && j.tool_use_id == sa.tool_use_id || j.id == sa.id)
        .cloned()
}

/// The tool call id of the running Bash call picked: its task in the dock
/// or the background view, or its step in the conversation; "" for none
/// (pickedShell).
pub fn picked_shell(m: &Model, key: &str) -> String {
    if let Some(j) = picked_job(m, key) {
        let shell = m
            .session
            .as_ref()
            .is_some_and(|v| v.sess.job_kind(&j) == "shell");
        if j.running() && shell {
            return j.tool_use_id;
        }
    }
    let sel = sel(m);
    let Some((_, id)) = sel.split_once(":s:") else {
        return String::new();
    };
    let id = id.split(":line:").next().unwrap_or(id); // a line of its script
    let Some(s) = m.session.as_ref().map(|v| &v.sess) else {
        return String::new();
    };
    match s.step_by_call(id).map(|i| s.step(i)) {
        Some(st) if st.status == Status::Running && &*st.tool == "Bash" => id.to_string(),
        _ => String::new(),
    }
}

/// The run you're on: the one watched, or the one picked in the dock or
/// the subagents view; and whether it still works (pickedSub).
pub fn picked_sub(m: &Model, key: &str) -> Option<(Subagent, bool)> {
    let watching = super::subview::watching(m, key);
    let sel = sel(m);
    let c = m.kept.peek::<super::Core>()?.conns.get(key)?;
    let id = if watching {
        c.watch.open.as_str()
    } else {
        sel.strip_prefix("run:")
            .or_else(|| sel.strip_prefix("sub:"))?
    };
    let s = &m.session.as_ref().filter(|v| v.key == key)?.sess;
    let r = &c.runs;
    let now = m.now;
    let sa = r.subs.iter().find(|sa| sa.id == id)?.clone();
    let live = r.state(s, &sa, &s.subagent_jobs(), now).1;
    Some((sa, live))
}

/// Sends op to session key's host off the UI thread; a failure says why
/// (hostCmd).
fn send(m: &mut Model, key: &str, op: Op) {
    let Some(link) = conn(m, key).link.clone() else {
        return;
    };
    super::later(
        m,
        move || link.op(op),
        |m, r| match r {
            Ok(()) => Out::Idle,
            Err(e) => {
                m.fail(&e);
                Out::Draw
            }
        },
    );
}

/// Stops task j (stopJob).
pub fn stop_job(m: &mut Model, key: &str, j: &Job) -> Out {
    if !j.running() {
        m.flash("that has already finished");
        return Out::Draw;
    }
    if !conn(m, key).hosted() {
        m.flash_err("rush can stop a task only in a session it runs; this one is Claude Code's");
        return Out::Draw;
    }
    let what = m.session.as_ref().map_or("task", |v| v.sess.job_kind(j));
    if j.background {
        m.flash(&format!("stopping the {what} · the rest carries on"));
    } else {
        m.flash(&format!(
            "stopping the {what} · Claude hears it was stopped and carries on"
        ));
    }
    send(m, key, Op::StopTask { id: j.id.clone() });
    Out::Draw
}

/// Sends a task the turn waits on into the background (None: every one),
/// so the turn carries on and the agent hears when it's done
/// (backgroundJob).
pub fn background_job(m: &mut Model, key: &str, j: Option<&Job>) -> Out {
    let c = conn(m, key);
    if !c.hosted() {
        return Out::Pass;
    }
    if c.info.proto < 3 {
        m.flash_err(
            "this session's host is older than this rush · /restart it to move tasks to the background",
        );
        return Out::Draw;
    }
    let id = match j {
        Some(j) => {
            let what = m.session.as_ref().map_or("task", |v| v.sess.job_kind(j));
            m.flash(&format!(
                "moved the {what} to the background · the turn carries on"
            ));
            j.tool_use_id.clone()
        }
        None => {
            m.flash("moved what the turn was waiting on to the background");
            String::new()
        }
    };
    send(m, key, Op::Background { id });
    Out::Draw
}

/// Stops run sa alone: the turn and the other runs carry on. One rush
/// hosts for the session's shell is a session of its own to stop
/// (stopSub).
pub fn stop_sub(m: &mut Model, key: &str, sa: &Subagent, live: bool) -> Out {
    if !live {
        m.flash("that subagent has already finished");
        return Out::Draw;
    }
    if !conn(m, key).hosted() {
        m.flash_err(
            "rush can stop a subagent only in a session it runs; this one is Claude Code's",
        );
        return Out::Draw;
    }
    m.flash(&format!("stopping {} · the turn carries on", sa.typ));
    let step = sa.id.strip_prefix(SPAWN_PREFIX).unwrap_or(&sa.id);
    let hosted = conn(m, key)
        .runs
        .spawns
        .get(step)
        .map(|r| r.hosted.clone())
        .unwrap_or_default();
    if !hosted.is_empty() {
        let h = super::host::hosts(m);
        super::later(
            m,
            move || h.stop(&hosted),
            |m, r| match r {
                Ok(()) => Out::Idle,
                Err(e) => {
                    m.fail(&e);
                    Out::Draw
                }
            },
        );
        return Out::Draw;
    }
    // A harness's own child: by the call that spawned it.
    send(
        m,
        key,
        Op::StopTask {
            id: step.to_string(),
        },
    );
    Out::Draw
}

/// Whether the turn waits on a command now: ctrl+b is the dock's then,
/// else /btw's.
fn waits_on_command(m: &mut Model, key: &str) -> bool {
    let Model { session, kept, .. } = m;
    let Some(v) = session.as_ref().filter(|v| v.key == key) else {
        return false;
    };
    let r = kept.get::<super::Core>().conns.get(key).map(|c| &c.runs);
    v.sess
        .running_jobs()
        .any(|j| !j.background && super::running::job_kind(r, &v.sess, j) != "subagent")
}

/// The processes still running a command of a chain, each with its start
/// as the system has it (the conversation keeps it to the millisecond):
/// one gone, or its pid reused, is left out.
fn live_parts(procs: &[PartProc]) -> Vec<crate::screens::projects_machine::shells::PartProc> {
    procs
        .iter()
        .filter_map(|p| {
            let start = rush_sys::proc::started(p.pid)?;
            let ms = start
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as rush_core::time::Ms);
            (ms == p.start)
                .then_some(crate::screens::projects_machine::shells::PartProc { pid: p.pid, start })
        })
        .collect()
}

/// Runs work off the UI thread; a failure says why (hostCmd).
pub(super) fn off_ui(m: &mut Model, work: impl FnOnce() -> Result<(), String> + Send + 'static) {
    super::later(m, work, |m, r| match r {
        Ok(()) => Out::Idle,
        Err(e) => {
            m.fail(&e);
            Out::Draw
        }
    });
}

/// The command of call id's chain running now (RunningPart).
fn running_part(m: &Model, key: &str, id: &str) -> Option<RunningPart> {
    m.session
        .as_ref()
        .filter(|v| v.key == key)
        .and_then(|v| v.sess.running_part(id))
}

/// Ends the command of call id's chain that runs now, and all it
/// started, leaving the shell to go on as it would after a failure
/// (killPart).
pub fn kill_part(m: &mut Model, key: &str, id: &str) -> Out {
    let Some(rp) = running_part(m, key, id) else {
        m.flash_err("rush hasn't seen which command of it runs yet · x stops the whole call");
        return Out::Draw;
    };
    let f: Vec<&str> = rp.command.split_whitespace().collect();
    let what = if f.len() > 3 {
        format!("{} …", f[..3].join(" "))
    } else {
        rp.command.clone()
    };
    let then = match rp.then.as_str() {
        "stops" => "the chain stops there (&&) and Claude hears it failed",
        "carries on" => "the chain carries on with the next command",
        "ends" => "it was the last command, so the call ends",
        _ => "",
    };
    m.flash(&format!("killing {what} · {then}"));
    let procs = rp.procs;
    off_ui(m, move || {
        let errs: Vec<String> = live_parts(&procs)
            .into_iter()
            .filter_map(|p| {
                rush_sys::actions::end_tree(p.pid, p.start, std::time::Duration::from_secs(3))
                    .err()
                    .map(|e| e.to_string())
            })
            .collect();
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("\n"))
        }
    });
    Out::Draw
}

/// Pauses the command of call id's chain that runs now, and all it
/// started, or lets one paused go on (pausePart).
pub fn pause_part(m: &mut Model, key: &str, id: &str) -> Out {
    use crate::screens::projects_machine::shells as sh;
    // A script held at a breakpoint goes on from there (scripts.go:203).
    if let Some(o) = super::scripts::go_on_from_break(m, key, id) {
        return o;
    }
    if let Some(pp) = sh::take_paused(id) {
        let took = (m.now - pp.at).max(0);
        m.flash(&format!(
            "resumed {} after {}",
            pp.what,
            crate::style::dur((took + 500) / 1000 * 1000)
        ));
        off_ui(m, move || sh::signal_parts(&pp.procs, libc::SIGCONT));
        return Out::Draw;
    }
    let Some(rp) = running_part(m, key, id) else {
        m.flash_err("rush hasn't seen which command of it runs yet");
        return Out::Draw;
    };
    let what = crate::screens::list_extras::jobs::first_word(&rp.command).to_string();
    let procs = live_parts(&rp.procs);
    sh::note_paused(
        id,
        sh::PausedPart {
            at: m.now,
            what: what.clone(),
            procs: procs.clone(),
        },
    );
    // The agent's own timeout (Claude Code's 2m by default) keeps counting
    // while it's paused; nothing here can stop that clock.
    m.flash(&format!(
        "paused {what} · p lets it go on · the agent's timeout still counts"
    ));
    off_ui(m, move || sh::signal_parts(&procs, libc::SIGSTOP));
    Out::Draw
}

/// b and x on a running command picked in the conversation do what they
/// do on its task, and k and p on a running Bash call picked anywhere
/// kill or pause the command of its chain running now (shellKey). Pass
/// on anything else.
fn shell_key(m: &mut Model, key: &str, s: &str, empty: bool) -> Out {
    if !empty || !matches!(s, "b" | "x" | "k" | "p") {
        return Out::Pass;
    }
    let id = picked_shell(m, key);
    if id.is_empty() {
        return Out::Pass;
    }
    match s {
        "k" => return kill_part(m, key, &id),
        "p" => return pause_part(m, key, &id),
        _ => {}
    }
    if !sel(m).contains(":s:") {
        return Out::Pass; // the task's own keys
    }
    let job = m.session.as_ref().and_then(|v| {
        v.sess
            .running_jobs()
            .filter(|j| j.tool_use_id == id)
            .last()
            .cloned()
    });
    if !conn(m, key).hosted() {
        m.flash_err(
            "rush can background or stop a call only in a session it runs · k still kills the command running now",
        );
        return Out::Draw;
    }
    match (s, job) {
        ("b", Some(j)) if j.background => {
            m.flash("that's already in the background");
            Out::Draw
        }
        ("b", Some(j)) => background_job(m, key, Some(&j)),
        ("b", None) => {
            // The agent hasn't said it's a task yet; the call's id will do.
            if conn(m, key).info.proto < 3 {
                return background_job(m, key, None);
            }
            m.flash("moved the shell to the background · the turn carries on");
            send(m, key, Op::Background { id });
            Out::Draw
        }
        (_, Some(j)) => stop_job(m, key, &j),
        _ => {
            m.flash("Claude Code hasn't said it's a task yet · try again in a moment");
            Out::Draw
        }
    }
}

/// Acts on a picked task, and ctrl+b on whatever the turn waits on
/// (jobKey); x on a picked run stops it (paneKey). The background view's
/// space, → and d are running::background_key's.
pub fn job_key(m: &mut Model, key: &str, s: &str, empty: bool) -> Out {
    if s == "ctrl+b" {
        // Only while the turn waits on a command; otherwise ctrl+b is /btw's.
        if !conn(m, key).hosted() || !waits_on_command(m, key) {
            return Out::Pass;
        }
        let j = picked_job(m, key).filter(|j| !j.background);
        return background_job(m, key, j.as_ref());
    }
    let out = shell_key(m, key, s, empty);
    if out != Out::Pass {
        return out;
    }
    let sel = sel(m);
    if let Some(j) = picked_job(m, key)
        && (!sel.starts_with("run:") || s == "b")
    {
        match s {
            "ctrl+x" => return stop_job(m, key, &j),
            "x" if empty => return stop_job(m, key, &j),
            "b" if empty && j.background => {
                m.flash("that's already in the background");
                return Out::Draw;
            }
            "b" if empty => return background_job(m, key, Some(&j)),
            _ => {}
        }
    }
    // ctrl+x on a subagent, picked or watched, stops that one alone; x
    // does too on its row.
    let watching = super::subview::watching(m, key);
    if (s == "ctrl+x" || s == "x" && empty && !watching)
        && let Some((sa, live)) = picked_sub(m, key)
        && (live || s == "x")
    {
        return stop_sub(m, key, &sa, live);
    }
    Out::Pass
}

/// The Session's keys hook: a picked task's, run's or command's x, b and
/// ctrl+x, before the box types them.
pub fn key(m: &mut Model, k: &Press) -> Out {
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    if !matches!(k.s.as_str(), "x" | "b" | "k" | "p" | "ctrl+x") {
        return Out::Pass;
    }
    let empty = conn(&mut *m, &key).input.is_empty();
    job_key(m, &key, &k.s, empty)
}

/// ctrl+b moves what the turn waits on to the background, while the dock
/// offers it: before /btw has the key, unless its thread or a card has
/// the keys (paneKey :4003).
pub fn ctrl_b(m: &mut Model, k: &Press) -> Out {
    if k.s != "ctrl+b" {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    if m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let cards = m.cards();
    if cards.conns.get(&key).is_some_and(|c| c.card_focus) {
        return Out::Pass;
    }
    if crate::screens::session_cards::btw::btw_for(m, &key).is_some_and(|t| t.focused) {
        return Out::Pass;
    }
    let empty = conn(m, &key).input.is_empty();
    job_key(m, &key, "ctrl+b", empty)
}

/// What x and k would stop of what's picked now, for a stop with a
/// message (stoppable): k only a running command.
pub fn stops(m: &Model, key: &str) -> (bool, bool) {
    let shell = !picked_shell(m, key).is_empty();
    let x = shell || picked_job(m, key).is_some() || picked_sub(m, key).is_some();
    (x, shell)
}

/// x (or k) on what's picked, as with nothing typed (stopPicked); Pass
/// when it stops nothing.
pub fn stop_picked(m: &mut Model, key: &str, k: &str) -> Out {
    let out = job_key(m, key, k, true);
    if out != Out::Pass {
        return out;
    }
    if k == "x"
        && let Some((sa, live)) = picked_sub(m, key)
    {
        return stop_sub(m, key, &sa, live);
    }
    Out::Pass
}
