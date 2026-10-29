//! The / commands rush answers itself, typed in a session's box: Claude
//! Code's names for rush's, the ones left out or the agent can't do and
//! claude: ones (claudescreen.rs), then rush's own (/done, /clean, /rush,
//! /clear, /plan, /diff, /tasks, /copy, /rename, /stop, /help, /cd,
//! /background, /resume, /model, /effort, and the cards' and sheets':
//! /fork, /rewind, /btw, /export, /subtask, /compact), then Claude Code's
//! screens. /model and /effort with nothing named list the agent's
//! values; a value picked is kept for the session (c.picked) and put back
//! when its host refuses it.
//!
//! After Go's internal/ui/sessionviews.go:845-1007 (runRushCommand),
//! choicesheet.go:31-125 (argRunning, setArg), :127-240 (choiceSheet,
//! openChoices, its body's about and the models added as they're read,
//! its keys), rushmode.go:5207 (leavePane) and :5456 (moveToRush, rushmove.rs), setup.go:533 (setupCommand typed the old way, /agent and
//! /profile). Writes to the host go off the UI thread (session_core's
//! later).

use rush_core::adapter::Feature;
use rush_core::text::Line;

use super::host::Op;
use super::{agent, conn, live, switch, views};
use crate::frame::choice::{Choice as Row, ChoiceSheet};
use crate::frame::sheet::Sheet;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::accounts::{choices, setup, startsheet};
use crate::screens::session_cards::{self as cards, claudescreen, permissions};

/// Runs text, a / command typed in session key's box, when it's one rush
/// answers; Pass when it goes to the agent (runRushCommand).
pub fn run(m: &mut Model, key: &str, text: &str) -> Out {
    let t = text.trim();
    let t = t.strip_prefix('/').unwrap_or(t);
    let (name, arg) = t.split_once(' ').unwrap_or((t, ""));
    let arg = arg.trim();
    let (o, name) = claudescreen::command(m, key, name, arg);
    if o.used() {
        return o;
    }
    let o = command(m, key, &name, arg, text);
    if o.used() {
        return o;
    }
    claudescreen::screen_command(m, key, &name, arg)
}

/// rush's own commands, by name (runRushCommand's switch); Pass for the
/// rest.
fn command(m: &mut Model, key: &str, name: &str, arg: &str, text: &str) -> Out {
    let a = agent(m, key).cloned();
    match name {
        "done" => done(m, a),
        "clean" => {
            match a {
                Some(_) => crate::screens::projects_machine::delete::ask_clean(m, key),
                None => m.flash_err("select an agent first"),
            }
            Out::Draw
        }
        "rush" => {
            let Some(a) = a else { return Out::Draw };
            move_to_rush(m, &a);
            Out::Draw
        }
        "clear" => {
            let Some(a) = a else { return Out::Draw };
            if !conn(m, key).hosted() || !m.reg.supports(a.kind.as_str(), Feature::Rewind) {
                // A new session beside it.
                let dir = a.job.cwd.clone();
                return super::start::start_hosted(m, "", &dir, None);
            }
            cards::slash(m, key, name, arg).or_draw()
        }
        "fork" | "rewind" | "handoff" | "compact" | "btw" | "export" | "subtask" => {
            cards::slash(m, key, name, arg)
        }
        // rush's, so #use now; typed the old way it still runs.
        "agent" | "profile" => {
            let t = text.trim();
            let mut f = t.split_whitespace().skip(1);
            let first = f.next().unwrap_or("");
            let msg = if first.is_empty() {
                ""
            } else {
                setup::after_words(t, 2).trim()
            };
            let at = m.flash.as_ref().map(|f| f.1);
            let out = setup::use_setup(m, first, msg);
            if m.flash.as_ref().map(|f| f.1) == at {
                m.flash("rush's own commands start with #: #use");
            }
            out.or_draw()
        }
        "plan" => {
            if !conn(m, key).hosted() {
                m.flash_err("/plan works in rush-mode sessions · /rush moves this one over");
                return Out::Draw;
            }
            let mode = if conn(m, key).info.permission_mode == "plan" || arg == "off" {
                "default"
            } else {
                "plan"
            };
            permissions::set_permission(m, key, mode)
        }
        "diff" => {
            if !views::show_view(m, key, "changes") {
                m.flash_err("no changes view for this agent");
            }
            Out::Draw
        }
        "tasks" => {
            if !["background", "subagents", "tasks"]
                .iter()
                .any(|v| views::show_view(m, key, v))
            {
                m.flash("nothing running in the background");
            }
            Out::Draw
        }
        "copy" => {
            let n = arg.parse::<usize>().ok().filter(|&n| n > 0).unwrap_or(1);
            let t = m
                .session
                .as_ref()
                .filter(|v| v.key == key)
                .map(|v| v.sess.last_answer(n))
                .unwrap_or_default();
            if t.is_empty() {
                m.flash_err("no answer to copy yet");
                return Out::Draw;
            }
            let lines = t.matches('\n').count() + 1;
            m.copy_text(t);
            m.flash(&format!("copied Claude's answer · {lines} lines"));
            Out::Draw
        }
        // #cd with nothing named opens the folders to move it to.
        "rename" | "stop" | "help" | "cd" => {
            if a.is_none() {
                return Out::Draw;
            }
            m.command(format!("#{name} {arg}").trim()).or_draw()
        }
        "background" => {
            leave_pane(m);
            m.flash("it carries on in the background · it's in Agents");
            Out::Draw
        }
        "resume" => {
            leave_pane(m);
            m.flash("past conversations are in Agents: pick one, and a message carries it on");
            Out::Draw
        }
        "model" | "effort" => {
            if arg.is_empty() && open_choices(m, key, name) {
                let kind = claudescreen::kind_of(m, key);
                startsheet::load_models(m, &kind);
                return Out::Draw;
            }
            set_arg(m, key, name, arg)
        }
        _ => Out::Pass,
    }
}

