//! What stops an agent, puts it away or hides it: #stop, #rm, #kill and
//! #done, ctrl+x and alt+d in the list, the question that closes one
//! (ctrl+] x), and the stop of agents long finished (hibernate). Every
//! stop goes through outside::stop_outside, so a rush session stops with
//! the agents it spawned, as ctrl+x stops it: none works on unseen.
//!
//! Go: internal/ui/keys.go (stopOrRemove :688, command's "done" :998,
//! "stop" :1021, "rm" :1028, "kill" :1040, listKey's ctrl+x and alt+d
//! :462-466, askKillTree :1215, //! :1215, killTree :1271, relaunch :1161), tempwork.go (markDone
//! :183, dismiss :226, askClose :246, closeAgent :288, stopAgent :306,
//! toDone :318, stopRun :338), logins.go (restart :703, its run through
//! the host's restartClaude :672), model.go (hibernate :2036, toggleDone
//! :2699, movedMsg :1308), keybind.go:181 (list.close, session.close) and
//! outside.go:19-22 (stopOutside, as of e0f011a4). A room's row asks to
//! hide the room (roomrows.go:229 askHideRoom, social/rooms.rs).

use std::collections::HashSet;

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use super::outside::{remove_outside, stop_outside};
use crate::frame::{Confirm, Ext, Out, Press};
use crate::model::Model;
use crate::screens::session_core::host::hosts;
use crate::screens::social::rooms::is_room_key;

/// Adds the commands and list keys (keys.go:462-466, :998-1043).
pub fn register(x: &mut Ext) {
    x.command(
        "done",
        "put the agent away in Earlier (alt+d); its idle process stops",
        |m, _| {
            let a = picked(m);
            mark_done(m, a)
        },
    );
    x.command("stop", "stop the agent", stop_command);
    x.command(
        "rm",
        "delete the session, and its worktree when that's safe",
        rm_command,
    );
    x.command(
        "kill",
        "kill the agent and everything it started",
        kill_command,
    );
    x.list_keys.push(list_key);
    x.on_fleet.push(hibernate);
}

/// The agent picked in the list, or the Session's (a command's a).
fn picked(m: &Model) -> Option<Agent> {
    m.selected().map(|i| m.agents[i].clone())
}

/// The agent picked, or says to pick one (need).
fn need(m: &mut Model) -> Option<Agent> {
    let a = picked(m);
    if a.is_none() {
        m.flash_err("select an agent first");
    }
    a
}

/// ctrl+x hides the agent picked, alt+d (and ctrl+d) puts it away
/// (keys.go:462-466).
fn list_key(m: &mut Model, k: &Press) -> Out {
    match k.s.as_str() {
        "ctrl+x" => {
            let a = picked(m);
            dismiss(m, a)
        }
        "ctrl+d" | "alt+d" => {
            let a = picked(m);
            mark_done(m, a)
        }
        _ => Out::Pass,
    }
}

/// #stop (keys.go:1021).
fn stop_command(m: &mut Model, _: &str) -> Out {
    let Some(a) = need(m) else {
        return Out::Draw;
    };
    if a.past {
        return stop_or_remove(m, Some(a));
    }
    to_done(m, &a);
    let stop = stopper(m, &a);
    run(m, format!("stopped {}", a.display_name), stop);
    Out::Draw
}

/// #rm (keys.go:1028).
fn rm_command(m: &mut Model, _: &str) -> Out {
    let Some(a) = need(m) else {
        return Out::Draw;
    };
    if a.past {
        return stop_or_remove(m, Some(a));
    }
    let name = a.display_name.clone();
    m.ask(
        Confirm::new(format!("Delete {name}?"), move |m| {
            let reg = m.reg.clone();
            run(m, format!("deleted {name}"), move || {
                remove_outside(&reg, &a)
            });
            Out::Draw
        })
        .detail("removes the session, and its worktree when that's safe"),
    );
    Out::Draw
}

/// #kill (keys.go:1040).
fn kill_command(m: &mut Model, _: &str) -> Out {
    if let Some(a) = need(m) {
        ask_kill_tree(m, a);
    }
    Out::Draw
}

