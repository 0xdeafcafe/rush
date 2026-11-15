//! A host's part in a conversation with guests (docs/guests.md): what its
//! agent says goes in the chat, a guest's reply is a card in the lead's
//! conversation, the lead is told what guests said atop its next turn,
//! and a file-changing ask waits while another participant holds the
//! write lease. Rust-only.

use std::sync::Arc;
use std::time::Duration;

use rush_core::adapter::Conn;
use rush_core::event::{self, Event, Exchange, Message, PartKind, Peer};
use rush_room::room::{Entry, SAY, TOOL, USER};
use rush_state::Time;
use serde_json::json;

use super::{Host, State, same_conn};
use crate::guests::{self, GUEST_NAME, GUEST_OF, PHASE, chat, lease, prompt, roster};

/// How often held asks look at the lease again.
const LEASE_LOOK: Duration = Duration::from_secs(1);

/// What the host keeps of its part in a conversation with guests.
#[derive(Default)]
pub(super) struct GuestState {
    /// Who asked the turn under way, for a guest: its reply goes to them.
    asker: Option<Peer>,
    /// What its turn did, a line a call, for a guest's card.
    did: Vec<String>,
    /// File-changing asks held for the lease, each with the agent that
    /// asked.
    parked: Vec<(Box<event::Approval>, Arc<dyn Conn>)>,
    /// A look at the lease is due.
    looking: bool,
}

impl Host {
    /// The conversation this session takes part in (its lead's id), and
    /// its name there.
    fn chat_of(&self, st: &State) -> Option<(String, String)> {
        let lead = guests::lead_of(&st.cfg.id, &st.cfg.meta)?;
        let name = match st.cfg.meta.get(GUEST_NAME) {
            Some(n) if !n.is_empty() => n.clone(),
            _ => {
                let r = roster::load(&lead);
                if r.lead.is_empty() {
                    guests::lead_name(&self.reg, &st.cfg.kind)
                } else {
                    r.lead
                }
            }
        };
        Some((lead, name))
    }

    /// Whether this session is a guest (rather than a lead).
    fn is_guest(st: &State) -> bool {
        st.cfg.meta.get(GUEST_OF).is_some_and(|l| !l.is_empty())
    }

    /// What the lead is told atop the turn a message starts, of what was
    /// said since its last by others, and the time it was told up to; None
    /// for a guest, a session with no guests, or nothing new.
    pub(super) fn guests_note(&self, st: &State) -> Option<(String, Time)> {
        if Self::is_guest(st) {
            return None;
        }
        let (lead, name) = self.chat_of(st)?;
        let r = roster::load(&lead);
        let es = chat::read(&lead);
        let new = chat::after(&es, r.lead_seen);
        let last = new.last()?.at;
        let budget = rush_room::seen::budget(r.guests.iter().map(|g| g.context));
        let note = prompt::lead_note(new, &name, budget);
        Some((note, last)).filter(|(n, _)| !n.is_empty())
    }

    /// The lead has been told the chat up to seen.
    pub(super) fn guests_told(&self, st: &State, seen: Time) {
        let id = st.cfg.id.clone();
        if let Err(e) = roster::update(&id, |r| {
            if seen.after(r.lead_seen) {
                r.lead_seen = seen;
            }
        }) {
            self.event(
                st,
                "guests-failed",
                json!({"err": e.to_string(), "at": "told"}),
            );
        }
    }

    /// A message the agent was given: for a lead, yours goes in the chat
    /// (and ends the count of asks between agents); for a guest, who asked
    /// is kept, for its reply.
    pub(super) fn guests_given(&self, st: &mut State, text: &str, ex: Option<&Exchange>) {
        if Self::is_guest(st) {
            if let Some(e) = ex {
                st.gs.asker = Some(e.sender.clone());
                st.gs.did.clear();
            }
            return;
        }
        if ex.is_some() || text.trim().is_empty() {
            return; // an agent's or rush's: said where it was asked
        }
        let Some((lead, _)) = self.chat_of(st) else {
            return;
        };
        let _ = roster::update(&lead, |r| r.hops = 0);
        self.chat_say(
            st,
            &lead,
            Entry {
                from: USER.into(),
                kind: SAY.into(),
                text: text.to_string(),
                ..Entry::default()
            },
        );
    }

    fn chat_say(&self, st: &State, lead: &str, e: Entry) {
        if let Err(err) = chat::append(lead, e) {
            self.event(
                st,
                "guests-failed",
                json!({"err": err.to_string(), "at": "chat"}),
            );
        }
    }

    /// The agent's own message: each call a line in the chat, and one that
    /// changes files takes the lease when it's free (an agent whose mode
    /// never asks is only seen, not held: docs/guests.md).
    pub(super) fn guests_message(&self, st: &mut State, m: &Message) {
        if m.role != "assistant" || !m.parent.is_empty() {
            return;
        }
        let calls: Vec<&rush_core::tool::Call> = m
            .parts
            .iter()
            .filter(|p| p.kind == PartKind::ToolCall)
            .filter_map(|p| p.call.as_deref())
            .collect();
        if calls.is_empty() {
            return;
        }
        let Some((lead, name)) = self.chat_of(st) else {
            return;
        };
        for c in calls {
            let did = rush_room::doing::doing(c);
            self.chat_say(
                st,
                &lead,
                Entry {
                    from: name.clone(),
                    kind: TOOL.into(),
                    text: did.clone(),
                    id: c.id.clone(),
                    ..Entry::default()
                },
            );
            if Self::is_guest(st) {
                st.gs.did.push(did);
            }
            if lease::changes_files(c, &st.info.cwd) {
                match lease::take(&lead, &st.cfg.id, &name, st.turn) {
                    Ok(lease::Took::Held(l)) => self.event(
                        st,
                        "lease-overlap",
                        json!({"holder": l.holder, "name": l.name, "call": rush_room::doing::doing(c)}),
                    ),
                    Ok(lease::Took::Mine) => {}
                    Err(e) => self.event(st, "guests-failed", json!({"err": e.to_string(), "at": "lease"})),
                }
            }
        }
    }

