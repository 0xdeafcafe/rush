//! `@kimi …` in a conversation's box: a message that starts by tagging
//! other models asks them, as guests of this conversation, rather than its
//! own agent (rush-host's guests, docs/guests.md). Which agents the tags
//! name is the host module's to say, off the UI's thread; a tag that names
//! none sends the message to the session's agent as typed. Rush-only.

use crate::frame::Out;
use crate::model::Model;

use super::{agent, conn, later};

/// How much of the conversation a guest new to it is given.
const BEFORE: usize = 12 << 10;

/// Whether text starts by tagging someone: it may be for guests.
pub fn tagged(text: &str) -> bool {
    rush_host::guests::mention::leading(text).is_some()
}

/// Asks the guests text tags, from session key's conversation; text that
/// tags no agent goes to the session's agent as typed, and one that can't
/// be asked comes back to the box, saying why.
pub fn send(m: &mut Model, key: &str, text: String) -> Out {
    let lead = agent(m, key).map(|a| a.job.id.clone()).unwrap_or_default();
    if lead.is_empty() {
        super::keys::send_as(m, key, &text);
        return Out::Draw;
    }
    let before = m
        .session
        .as_ref()
        .filter(|v| v.key == key)
        .map(|v| super::told::latest(&v.sess, BEFORE))
        .unwrap_or_default();
    let reg = m.reg.clone();
    conn(m, key).input.clear_sent();
    let k = key.to_string();
    later(
        m,
        move || {
            let r = rush_host::guests::route(&reg, &lead, &text, &before);
            (r, text)
        },
        move |m, (r, text)| match r {
            Ok(Some(said)) => {
                m.flash(&said);
                Out::Draw
            }
            Ok(None) => {
                super::keys::send_as(m, &k, &text);
                Out::Draw
            }
            Err(e) => {
                let c = conn(m, &k);
                if c.input.is_empty() {
                    c.input.set(&text);
                }
                m.flash_err(&e);
                Out::Draw
            }
        },
    );
    Out::Draw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_may_be_for_guests() {
        for (text, want) in [
            ("@kimi look", true),
            ("@kimi @sol look", true),
            ("look @kimi", false),
            ("@src/main.rs is broken", false),
            ("plain", false),
        ] {
            assert_eq!(tagged(text), want, "{text}");
        }
    }
}