/// Runs work off the UI thread, then says msg, or what went wrong
/// (cmdErr and doneMsg): "" says nothing.
fn run(m: &mut Model, msg: String, work: impl FnOnce() -> Result<(), String> + Send + 'static) {
    m.later(work, move |m, r| {
        match r {
            Ok(()) if !msg.is_empty() => m.flash(&msg),
            Ok(()) => {}
            Err(e) => m.fail(&e),
        }
        Out::Draw
    });
}

/// a's stop, its agent's own way, for off the UI thread: a rush session
/// with the agents it spawned, through its host (stopOutside).
fn stopper(m: &mut Model, a: &Agent) -> impl FnOnce() -> Result<(), String> + Send + 'static {
    let (reg, h, a) = (m.reg.clone(), hosts(m), a.clone());
    move || stop_outside(&reg, &a, |id| h.stop_all(id))
}

/// Whether a has a run to stop: a process, or a live worker.
fn running(a: &Agent) -> bool {
    a.pid != 0 || (a.job.live() && a.worker.is_some())
}

/// x on an agent long finished: a terminal one closes, a running one
/// stops, one past nothing (stopOrRemove).
pub fn stop_or_remove(m: &mut Model, a: Option<Agent>) -> Out {
    let Some(a) = a else {
        return Out::Idle;
    };
    let name = a.display_name.clone();
    if a.past {
        m.flash(&format!(
            "{name} is a past conversation: nothing runs to stop, and its transcript is kept"
        ));
        return Out::Draw;
    }
    if a.interactive {
        let pid = a.pid;
        m.ask(
            Confirm::new(format!("Close {name}?"), move |m| {
                run(m, format!("closed {name}"), move || {
                    rush_sys::actions::terminate(pid).map_err(|e| e.to_string())
                });
                Out::Draw
            })
            .detail(format!("sends SIGTERM to the terminal session (pid {pid})")),
        );
        return Out::Draw;
    }
    if running(&a) {
        to_done(m, &a);
        m.flash(&format!("stopping {name}…"));
        let stop = stopper(m, &a);
        run(m, format!("stopped {name}"), stop);
        return Out::Draw;
    }
    m.ask(
        Confirm::new(format!("Delete {name}?"), move |m| {
            m.flash(&format!("deleting {name}…"));
            let reg = m.reg.clone();
            run(m, format!("deleted {name}"), move || {
                remove_outside(&reg, &a)
            });
            Out::Draw
        })
        .detail("and its worktree, when that's safe"),
    );
    Out::Draw
}

/// #kill: stop it, keeping the conversation, or ! SIGKILLs its whole tree
/// (askKillTree). Its processes are counted off the UI thread first.
pub fn ask_kill_tree(m: &mut Model, a: Agent) {
    let Some(root) = a.worker else {
        m.flash_err(&format!("{} has no running process", a.display_name));
        return;
    };
    m.later(
        move || {
            let t = rush_sys::proc::snapshot();
            let (mem, _, n) = t.sum(root, &HashSet::new());
            let start = t.procs.get(&root).map(|p| p.start);
            (mem, n, start)
        },
        move |m, (mem, n, start)| {
            let name = a.display_name.clone();
            let b = a.clone();
            let mut c = Confirm::new(format!("Stop {name}?"), move |m| {
                to_done(m, &a);
                let stop = stopper(m, &a);
                run(m, format!("stopped {}", a.display_name), stop);
                Out::Draw
            })
            .detail(format!(
                "{n} processes · {} · the conversation is kept",
                crate::style::mem(mem)
            ));
            c.bang_text = "SIGKILL the whole tree".into();
            c.on_bang = Some(Box::new(move |m| {
                to_done(m, &b);
                let Some(start) = start else {
                    m.fail(&format!("{} has no running process", b.display_name));
                    return Out::Draw;
                };
                m.later(
                    move || rush_sys::actions::kill_tree(root, start).map_err(|e| e.to_string()),
                    |m, r| {
                        match r {
                            Ok(n) => m.flash(&format!("killed {n} processes")),
                            Err(e) => m.fail(&e),
                        }
                        Out::Draw
                    },
                );
                Out::Draw
            }));
            m.ask(c);
            Out::Draw
        },
    );
}