    /// Holds approval a while another participant holds the lease: the ask
    /// isn't shown or answered until it's free (looked at every second).
    /// Whether it was held.
    pub(super) fn lease_parks(
        &self,
        st: &mut State,
        conn: &Arc<dyn Conn>,
        a: &event::Approval,
    ) -> bool {
        if !lease::changes_files(&a.call, &st.info.cwd) {
            return false;
        }
        let Some((lead, name)) = self.chat_of(st) else {
            return false;
        };
        let l = match lease::take(&lead, &st.cfg.id, &name, st.turn) {
            Ok(lease::Took::Held(l)) => l,
            _ => return false,
        };
        self.event(
            st,
            "lease-wait",
            json!({"id": a.id, "holder": l.holder, "name": l.name, "call": rush_room::doing::doing(&a.call)}),
        );
        st.gs.parked.push((Box::new(a.clone()), conn.clone()));
        st.info.detail = format!("waiting for {} to finish editing", l.name);
        self.publish(st);
        self.look_at_lease(st);
        true
    }

    fn look_at_lease(&self, st: &mut State) {
        if st.gs.looking {
            return;
        }
        st.gs.looking = true;
        let h = self.clone();
        self.timers.after(LEASE_LOOK, move || {
            let h2 = h.clone();
            h.go("lease", move |_| h2.lease_freed());
        });
    }

    /// Lets the held asks go once the lease is theirs, each as it would
    /// have gone; looks again while any still waits.
    fn lease_freed(&self) {
        let mut st = self.lock();
        st.gs.looking = false;
        let parked = std::mem::take(&mut st.gs.parked);
        // No conversation any more: what was held goes as it would have.
        let chat = self.chat_of(&st);
        let mut waiting = None;
        for (a, conn) in parked {
            if !same_conn(&st.conn, &Some(conn.clone())) {
                continue; // its agent has gone
            }
            if waiting.is_some() {
                st.gs.parked.push((a, conn));
                continue;
            }
            let took = match &chat {
                Some((lead, name)) => lease::take(lead, &st.cfg.id, name, st.turn),
                None => Ok(lease::Took::Mine),
            };
            match took {
                Ok(lease::Took::Held(l)) => {
                    waiting = Some(l.name);
                    st.gs.parked.push((a, conn));
                }
                _ => {
                    self.event(&st, "lease-taken", json!({"id": a.id}));
                    st.info.detail.clear();
                    self.on_agent_event(&mut st, &conn, Event::Approval(a));
                }
            }
        }
        if let Some(n) = waiting {
            st.info.detail = format!("waiting for {n} to finish editing");
            self.look_at_lease(&mut st);
        }
        self.publish(&mut st);
    }

    /// An ask the agent called off is held no longer.
    pub(super) fn lease_cancelled(st: &mut State, id: &str) {
        st.gs.parked.retain(|(a, _)| a.id != id);
    }

    /// The turn ended: its last words go in the chat, the lease goes, and a
    /// guest's reply is a card in the lead's conversation.
    pub(super) fn guests_turn_end(&self, st: &mut State, e: &event::TurnEnd) {
        st.gs.parked.clear();
        let Some((lead, name)) = self.chat_of(st) else {
            return;
        };
        lease::release(&lead, &st.cfg.id);
        let said = if st.last_said.is_empty() {
            e.text.trim().to_string()
        } else {
            st.last_said.trim().to_string()
        };
        let asker = st.gs.asker.take();
        let did = std::mem::take(&mut st.gs.did);
        let to = asker.as_ref().map_or(String::new(), asked_by);
        if !said.is_empty() {
            self.chat_say(
                st,
                &lead,
                Entry {
                    from: name.clone(),
                    kind: SAY.into(),
                    text: said.clone(),
                    to: if to == USER { "you".into() } else { to.clone() },
                    ..Entry::default()
                },
            );
        }
        if !Self::is_guest(st) || (said.is_empty() && did.is_empty()) {
            return;
        }
        let text = if said.is_empty() {
            format!("({}, and nothing said)", e.reason)
        } else {
            said
        };
        let card = Exchange {
            id: crate::paths::rand_bytes::<16>()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            at: rush_core::time::now(),
            phase: PHASE.into(),
            text: prompt::reply(&text, &did, &to),
            sender: Peer {
                session_id: st.cfg.id.clone(),
                kind: rush_core::adapter::migrated(&st.cfg.kind).0,
                name,
            },
            receiver: asker.unwrap_or_else(|| Peer {
                name: "you".into(),
                ..Peer::default()
            }),
            ..Exchange::default()
        };
        // Off the lock: the lead's host may take a moment to answer.
        self.go("guest-reply", move |_| {
            guests::ask::show_in_lead(&lead, card);
        });
    }
}

/// Who asked, as the chat names them: USER for you.
fn asked_by(p: &Peer) -> String {
    if p.session_id.is_empty() && (p.name.is_empty() || p.name == "you") {
        USER.into()
    } else {
        p.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_asked() {
        let p = |id: &str, name: &str| Peer {
            session_id: id.into(),
            name: name.into(),
            ..Peer::default()
        };
        for (peer, want) in [
            (p("", "you"), USER),
            (p("", ""), USER),
            (p("abc", "Claude"), "Claude"),
            (p("abc", "you"), "you"),
        ] {
            assert_eq!(asked_by(&peer), want);
        }
    }
}
