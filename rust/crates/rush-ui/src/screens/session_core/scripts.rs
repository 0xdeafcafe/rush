//! A session's shell calls rush ran as scripts, as last read: each one's
//! files' key and what its trace said, read again off the UI thread while
//! the call runs; what the conversation shows of each (the line it's on,
//! how long each line took, the held marker), and a click on a line's
//! number setting a breakpoint there; p on one held at a breakpoint lets
//! it go on. After Go's internal/ui/scripts.go (scriptSeen :17, scriptRead
//! :35, scriptEvery :51, scriptTurns :55, scriptViews :59, script :91,
//! toggleScriptBreak :160, goOnFromBreak :203) and rushmode.go:2404
//! (o.Scripts) and :5073 (a click on a script's line). How a run reads
//! as a view, and the breakpoint's words, are projects_machine/scripts.rs.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rush_convo::model::{Item, Status};
use rush_convo::render::{ScriptView, script_line};
use rush_core::time::Ms;
use rush_sys::script::Run;

use super::conn;
use crate::frame::Out;
use crate::model::Model;
use crate::screens::projects_machine::scripts::{
    CallIs, SCRIPT_EVERY, SCRIPT_TURNS, break_flash, break_refused, held, script_view,
};

/// A shell call's script as last read: its files' key and what its trace
/// said, and when that was (scriptSeen).
#[derive(Clone, Debug, Default)]
pub struct Seen {
    /// None until a read finds it ran as a script.
    pub read: Option<Read>,
    /// A read is under way, off the UI.
    pub reading: bool,
    /// When it was last asked to be read; 0 to read it again.
    pub at: Ms,
    /// The call still runs: only then can it be stopped.
    pub live: bool,
    /// Too short to have run as a script: never read.
    pub plain: bool,
    /// Reads taken in so far: what its view was worked out from.
    pub reads: u64,
    /// The view last worked out, for the read, status and end it came
    /// from: a frame asks for every script's (view, viewOf).
    pub view: Option<(ViewOf, ScriptView)>,
}

/// What a script's view was worked out from (viewOf).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewOf {
    reads: u64,
    status: Status,
    end: Ms,
}

/// What a call's script files said when last read (scriptRead).
#[derive(Clone, Debug)]
pub struct Read {
    pub key: String,
    pub run: Run,
}

/// A shell call of the latest turns: its id, command, how it stands and
/// when it ended.
struct Call {
    id: String,
    command: String,
    status: Status,
    end: Ms,
}

/// The shell calls of session key's latest turns, as its open
/// conversation has them (scriptViews' walk).
fn calls(m: &Model, key: &str) -> Vec<Call> {
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return Vec::new();
    };
    let turns = &v.sess.turns;
    let mut out = Vec::new();
    for t in &turns[turns.len().saturating_sub(SCRIPT_TURNS)..] {
        for it in &t.items {
            let Item::Step(sid) = it else { continue };
            let st = v.sess.step(*sid);
            if &*st.tool != "Bash" || st.id.is_empty() {
                continue;
            }
            out.push(Call {
                id: st.id.to_string(),
                command: st.call().input.command.trim().to_string(),
                status: st.status,
                end: st.end,
            });
        }
    }
    out
}

/// Reads session key's shell calls again where they may have moved on:
/// the trace is a line per command run, and the disk mustn't hold a
/// frame. What's read is there from the next frame (script); the open
/// conversation draws each as its view says (scriptViews, o.Scripts).
pub fn sync(m: &mut Model, key: &str) {
    let now = m.now;
    let calls = calls(m, key);
    let mut shows = Vec::with_capacity(calls.len());
    for c in calls {
        let running = c.status == Status::Running;
        shows.push((c.id.clone(), c.status, c.end));
        let s = conn(m, key).scripts.entry(c.id.clone()).or_default();
        s.live = running;
        if s.plain || s.reading {
            continue;
        }
        let fresh = now - s.at < SCRIPT_EVERY.as_millis() as Ms;
        if s.at != 0 && (fresh || !running && s.at > c.end) {
            continue;
        }
        if !rush_sys::script::long(&c.command) {
            s.plain = true; // its command doesn't change: never one
            continue;
        }
        (s.at, s.reading) = (now, true);
        let (key, id) = (key.to_string(), c.id);
        let call = id.clone();
        m.later(
            move || read(&call, &c.command),
            move |m, r| {
                let Some(s) = conn(m, &key).scripts.get_mut(&id) else {
                    return Out::Idle;
                };
                s.reading = false;
                let Some(r) = r else { return Out::Idle };
                let moved = s.read.as_ref().is_none_or(|was| was.run != r.run);
                s.read = Some(r);
                if !moved {
                    return Out::Idle;
                }
                s.reads += 1;
                Out::Draw
            },
        );
    }
    // What each read says, worked out again only when the read, or how
    // the call stands, changed.
    let mut views = HashMap::new();
    for (id, status, end) in shows {
        let Some(s) = conn(m, key).scripts.get_mut(&id) else {
            continue;
        };
        let Some(r) = &s.read else { continue };
        let of = ViewOf {
            reads: s.reads,
            status,
            end,
        };
        if s.view.as_ref().is_none_or(|(o, _)| *o != of) {
            s.view = Some((of, view_of(&r.run, status, end)));
        }
        if let Some((_, v)) = &s.view {
            views.insert(id, v.clone());
        }
    }
    if let Some(v) = m.session.as_mut().filter(|v| v.key == key)
        && v.o.scripts != views
    {
        v.o.scripts = views;
    }
}

