//! An agent's own screens, with the terminal handed to them: a background
//! session opened full screen in its agent's program (#full, the bar's
//! "Open full screen"), and the agent's own list of its sessions. After
//! Go's internal/ui/model.go:2663-2687 (attach: the Joiner's screen run
//! with tea.Exec), :1327-1353 (attachDoneMsg: a session its agent let go
//! carries on in rush mode, one opened elsewhere says so, else the
//! Attacher's own command), :1300 (doneMsg), :2766-2775 (nativeView:
//! the SessionsViewer), keys.go:1117-1122 (#full) and cmdbar.go:802-806.
//! The commands are made off the UI thread (they look for the agent's
//! program), and run through Model::exec (frame/outside.rs).
//!
//! First the agent is marked seen (model.go:898 markSeen) and the
//! preview's live mirror let go: one attach at a time, as the preview's
//! would fight the full screen over the session's size (model.go:2683
//! closeLive). The mirror is the preview's to own: it sets
//! [`Attached::close_live`], and opens none while [`attached`] says one
//! is (live.go:238).

use rush_core::adapter::{Elsewhere, Gone, caused_by};
use rush_core::agent::{Agent, State};
use rush_fleet::Predicates;

use super::outside::profile_of;
use crate::frame::Out;
use crate::model::Model;

/// How a session's screen in rush's terminal ended.
enum Joined {
    Ok,
    /// The agent has let the session go.
    Gone,
    /// It was opened in another window, which took it.
    Elsewhere,
    Failed(String),
}

/// Which session has the terminal (m.attached), and how the preview's
/// live mirror is let go.
#[derive(Default)]
pub struct Attached {
    /// The agent whose screen has the terminal; "" when none has.
    pub key: String,
    /// Lets go of the preview's live mirror (closeLive): set by the
    /// preview that keeps one.
    pub close_live: Option<fn(&mut Model)>,
}

/// The agent whose own screen has the terminal now, if one has: the
/// preview opens no live mirror meanwhile (live.go:238).
pub fn attached(m: &Model) -> Option<&str> {
    m.kept
        .peek::<Attached>()
        .map(|a| a.key.as_str())
        .filter(|k| !k.is_empty())
}

/// Notes that you've seen a, when it was waiting on you, so the list
/// stops saying so: kept in rush's state.json, off the UI thread
/// (markSeen).
pub fn mark_seen(m: &mut Model, a: &Agent) {
    if a.job.state != State::Blocked && !a.halted() && !a.your_turn(m.now) {
        return;
    }
    let key = a.key.clone();
    m.later(
        move || {
            let mut s = rush_state::Store::load();
            s.overlay.seen.insert(key, rush_state::Time::now());
            s.save_overlay().map_err(|e| e.to_string())
        },
        // Go lets a failed save go unsaid (model.go:903).
        |_, _: Result<(), String>| Out::Idle,
    );
}

/// Opens a, a background session, full screen in its agent's program,
/// the terminal handed to it until you leave (attach).
pub fn attach(m: &mut Model, a: Option<Agent>) -> Out {
    let Some(a) = a else {
        return Out::Idle;
    };
    m.kept::<Attached>().key = a.key.clone();
    mark_seen(m, &a);
    let name = a.display_name.clone();
    if a.interactive {
        m.flash(&format!(
            "{name} is open in another terminal (pid {})",
            a.pid
        ));
        return Out::Draw;
    }
    if a.past {
        m.flash(&format!(
            "{name} is a past conversation · a message resumes it in rush mode"
        ));
        return Out::Draw;
    }
    // One attach at a time from here: the preview's would fight the full
    // screen over the session's size.
    if let Some(close) = m.kept::<Attached>().close_live {
        close(m);
    }
    let Some(j) = m
        .reg
        .get(a.kind.as_str())
        .filter(|ad| ad.joiner().is_some())
    else {
        m.kept::<Attached>().key.clear();
        m.flash_err(&format!(
            "couldn't open {name}: its agent has no screen of its own to open"
        ));
        return Out::Draw;
    };
    let p = acct_of(m, &a);
    let mut term = j.joiner().expect("filtered").join(&p, &a.job.id);
    m.exec_with(
        move || match term.run() {
            Ok(()) => Joined::Ok,
            Err(e) if caused_by::<Elsewhere>(&*e) => Joined::Elsewhere,
            Err(e) if caused_by::<Gone>(&*e) => Joined::Gone,
            Err(e) => Joined::Failed(e.to_string()),
        },
        move |m, r| joined(m, a, r),
    );
    Out::Draw
}