/// alt+d and #done: the agent put away in Earlier, or back. Put away, it
/// shouldn't hold memory, so an idle process of its goes too (a message
/// resumes it); only one still working asks first. Temp work stays
/// (markDone).
pub fn mark_done(m: &mut Model, a: Option<Agent>) -> Out {
    let Some(a) = a else {
        m.flash_err("select an agent first");
        return Out::Draw;
    };
    crate::screens::accounts::onboard::did_step(m, "done");
    if a.done {
        toggle_done(m, &a.key);
        return Out::Draw;
    }
    let working = a.job.live() || a.busy();
    let interactive = a.interactive;
    let name = a.display_name.clone();
    let done = move |m: &mut Model| -> Out {
        toggle_done(m, &a.key);
        if a.interactive {
            return Out::Draw; // it's open in a terminal: leave it be
        }
        if a.rush {
            // Its idle host may be gone and the agents it spawned still at
            // work.
            m.flash(&format!(
                "done: {} · it stopped with all it started, a message resumes it",
                a.display_name
            ));
            let (h, id) = (hosts(m), a.job.id.clone());
            run(m, String::new(), move || h.stop_all(&id));
            return Out::Draw;
        }
        if a.pid == 0 {
            return Out::Draw; // nothing resident
        }
        m.flash(&format!(
            "done: {} · its process stopped, a message resumes it",
            a.display_name
        ));
        let stop = stopper(m, &a);
        run(m, String::new(), stop);
        Out::Draw
    };
    if !working {
        return done(m);
    }
    let detail = if interactive {
        "it's still working, in its terminal; it keeps going there"
    } else {
        "it's still working · y stops it"
    };
    m.ask(Confirm::new(format!("Mark {name} done?"), done).detail(detail));
    Out::Draw
}

/// ctrl+x: the agent goes for good. It stops with all it set going and
/// leaves the list; its transcript stays on disk. One mid-turn asks first,
/// ctrl+x again saying yes; one in a terminal asks too, since that ends it
/// there (dismiss).
pub fn dismiss(m: &mut Model, a: Option<Agent>) -> Out {
    let Some(a) = a else {
        return Out::Idle;
    };
    if is_room_key(&a.key) || a.past || a.interactive {
        return ask_close(m, Some(a));
    }
    if a.busy() {
        let again = super::keybind::bound_key(m, "list.stop");
        let name = a.display_name.clone();
        let mut c = Confirm::new(format!("Stop {name} and hide it?"), move |m| {
            close_agent(m, &a)
        })
        .detail("it's mid-turn · it stops with all it started · transcript kept on disk");
        c.yes_text = "stop".into();
        c.again = again;
        m.ask(c);
        return Out::Draw;
    }
    close_agent(m, &a)
}

