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
//! Which spawned agent the parent's view is watching is the subagents
//! view's (spawns.go's watchedHost); it calls [`send`] with the host's
//! id once it routes the box there.

use super::host::hosts;
use super::subqueue::{self, SubNote};
use super::{conn, core};
use crate::frame::Out;
use crate::model::Model;

/// The key that sends now in a Session (sendNowKey).
pub fn send_now_key(m: &mut Model) -> String {
    let k = crate::screens::cmdbar::keybind::bound_key(m, "session.send");
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
    c.input.clear();
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
    const NAME: &str = "the spawned agent";
    if side && let Err(e) = &r {
        super::subview::give_back(m, key, &text);
        m.fail(&format!("couldn't send to {NAME}: {e} · back in its box"));
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
            m.flash(&format!("sent to {NAME} · {how}"));
        }
        Err(e) => {
            let why = format!("couldn't send to {NAME}: {e}");
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
