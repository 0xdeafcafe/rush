//! A session's shell calls rush ran as scripts, as last read: each one's
//! files' key and what its trace said, read again off the UI thread while
//! the call runs, so p on one held at a breakpoint lets it go on. After
//! Go's internal/ui/scripts.go (scriptSeen :17, scriptRead :35,
//! scriptEvery :51, scriptTurns :55, scriptViews :59's walk of the latest
//! turns, script :91, goOnFromBreak :203). What the conversation shows of
//! a run, and breakpoints, are projects_machine/scripts.rs.

use rush_convo::model::{Item, Status};
use rush_core::time::Ms;
use rush_sys::script::Run;

use super::conn;
use crate::frame::Out;
use crate::model::Model;
use crate::screens::projects_machine::scripts::{SCRIPT_EVERY, SCRIPT_TURNS, held};

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
}

/// What a call's script files said when last read (scriptRead).
#[derive(Clone, Debug)]
pub struct Read {
    pub key: String,
    pub run: Run,
}

/// A shell call of the latest turns: its id, command, whether it runs
/// and when it ended.
struct Call {
    id: String,
    command: String,
    running: bool,
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
                running: st.status == Status::Running,
                end: st.end,
            });
        }
    }
    out
}

/// Reads session key's shell calls again where they may have moved on:
/// the trace is a line per command run, and the disk mustn't hold a
/// frame. What's read is there from the next frame (script).
pub fn sync(m: &mut Model, key: &str) {
    let now = m.now;
    for c in calls(m, key) {
        let s = conn(m, key).scripts.entry(c.id.clone()).or_default();
        s.live = c.running;
        if s.plain || s.reading {
            continue;
        }
        let fresh = now - s.at < SCRIPT_EVERY.as_millis() as Ms;
        if s.at != 0 && (fresh || !c.running && s.at > c.end) {
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
                if let Some(s) = conn(m, &key).scripts.get_mut(&id) {
                    s.reading = false;
                    if r.is_some() {
                        s.read = r;
                    }
                }
                Out::Idle
            },
        );
    }
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