trait OrDraw {
    fn or_draw(self) -> Out;
}

impl OrDraw for Out {
    /// A command typed is rush's whatever came of it: Pass and Idle as
    /// Draw (a flash may have been set).
    fn or_draw(self) -> Out {
        if matches!(self, Out::Pass | Out::Idle) {
            Out::Draw
        } else {
            self
        }
    }
}

/// #model and #effort with the session of the agent they're for open:
/// its /model and /effort; None when it isn't (keys.go:956-967).
pub fn hash_arg(m: &mut Model, name: &str, arg: &str) -> Option<Out> {
    let key = super::open_key(m)?;
    let a = m.selected_row()?;
    if a.key != key || crate::screens::social::rooms::is_room_key(&key) {
        return None;
    }
    Some(run(m, &key, format!("/{name} {arg}").trim()))
}

/// /done: put away, its conversation closes rather than showing the next
/// agent's; on a narrow screen the list comes back. Zen goes on to the
/// next agent that needs you. Asked first, it closes once you say yes.
fn done(m: &mut Model, a: Option<rush_core::agent::Agent>) -> Out {
    use crate::screens::cmdbar::stops::mark_done;
    let stays = a.as_ref().is_none_or(|a| a.done) || m.zen().is_some();
    let had = m.confirm.is_some();
    let out = mark_done(m, a);
    if stays {
        return out;
    }
    if !had
        && let Some(c) = m.confirm.as_mut()
        && let Some(yes) = c.on_yes.take()
    {
        c.on_yes = Some(Box::new(move |m: &mut Model| {
            leave_pane(m);
            yes(m)
        }));
        return out;
    }
    leave_pane(m);
    out
}

/// Back to Agents from the Session; a hosted view's one session stays,
/// there's no list to go back to (leavePane).
pub fn leave_pane(m: &mut Model) {
    if crate::hosted::keeps_open(m) {
        return;
    }
    m.close();
    live::sync(m);
}

/// /rush: a session rush doesn't run moved to rush mode (moveToRush).
fn move_to_rush(m: &mut Model, a: &rush_core::agent::Agent) {
    super::rushmove::move_to_rush(m, a);
}

