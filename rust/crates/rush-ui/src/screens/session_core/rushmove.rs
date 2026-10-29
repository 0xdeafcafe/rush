//! Moving a session rush doesn't run into rush mode: /rush, #rush, a
//! message to a past one, or one whose daemon let it go. The agent's own
//! copy stops (the conversation kept) and the same conversation resumes
//! under rush's host, which runs it headless from then on; one open in a
//! terminal is copied, its terminal left as it is. One mid-turn moves
//! when the turn ends. After Go's internal/ui/rushmode.go (canResume
//! :5330, moveToRush :5456, moveToRushWith :5459, movePending :5529,
//! movedToRushMsg :5543) and model.go:1016-1049 (movedToRushMsg: the old
//! row put away, its queued messages sent to the new session, the name
//! you gave it kept). The work itself is the binary's
//! [`super::host::Hosts::adopt`], off the UI thread.

use std::collections::HashSet;

use rush_core::agent::Agent;

use super::host::{Started, hosts};
use super::{agent, busy, core, keys};
use crate::frame::Out;
use crate::model::Model;

/// The agents waiting for their turn to end to move to rush mode, by key
/// (Go's m.moveWhenIdle).
#[derive(Default)]
pub struct MoveWhenIdle(pub HashSet<String>);

/// /rush and #rush: a moved to rush mode (moveToRush).
pub fn move_to_rush(m: &mut Model, a: &Agent) {
    move_to_rush_with(m, a, "");
}

/// a moved to rush mode with prompt as its first message there
/// (moveToRushWith).
pub fn move_to_rush_with(m: &mut Model, a: &Agent, prompt: &str) {
    let name = &a.display_name;
    if a.rush {
        m.flash(&format!("{name} already runs in rush mode"));
        return;
    }
    if !keys::can_resume(m, a) {
        return;
    }
    if a.job.session_id.is_empty() {
        m.fail(&format!("can't find {name}'s conversation to resume"));
        return;
    }
    if a.headless {
        m.flash_err(&format!(
            "{name} is driven by another program; rush can't take it over"
        ));
        return;
    }
    let waiting = &mut m.kept::<MoveWhenIdle>().0;
    if !a.interactive && busy(a) && !waiting.contains(&a.key) {
        // Stopping it now would lose the turn in progress.
        waiting.insert(a.key.clone());
        m.flash(&format!(
            "{name} moves to rush mode when this turn ends · /rush again moves it now"
        ));
        return;
    }
    waiting.remove(&a.key);
    if a.interactive {
        m.flash(&format!(
            "copying {name} into rush mode · the terminal one is left as it is"
        ));
    } else {
        m.flash(&format!("moving {name} to rush mode…"));
    }
    let h = hosts(m);
    let (a, prompt) = (a.clone(), prompt.to_string());
    let from = a.clone();
    super::later(
        m,
        move || h.adopt(&a, &prompt),
        move |m, r| moved(m, &from, r),
    );
}

/// After each reading of the list, in the rush that leads: those waiting
/// for their turn to end move once it has, and those gone or already
/// moved stop waiting (movePending, model.go:1103).
pub fn move_pending(m: &mut Model) {
    if !m.leads() {
        return;
    }
    let Some(w) = m.kept.peek::<MoveWhenIdle>() else {
        return;
    };
    if w.0.is_empty() {
        return;
    }
    let keys: Vec<String> = w.0.iter().cloned().collect();
    for key in keys {
        match agent(m, &key).cloned() {
            None => {
                m.kept::<MoveWhenIdle>().0.remove(&key);
            }
            Some(a) if a.rush => {
                m.kept::<MoveWhenIdle>().0.remove(&key);
            }
            Some(a) if !busy(&a) => move_to_rush(m, &a),
            Some(_) => {}
        }
    }
}

/// It moved, or why not (movedToRushMsg): the old row is finished, the
/// messages still waiting for it go to the new session, the name you
/// gave it is the new one's, and the new one is picked and opened.
fn moved(m: &mut Model, from: &Agent, r: Result<Started, String>) -> Out {
    let st = match r {
        Ok(st) => st,
        Err(e) => {
            m.fail(&e);
            return Out::Draw;
        }
    };
    let new_key = rush_state::dir::key(&st.acct, &format!("a:{}", st.id));
    let old = from.key.clone();
    if agent(m, &old).is_some_and(|a| !a.done) {
        crate::screens::cmdbar::stops::toggle_done(m, &old);
    }
    // Its name is the new row's.
    let (o, n) = (old.clone(), new_key.clone());
    m.later(
        move || {
            let mut s = rush_state::Store::load();
            s.overlay.done.insert(o.clone(), rush_state::Time::now());
            match s.overlay.names.get(&o).cloned() {
                Some(name) if !name.is_empty() => {
                    s.overlay.names.insert(n, name);
                }
                _ => {}
            }
            s.save_overlay().map_err(|e| e.to_string())
        },
        |m, r: Result<(), String>| {
            if let Err(e) = r {
                m.fail(&e);
            }
            Out::Idle
        },
    );
    // Messages still waiting for the old row go to the new session.
    let items = core(m)
        .local_q
        .remove(&old)
        .map(|q| q.items)
        .unwrap_or_default();
    if !items.is_empty() {
        let text = super::host::join_queue(&items);
        let (h, id) = (hosts(m), st.id.clone());
        super::later(
            m,
            move || h.send_to(&id, &text, false),
            |m, r| {
                match r {
                    Ok(()) => m.flash("queued messages moved over"),
                    Err(e) => m.fail(&e),
                }
                Out::Draw
            },
        );
    }
    let out = super::start::started(m, Ok(st));
    if from.interactive {
        m.flash(&format!(
            "{} carries on in rush mode · its terminal copy is still open there, now under Done",
            from.display_name
        ));
    }
    out
}