/// The one way to close an agent, esc twice in its Session or ctrl+] x:
/// y hides it (stopped, gone from the list for good; its conversation
/// kept on disk), z stops it and keeps it in the list, r restarts it, s
/// switches its harness or model, x deletes it for good (askClose).
pub fn ask_close(m: &mut Model, a: Option<Agent>) -> Out {
    let Some(a) = a else {
        return Out::Idle;
    };
    if is_room_key(&a.key) {
        return crate::screens::social::rooms::ask_hide_room(m, &a);
    }
    let name = a.display_name.clone();
    let hide = a.clone();
    let mut c = Confirm::new(format!("Hide {name}?"), move |m| close_agent(m, &hide));
    c.yes_text = "hide".into();
    if a.past {
        c.question = format!("{name} is a past conversation");
        c.on_yes = None;
    } else if a.interactive {
        c.detail =
            "hides it from the list · it runs in its terminal: this ends that · transcript kept"
                .into();
    } else {
        c.detail = "y hides it from the list and stops it · z just stops it and keeps it in the list · transcript kept on disk".into();
    }
    // A stop, not a hide: the run ends, the agent stays in the list, its
    // transcript stays. It comes first, so the gentlest reads first.
    if !a.past && running(&a) {
        let b = a.clone();
        c.more.push((
            "z".into(),
            "stop, keep it".into(),
            Box::new(move |m| stop_agent(m, &b)),
        ));
    }
    if !a.past && !a.interactive {
        let b = a.clone();
        c.more.push((
            "r".into(),
            "restart".into(),
            Box::new(move |m| restart(m, b, String::new())),
        ));
    }
    let linked = crate::screens::session_core::core(m).linked == a.key;
    if linked {
        let key = a.key.clone();
        c.more.push((
            "s".into(),
            "switch harness or model".into(),
            Box::new(
                move |m| match crate::screens::bars::agent::session_start(m, &key) {
                    Some(o) => crate::screens::accounts::startsheet::open_switch_sheet(m, &key, o),
                    None => Out::Idle,
                },
            ),
        ));
    }
    let removes = m
        .reg
        .get(a.kind.as_str())
        .is_some_and(|ad| ad.remover().is_some());
    if removes && !a.interactive {
        let b = a.clone();
        c.more.push((
            "x".into(),
            "delete for good".into(),
            Box::new(move |m| {
                let reg = m.reg.clone();
                let name = b.display_name.clone();
                run(m, format!("deleted {name}"), move || {
                    remove_outside(&reg, &b)
                });
                Out::Draw
            }),
        ));
    }
    if c.on_yes.is_none() && c.more.is_empty() {
        m.flash(&format!(
            "{name} is a past conversation: nothing runs to close"
        ));
        return Out::Draw;
    }
    m.ask(c);
    Out::Draw
}

/// #restart: the agent's program starts again, so it picks up the
/// account in use and settings changed since. A rush agent keeps its
/// place; any other is relaunched on its account. One stopped by a limit
/// or an error is told to continue; one mid-turn asks first (restart).
pub fn restart(m: &mut Model, a: Agent, text: String) -> Out {
    if !a.rush {
        let to = a.acct.clone();
        return relaunch(m, a, to);
    }
    let detail = a.job.detail.clone();
    let limited = detail.starts_with("usage limit");
    let text = if text.is_empty() && (limited || detail.starts_with("API error")) {
        "continue".to_string()
    } else {
        text
    };
    let (id, name) = (a.job.id.clone(), a.display_name.clone());
    let go = move |m: &mut Model| -> Out {
        let h = hosts(m);
        let (id, n, t) = (id.clone(), name.clone(), text.clone());
        let (name, text) = (name.clone(), text.clone());
        m.later(
            move || h.restart(&id, &n, &t),
            move |m, r| {
                match r {
                    Ok(()) if text.is_empty() => m.flash(&format!(
                        "restarted {name} · it starts again with your next message"
                    )),
                    Ok(()) => m.flash(&format!("restarted {name}")),
                    Err(e) => m.fail(&e),
                }
                Out::Draw
            },
        );
        Out::Draw
    };
    if a.job.state == State::Working || a.job.state == State::Blocked && !limited {
        m.ask(
            Confirm::new(format!("Restart {}?", a.display_name), go).detail(
                "it's in the middle of a turn · y cuts it off and resumes the conversation",
            ),
        );
        return Out::Draw;
    }
    go(m)
}