/// Back from a session's screen (attachDoneMsg): the terminal is rush's
/// again.
fn joined(m: &mut Model, a: Agent, r: Joined) -> Out {
    m.kept::<Attached>().key.clear();
    let e = match r {
        Joined::Ok => return Out::Draw,
        Joined::Elsewhere => {
            m.flash("opened in another window");
            return Out::Draw;
        }
        Joined::Gone => {
            // Its agent has let the job go; its conversation carries on here.
            crate::screens::session_core::keys::adopt(m, &a, "");
            return Out::Draw;
        }
        Joined::Failed(e) => e,
    };
    let Some(ad) = m
        .reg
        .get(a.kind.as_str())
        .filter(|ad| ad.attacher().is_some())
    else {
        m.fail(&format!("couldn't open {}: {e}", a.display_name));
        return Out::Draw;
    };
    // The command is made (its program looked for) before the terminal is
    // handed over, off the UI thread.
    let p = acct_of(m, &a);
    let id = a.job.id.clone();
    m.later(
        move || ad.attacher().expect("filtered").attach(&p, &id),
        |m, cmd| {
            m.exec(cmd, Box::new(done));
            Out::Draw
        },
    );
    Out::Draw
}

/// The profile a runs in (a.Acct), else the one its agent knows by its
/// account's name.
fn acct_of(m: &Model, a: &Agent) -> rush_core::adapter::Profile {
    if a.acct.name.is_empty() && a.acct.dir.is_empty() {
        profile_of(&m.reg, a)
    } else {
        a.acct.clone()
    }
}

/// What an agent's program said as it ended (doneMsg).
fn done(m: &mut Model, r: crate::frame::outside::Ran) -> Out {
    if let Err(e) = r {
        m.fail(&e);
    }
    Out::Draw
}

/// #full: the agent picked, a background session, full screen in its
/// agent's program (keys.go:1117).
pub fn full_command(m: &mut Model, _: &str) -> Out {
    let Some(a) = m.selected_row() else {
        m.flash_err("select an agent first");
        return Out::Draw;
    };
    if a.rush || a.interactive || a.past {
        m.flash_err("only a Claude Code agent in the background opens full screen");
        return Out::Draw;
    }
    attach(m, Some(a))
}

/// Whether the bar offers a full screen of a: a background session its
/// agent runs (cmdbar.go:802).
pub fn offers_full(a: &Agent) -> bool {
    !a.rush && !a.interactive && a.pid != 0
}

/// The logins agent's own list of its sessions, on the account in use, in
/// the terminal (nativeView). Nothing when it has none.
pub fn native_view(m: &mut Model) -> Out {
    use crate::screens::accounts::look::LOGINS_KIND;
    let Some(ad) = m
        .reg
        .get(LOGINS_KIND)
        .filter(|a| a.sessions_viewer().is_some())
    else {
        return Out::Idle;
    };
    let Some(p) = crate::screens::accounts::rows::profile_of(m, &ad) else {
        return Out::Idle;
    };
    // Making the command looks its program up: not on the UI thread.
    m.later(
        move || ad.sessions_viewer().expect("filtered").sessions_view(&p),
        |m, cmd| {
            m.exec(cmd, Box::new(done));
            Out::Draw
        },
    );
    Out::Draw
}