/// What the conversation shows of run r of a call that is as status,
/// ended at end (scriptView), in its own time.
fn view_of(r: &Run, status: Status, end: Ms) -> ScriptView {
    let is = match status {
        Status::Running => CallIs::Running,
        Status::Failed => CallIs::Failed,
        _ => CallIs::Done,
    };
    let end = (end > 0).then(|| UNIX_EPOCH + Duration::from_millis(end as u64));
    let v = script_view(r, is, end);
    ScriptView {
        at: v.at,
        since: v.since.map_or(0, ms),
        took: v
            .took
            .into_iter()
            .map(|(n, d)| (n, d.as_millis() as Ms))
            .collect(),
        breaks: v.breaks,
        held: v.held,
        failed_at: v.failed_at,
    }
}

fn ms(t: SystemTime) -> Ms {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as Ms)
}

/// A click on row r of session key's conversation: on a script's line,
/// sets a breakpoint there or takes it away; None when r isn't a line of
/// a script read (toggleScriptBreak). The breakpoint's file is written
/// off the UI thread.
pub fn toggle_break(m: &mut Model, key: &str, r: &str) -> Option<Out> {
    let (step_ref, n) = script_line(r)?;
    let (_, id) = step_ref.split_once(":s:")?;
    let s = conn(m, key).scripts.get(id)?;
    let read = s.read.as_ref().filter(|r| !r.key.is_empty())?;
    if let Some(why) = break_refused(&read.run, s.live, n) {
        m.flash_err(&why);
        return Some(Out::Draw);
    }
    let file = read.key.clone();
    let (key, id) = (key.to_string(), id.to_string());
    m.later(
        move || rush_sys::script::Scripts::new(rush_state::dir()).toggle(&file, n),
        move |m, on| {
            match on {
                Ok(on) => m.flash(&break_flash(n, on)),
                Err(e) => m.fail(&format!("couldn't set the breakpoint: {e}")),
            }
            if let Some(s) = conn(m, &key).scripts.get_mut(&id) {
                s.at = 0; // read again, with its dot
            }
            Out::Draw
        },
    );
    Some(Out::Draw)
}

/// Call id's script as its files say, by its id's key or its command's;
/// None when it didn't run as one. Reads files: off the UI thread.
fn read(id: &str, command: &str) -> Option<Read> {
    let scripts = rush_sys::script::Scripts::new(rush_state::dir());
    [
        rush_sys::script::key(id, command),
        rush_sys::script::key("", command),
    ]
    .into_iter()
    .find_map(|k| {
        let run = scripts.read(&k)?;
        (run.source.trim() == command).then_some(Read { key: k, run })
    })
}

/// Lets call id's script, stopped at a breakpoint, go on: the shell
/// stopped itself, so it's started again. None when nothing holds it
/// (goOnFromBreak).
pub fn go_on_from_break(m: &mut Model, key: &str, id: &str) -> Option<Out> {
    let s = conn(m, key).scripts.get_mut(id)?;
    let (pid, line) = held(&s.read.as_ref()?.run)?;
    s.at = 0; // read again
    m.flash(&format!("going on from line {line}"));
    super::jobkeys::off_ui(m, move || {
        // SAFETY: kill only signals the process; no memory is shared.
        if unsafe { libc::kill(pid, libc::SIGCONT) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    });
    Some(Out::Draw)
}