/// Starts a session running outside rush again on profile to, keeping
/// its conversation; another account's says it moved. The old row stays
/// in Earlier, pointing at the new one (relaunch, movedMsg).
pub fn relaunch(m: &mut Model, a: Agent, to: rush_core::adapter::Profile) -> Out {
    let note = if to.name != a.acct.name {
        "This conversation moved to another account; carry on where you left off.".to_string()
    } else {
        String::new()
    };
    let kind = a.kind.as_str().to_string();
    if !m.reg.get(&kind).is_some_and(|ad| ad.mover().is_some()) {
        m.flash_err(&format!(
            "{} can't move a session outside rush mode · /rush moves it over first",
            crate::screens::accounts::look::harness_name(&m.reg, &kind)
        ));
        return Out::Draw;
    }
    m.flash(&format!("relaunching {}…", a.display_name));
    let reg = m.reg.clone();
    let mv = rush_core::adapter::Move {
        from: a.acct.clone(),
        to: to.clone(),
        job: a.job.clone(),
        note,
        ..Default::default()
    };
    m.later(
        move || {
            let ad = reg.get(&kind).ok_or("its agent isn't here")?;
            let mover = ad.mover().ok_or("its agent can't move it")?;
            mover
                .move_session(&mv)
                .map_err(|e| e.to_string())
                .map(|id| rush_state::key(&to.name, &id))
        },
        move |m, r: Result<String, String>| {
            match r {
                Ok(to) => moved(m, &a, to),
                Err(e) => m.fail(&e),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// A session moved: its old row keeps its history, put away in Earlier
/// and pointing at the new one, which is picked (movedMsg).
fn moved(m: &mut Model, from: &Agent, to: String) {
    let (key, t) = (from.key.clone(), to.clone());
    m.later(
        move || {
            let mut s = rush_state::Store::load();
            s.overlay.moved.insert(key.clone(), t.clone());
            s.overlay.done.insert(key.clone(), rush_state::Time::now());
            if let Some(n) = s.overlay.names.get(&key).cloned()
                && !n.is_empty()
            {
                s.overlay.names.insert(t, n);
            }
            s.save_overlay().map_err(|e| e.to_string())
        },
        |m, r: Result<(), String>| {
            if let Err(e) = r {
                m.fail(&e);
            }
            Out::Draw
        },
    );
    let short = to
        .strip_prefix(&format!("{}/", from.acct.name))
        .unwrap_or(&to)
        .to_string();
    m.sel = to;
    m.flash(&format!("moved — now {short}"));
}

/// Hides a for good and stops it; one in a terminal is ended there. The
/// hide is in rush's state, so no reading lists it again; its transcript
/// stays (closeAgent).
pub fn close_agent(m: &mut Model, a: &Agent) -> Out {
    let next = if m.sel == a.key {
        crate::screens::list_extras::neighbour(m, &a.key)
    } else {
        String::new()
    };
    let key = a.key.clone();
    m.later(
        move || {
            let mut s = rush_state::Store::load();
            s.hide(&key);
            s.save_overlay().map_err(|e| e.to_string())
        },
        |m, r: Result<(), String>| {
            if let Err(e) = r {
                m.fail(&format!("couldn't keep it hidden: {e}"));
            }
            Out::Draw
        },
    );
    // Gone from the list now, not on the next reading.
    if let Some(i) = m.agents.iter().position(|x| x.key == a.key) {
        m.agents.remove(i);
        if i < m.roots.len() {
            m.roots.remove(i);
        }
    }
    m.rebuild();
    if !next.is_empty() {
        m.sel = next;
    }
    m.flash(&format!("hidden: {}", a.display_name));
    stop_run(m, a, String::new());
    Out::Draw
}

/// Ends a's run and leaves it in the list: a stop, not a hide. It comes
/// back with a message (stopAgent).
pub fn stop_agent(m: &mut Model, a: &Agent) -> Out {
    if a.pid == 0 && !a.job.live() {
        m.flash(&format!("{} isn't running", a.display_name));
        return Out::Draw;
    }
    to_done(m, a);
    m.flash(&format!("stopping {}…", a.display_name));
    stop_run(m, a, format!("stopped {}", a.display_name));
    Out::Draw
}

/// Puts a away in Earlier when it's stopped or killed: a stopped agent has
/// nothing left to show among the working ones. A message brings it back
/// (toDone).
pub fn to_done(m: &mut Model, a: &Agent) {
    let done = m.agents.iter().any(|x| x.key == a.key && x.done);
    if !done && !a.past {
        toggle_done(m, &a.key);
    }
}

/// Ends a's run as its own kind needs: a terminal session gets a SIGTERM,
/// a rush session its own stop with what it spawned, an outside one its
/// stop. msg is said on success, "" nothing (stopRun).
pub fn stop_run(m: &mut Model, a: &Agent, msg: String) {
    if a.interactive {
        let pid = a.pid;
        run(m, msg, move || {
            rush_sys::actions::terminate(pid).map_err(|e| e.to_string())
        });
        return;
    }
    if a.rush {
        // With the agents it spawned: none works on once it's stopped.
        let (h, id) = (hosts(m), a.job.id.clone());
        run(m, msg, move || h.stop_all(&id));
        return;
    }
    if a.pid == 0 {
        return;
    }
    let stop = stopper(m, a);
    run(m, msg, stop);
}

/// Puts the agent with key away in Earlier, or brings it back, at once
/// in the list and in rush's state off the UI thread. The selection stays
/// in the group the agent leaves: the row below it, or above if it was
/// the last; brought back, it goes with it, its section unfolded
/// (toggleDone).
pub fn toggle_done(m: &mut Model, key: &str) {
    let Some(i) = m.agents.iter().position(|a| a.key == key) else {
        return;
    };
    let from = m.group_of.get(key).cloned().unwrap_or_default();
    let back = m.agents[i].done;
    let a = &m.agents[i];
    let name = a.display_name.clone();
    // One whose process is being stopped leaves when that exits, so it's
    // counted as gone already, else the selection would follow it down
    // into Earlier.
    let stopping = !a.done && a.pid != 0 && !a.interactive && !a.pinned;
    let next = if m.sel == key && !back {
        crate::screens::list_extras::neighbour(m, key)
    } else {
        String::new()
    };
    m.agents[i].done = !back;
    keep_done(m, key, !back);
    if back {
        m.flash(&format!("moved back: {name}"));
        m.rebuild();
        if let Some(t) = m.group_of.get(key).cloned()
            && !t.is_empty()
            && m.folded(&t)
        {
            m.toggle_fold(&t);
        }
        m.sel = key.to_string();
        return;
    }
    m.rebuild();
    let now_in = m.group_of.get(key).cloned().unwrap_or_default();
    if !next.is_empty()
        && (stopping || now_in != from)
        && m.group_of.get(&next).is_some_and(|g| *g == from)
    {
        m.sel = next;
    }
}

/// Keeps key put away (or not) in rush's state.json, off the UI thread.
fn keep_done(m: &mut Model, key: &str, done: bool) {
    let key = key.to_string();
    m.later(
        move || {
            let mut s = rush_state::Store::load();
            if done {
                s.overlay.done.insert(key, rush_state::Time::now());
            } else {
                s.overlay.done.remove(&key);
            }
            s.save_overlay().map_err(|e| e.to_string())
        },
        |m, r: Result<(), String>| {
            if let Err(e) = r {
                m.fail(&format!("couldn't keep it in Earlier: {e}"));
            }
            Out::Draw
        },
    );
}

/// The agents hibernate has stopped, by key: one try each, so a failed
/// stop isn't tried again with every reading.
#[derive(Default)]
struct Hibernated(HashSet<String>);

/// The finished agents whose process is still resident after Settings ›
/// General's Hibernate after, each once; none in a rush that follows: the
/// one that leads stops them (hibernate, applySnap's leads).
pub fn to_hibernate(m: &mut Model, after_minutes: i64, now: Ms) -> Vec<Agent> {
    if !m.leads() || after_minutes <= 0 {
        return Vec::new();
    }
    let after = after_minutes * 60_000;
    let due: Vec<Agent> = m
        .agents
        .iter()
        .filter(|a| a.worker.is_some() && a.job.state == State::Done && a.age(now) > after)
        .cloned()
        .collect();
    let seen = &mut m.kept::<Hibernated>().0;
    due.into_iter()
        .filter(|a| seen.insert(a.key.clone()))
        .collect()
}

/// After each reading of the list, with a loop running: what's due to
/// hibernate is stopped, its agent's own way (hibernate).
fn hibernate(m: &mut Model) {
    if m.sender().is_none() {
        return;
    }
    let after = crate::screens::projects_machine::hibernate_after(m);
    let now = m.now;
    for a in to_hibernate(m, after, now) {
        let stop = stopper(m, &a);
        std::thread::spawn(move || {
            let err = stop().err().unwrap_or_default();
            rush_sys::events::log(
                "hibernate",
                serde_json::json!({
                    "key": a.key, "name": a.display_name, "pid": a.pid,
                    "why": format!("done for over {after} minutes"), "err": err,
                }),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::session_core::host::{Fake, set_hosts};
    use rush_core::agent::Job;

    fn agent(key: &str, state: State) -> Agent {
        Agent {
            key: key.into(),
            display_name: key.into(),
            rush: true,
            job: Job {
                id: key.into(),
                state,
                updated_at: 1_000,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn model(agents: Vec<Agent>) -> (Model, std::sync::Arc<Fake>) {
        let mut m = crate::screens::social::feed::tests::model(120, 30);
        let n = agents.len();
        m.set_fleet(crate::worker::Fleet {
            agents,
            roots: vec![String::new(); n],
            ..Default::default()
        });
        let h = Fake::new();
        set_hosts(&mut m, std::sync::Arc::new(h.clone()));
        (m, h)
    }

    // e0f011a4: #stop stops a rush agent with the agents it spawned, as
    // ctrl+x does, and puts it away in Earlier.
    #[test]
    fn stop_goes_through_stop_all() {
        let (mut m, h) = model(vec![agent("a", State::Working)]);
        m.sel = "a".into();
        m.command("#stop");
        assert_eq!(*h.stopped_all.lock().unwrap(), ["a"]);
        assert!(m.agents[0].done, "a stopped agent is put away");
    }

    // alt+d on a finished rush agent: put away, and stopped with all it
    // started; alt+d again brings it back.
    #[test]
    fn done_stops_and_brings_back() {
        let (mut m, h) = model(vec![agent("a", State::Done)]);
        m.sel = "a".into();
        m.press("alt+d");
        assert!(m.agents[0].done);
        assert_eq!(*h.stopped_all.lock().unwrap(), ["a"]);
        m.sel = "a".into(); // picked again in Earlier
        m.press("alt+d");
        assert!(!m.agents[0].done, "brought back");
        assert_eq!(m.sel, "a");
    }

    // A working one asks first.
    #[test]
    fn done_asks_while_working() {
        let (mut m, h) = model(vec![agent("a", State::Working)]);
        m.agents[0].pid = 0;
        m.sel = "a".into();
        m.command("#done");
        assert_eq!(m.confirm.as_ref().unwrap().question, "Mark a done?");
        m.press("y");
        assert!(m.agents[0].done && h.stopped_all.lock().unwrap().len() == 1);
    }

    // ctrl+x hides one, which leaves the list and stops.
    #[test]
    fn ctrl_x_hides() {
        let (mut m, h) = model(vec![agent("a", State::Done), agent("b", State::Done)]);
        m.sel = "a".into();
        m.press("ctrl+x");
        assert!(m.agents.iter().all(|a| a.key != "a"), "hidden");
        assert_eq!(*h.stopped_all.lock().unwrap(), ["a"]);
    }

    // #rm on a past conversation says there's nothing to stop.
    #[test]
    fn rm_on_past() {
        let (mut m, _) = model(vec![agent("a", State::Done)]);
        m.agents[0].past = true;
        m.sel = "a".into();
        m.command("#rm");
        assert!(m.confirm.is_none());
    }

    // Hibernate: a finished agent past its time, with a process, once;
    // never in a rush that follows (model.go:2036, snapload.go:84).
    #[test]
    fn hibernate_leads_only() {
        let mut a = agent("a", State::Done);
        a.worker = Some(4242);
        let (mut m, _) = model(vec![a]);
        let now = 1_000 + 31 * 60_000;
        m.follows = true;
        assert!(
            to_hibernate(&mut m, 30, now).is_empty(),
            "a follower stops nothing"
        );
        m.follows = false;
        assert_eq!(to_hibernate(&mut m, 30, now).len(), 1);
        assert!(to_hibernate(&mut m, 30, now).is_empty(), "one try each");
        assert!(to_hibernate(&mut m, 0, now).is_empty());
    }
}