/// What session key runs with for /name, as its host says, or for a
/// model, as its answers do (argRunning).
pub fn running(m: &mut Model, key: &str, name: &str) -> String {
    let host_model = conn(m, key).info.model.clone();
    let Some(v) = m.session.as_ref().filter(|v| v.key == key) else {
        return if name == "effort" {
            String::new()
        } else {
            host_model
        };
    };
    if name == "effort" {
        return v.sess.effort().to_string();
    }
    [host_model.as_str(), &v.sess.info.model, &v.sess.model]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Switches session key's /name to id, "" (or "default") for the agent's
/// own default, from the next turn. It's kept as picked for the session,
/// and put back as it was when the host refuses it (setArg).
pub fn set_arg(m: &mut Model, key: &str, name: &str, id: &str) -> Out {
    let c = conn(m, key);
    if !c.hosted() && c.sleeping {
        let Some(a) = agent(m, key).cloned() else {
            return Out::Idle;
        };
        let (k, n, i) = (key.to_string(), name.to_string(), id.to_string());
        return switch::wake_then(m, &a, move |m| set_arg(m, &k, &n, &i));
    }
    if !c.hosted() {
        m.flash_err(&format!(
            "/{name} works in rush-mode sessions · /rush moves this one over"
        ));
        return Out::Draw;
    }
    let id = if id == "default" { "" } else { id };
    let kind = claudescreen::kind_of(m, key);
    if name == "model" && m.reg.provider_of(&kind) == "ollama" {
        let mut o = crate::screens::bars::agent::session_start(m, key).unwrap_or_default();
        if o.model != id {
            o.model = id.to_string();
            return switch::switch_session_message(m, key, o, "");
        }
    }
    let c = conn(m, key);
    let was = c.picked.insert(name.to_string(), id.to_string());
    let Some(link) = c.link.clone() else {
        return Out::Draw;
    };
    let shown = if id.is_empty() { "default" } else { id };
    m.flash(&format!("{name}: {shown} from the next turn"));
    let op = if name == "effort" {
        Op::Effort(id.to_string())
    } else {
        Op::Model(id.to_string())
    };
    let (key, name, id) = (key.to_string(), name.to_string(), id.to_string());
    super::later(
        m,
        move || link.op(op),
        move |m, r| {
            let Err(e) = r else { return Out::Idle };
            // Not switched: the list marks what it runs with again.
            let c = conn(m, &key);
            if c.picked.get(&name) == Some(&id) {
                match was {
                    Some(w) => c.picked.insert(name, w),
                    None => c.picked.remove(&name),
                };
            }
            m.fail(&e);
            Out::Draw
        },
    );
    Out::Draw
}

/// Whether session key is one rush hosts, or will once its host wakes;
/// not one read from its transcript (c.client != nil || c.sleeping).
fn rush_mode(m: &Model, key: &str) -> bool {
    m.kept
        .peek::<super::Core>()
        .and_then(|c| c.conns.get(key))
        .is_some_and(|c| c.hosted() || c.sleeping)
}

/// Opens /name's list for session key: the agent's own default first,
/// then its values, the one it runs with marked. False when name has no
/// list (openChoices).
pub fn open_choices(m: &mut Model, key: &str, name: &str) -> bool {
    if name != "model" && name != "effort" {
        return false;
    }
    let kind = claudescreen::kind_of(m, key);
    let opts = if name == "model" {
        setup::models(m, &kind)
    } else {
        choices::arg_options(&m.reg, &kind, name).unwrap_or_default()
    };
    let run = running(m, key, name);
    let picked = conn(m, key).picked.get(name).cloned();
    let (rows, now) = choices::choice_rows(&opts, &run, picked.as_deref());
    let (k, n) = (key.to_string(), name.to_string());
    let mut inner = ChoiceSheet::new(
        name,
        ABOUT,
        rows,
        Box::new(move |m, ch| set_arg(m, &k, &n, &ch.id)),
    );
    inner.now = now;
    inner.cur = now.unwrap_or(0);
    let k = key.to_string();
    inner.on_key = Some(Box::new(move |m, s| {
        if rush_mode(m, &k) {
            return Out::Pass;
        }
        match s {
            "r" => match agent(m, &k).cloned() {
                Some(a) => {
                    move_to_rush(m, &a);
                    Out::Close
                }
                None => Out::Idle,
            },
            "enter" => {
                m.flash_err("Native session · press r to bring its conversation into rush first");
                Out::Draw
            }
            _ => Out::Pass,
        }
    }));
    m.open_sheet(Box::new(ArgSheet {
        conn: key.to_string(),
        kind,
        inner,
    }));
    true
}

/// What the list says it's for in a session rush hosts.
const ABOUT: &str = "for this session, from the next turn";

/// /model or /effort with nothing named (choiceSheet): the frame's list,
/// with the models the agent lists added as they're read, and what it's
/// for said as the session is now.
pub struct ArgSheet {
    pub conn: String,
    pub kind: String,
    pub inner: ChoiceSheet,
}

impl Sheet for ArgSheet {
    fn width(&self, m: &Model) -> usize {
        self.inner.width(m)
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        if self.inner.name == "model" {
            for o in setup::models(m, &self.kind) {
                if !self.inner.opts.iter().any(|r| r.id == o.id) {
                    self.inner.opts.push(Row {
                        id: o.id,
                        note: o.note,
                    });
                }
            }
        }
        self.inner.about = if rush_mode(m, &self.conn) || agent_gone(m, &self.conn) {
            ABOUT.into()
        } else {
            "native session · r brings its conversation into rush to change settings".into()
        };
        self.inner.body(m, w, h)
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        self.inner.key(m, k)
    }
}

/// Whether session key's agent has gone from the list (sheetConn nil).
fn agent_gone(m: &Model, key: &str) -> bool {
    !m.agents.iter().any(|a| a.key == key)
        || m.kept
            .peek::<super::Core>()
            .is_none_or(|c| !c.conns.contains_key(key))
}
