//! A message to a spawned agent from its parent's view: a session rush
//! hosts that the parent started (rush spawn, spawn_agent), watched from
//! the parent's conversation. It goes to the spawned agent's own host,
//! queued for its turn's end, or sent now, which stops its turn and sends
//! it, as in its own view; queued behind a turn that can't take it (Kimi),
//! it says so and how to send it now. Then a note under the conversation
//! says where it went, and one that didn't go comes back to the box. After
//! Go's internal/ui/rushmode.go:4647 (sendPane's watchedHost branch, a88d13b3),
//! subqueue.go (viaSub :64, noteSub :90, onSubSent :101) and
//! cmdbar.go:1756 (sendNowKey).
//!
//! Which spawned agent a message goes to is the subagents view's: the box
//! under the run picked there routes it (relay.rs's send_to).
//!
//! Rust-only: the notes name the agent as wherever it asks you
//! (cards_host::sub_who), and a spawned agent opens as a session of its
//! own (open_session), its row no longer folded into its parent's.

use super::host::hosts;
use super::subqueue::{self, SubNote};
use super::{conn, core};
use crate::frame::Out;
use crate::model::Model;

/// The key that sends now in a Session (sendNowKey).
pub fn send_now_key(m: &mut Model) -> String {
    let k = crate::keybind::bound_key(m, "session.send");
    if k.is_empty() { "ctrl+enter".into() } else { k }
}

/// What a message to a spawned agent of kind says it did (sendPane's how).
pub fn how(m: &mut Model, kind: &str, now: bool) -> String {
    if now {
        return "its turn stopped for it · it reads it now".into();
    }
    if !hosts(m).takes_mid_turn(kind) {
        return format!(
            "in its own queue · its turn can't take it · {} stops the turn and sends it",
            send_now_key(m)
        );
    }
    "in its own queue · it reads it when its turn ends".into()
}

/// Sends the box of session key to the spawned agent run sub, whose host
/// is id and agent kind: queued for its turn's end, or now, stopping it
/// (sendPane, viaSub).
pub fn send(m: &mut Model, key: &str, sub: &str, id: &str, kind: &str, now: bool) -> Out {
    let c = conn(m, key);
    let text = c.input.sent(true).trim().to_string();
    if text.is_empty() {
        return Out::Idle;
    }
    c.input.clear_sent();
    send_text(m, key, (sub, id, kind), text, now, false)
}

/// Sends text to the spawned agent run sub, whose host is id and agent
/// kind; side when it came from the box under the run in the subagents
/// view's column, where it goes back to when it doesn't go.
pub fn send_text(
    m: &mut Model,
    key: &str,
    (sub, id, kind): (&str, &str, &str),
    text: String,
    now: bool,
    side: bool,
) -> Out {
    let how = how(m, kind, now);
    let h = hosts(m);
    let (key, sub, id) = (key.to_string(), sub.to_string(), id.to_string());
    m.later(
        {
            let (id, text) = (id.clone(), text.clone());
            move || h.send_to(&id, &text, now)
        },
        move |m, r| sent(m, &key, &sub, text, how, side, r),
    );
    Out::Draw
}

/// Where a message to the spawned agent went, or the box back when it
/// didn't (onSubSent).
fn sent(
    m: &mut Model,
    key: &str,
    sub: &str,
    text: String,
    how: String,
    side: bool,
    r: Result<(), String>,
) -> Out {
    let name = name_of(m, key, sub);
    if side && let Err(e) = &r {
        super::subbox::give_back(m, key, sub, &text);
        m.fail(&format!("couldn't send to {name}: {e} · back in its box"));
        return Out::Draw;
    }
    let k = subqueue::key(key, sub);
    match r {
        Ok(()) => {
            let notes = core(m).sub_notes.entry(k).or_default();
            subqueue::note(
                notes,
                SubNote {
                    text,
                    how: how.clone(),
                    told: false,
                },
            );
            m.flash(&format!("sent to {name} · {how}"));
        }
        Err(e) => {
            let why = format!("couldn't send to {name}: {e}");
            let c = conn(m, key);
            if c.input.is_empty() {
                c.input.insert_str(&text);
                m.fail(&format!("{why} · back in the box"));
            } else {
                let notes = core(m).sub_notes.entry(k).or_default();
                subqueue::note(
                    notes,
                    SubNote {
                        text,
                        how: format!("✗ not sent: {e}"),
                        told: false,
                    },
                );
                m.fail(&why);
            }
        }
    }
    Out::Draw
}

/// What spawned run sub of session key is called, as wherever it asks
/// you (cards_host::sub_who); "the spawned agent" while its run isn't
/// known.
fn name_of(m: &mut Model, key: &str, sub: &str) -> String {
    let r = &conn(m, key).runs;
    r.hosted_of(sub)
        .map(|x| super::cards_host::sub_who(r, sub, x))
        .unwrap_or_else(|| "the spawned agent".into())
}

/// Opens the spawned agent rush hosts as session id as a session of its
/// own, where its own cards are: at once when it's a row in the list,
/// else it's made one (it's folded into its parent's) and opened once a
/// reading has it. Whether it could be. Rust-only.
pub fn open_session(m: &mut Model, id: &str) -> bool {
    if let Some(a) = m.agents.iter().find(|a| a.rush && a.job.id == id) {
        let key = a.key.clone();
        super::menubar_jump::jump_to(m, key);
        return true;
    }
    if !m.kept::<crate::worker::FleetIn>().unfold(id) {
        m.flash_err("its session can't be opened here");
        return false;
    }
    core(m).open_on_load = id.to_string();
    m.flash("opening its session…");
    true
}
