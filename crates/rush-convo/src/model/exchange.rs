//! Messages between agents, apart from what you say to them: a row of
//! their own in the turn they land in. Port of internal/convo/exchange.go
//! (peerLabel, exchange, ExchangeText, ExchangePeer, RestoreExchanges,
//! archivedExchange) and usermessages.go:21-31 (a sent exchange's echo).

use rush_core::event::{Exchange, Peer};
use rush_core::time::Ms;

use super::{ECHOES, Echo, Item, Picture, Session, Turn};

/// A message between agents as a turn keeps it.
#[derive(Clone, Debug, Default)]
pub struct ExchangeItem {
    pub exchange: Exchange,
    /// Embedded images the agent's own copy of it carried.
    pub pictures: Vec<Picture>,
}

/// One end of an exchange in words: its name, kind and session.
pub fn peer_label(p: &Peer) -> String {
    let mut label = if !p.name.is_empty() {
        p.name.clone()
    } else if !p.kind.is_empty() {
        p.kind.clone()
    } else {
        "agent".into()
    };
    if !p.kind.is_empty() && p.kind != label {
        label = format!("{label} · {}", p.kind);
    }
    if !p.session_id.is_empty() {
        label = format!("{label} · {}", p.session_id);
    }
    label
}

/// The peer session a foldable exchange row ("exchange:dir:id:peer") is
/// with, if it's one.
pub fn exchange_peer(r: &str) -> Option<&str> {
    let rest = r.strip_prefix("exchange:")?;
    Some(&rest[rest.rfind(':').map_or(0, |i| i + 1)..])
}

impl Session {
    /// An exchange: a row of its own. A received message's own copy only
    /// attaches to that row, never makes another prompt of yours. Says
    /// where it is: (turn, item).
    pub(super) fn exchange(&mut self, e: &Exchange, now: Ms) -> (usize, usize) {
        let key: Box<str> = format!("{}:{}", e.direction, e.id).into();
        if !e.id.is_empty()
            && let Some(&at) = self.exchanges.get(&key)
        {
            return at;
        }
        if e.archived {
            return self.archived_exchange(e, key, now);
        }
        let sent_or_result = e.direction == "sent" || e.phase == "result";
        let ti = match self.live_idx() {
            Some(i) => i,
            None if sent_or_result && !self.turns.is_empty() => self.turns.len() - 1,
            None => {
                let i = self.turn_for(now);
                let t = &mut self.turns[i];
                if sent_or_result {
                    t.live = false;
                    t.end = now;
                    t.from = "agent exchange".into();
                } else {
                    t.from = peer_label(&e.sender);
                }
                i
            }
        };
        let t = &mut self.turns[ti];
        t.items.push(Item::Exchange(Box::new(ExchangeItem {
            exchange: e.clone(),
            pictures: Vec::new(),
        })));
        t.touch();
        let at = (ti, t.items.len() - 1);
        if !e.id.is_empty() {
            self.exchanges.insert(key, at);
        }
        at
    }

    /// You (through rush) sent another agent's message to this one: its row,
    /// and the agent's own copy of it matched to that row (Go's sent of a
    /// host.Sent with an Exchange).
    pub fn sent_exchange(&mut self, e: &Exchange, now: Ms) {
        self.applied += 1;
        let (turn, item) = self.exchange(e, now);
        if self.echoes.len() == ECHOES {
            self.echoes.remove(0);
        }
        self.echoes.push(Echo {
            text: e.text.clone(),
            turn,
            item: Some(item),
        });
    }

    /// rush's news of a pull request as the agent's transcript keeps it,
    /// handed over mid-turn: its card where it was taken, until rush's own
    /// record of it, restored, takes its place (Rust-only).
    pub(crate) fn news_taken(&mut self, text: &str, now: Ms) {
        // Already its card: rush delivered it while this session watched.
        let shown = self.turns.iter().rev().take(2).any(|t| {
            t.items
                .iter()
                .any(|it| matches!(it, Item::Exchange(x) if x.exchange.text == text))
        });
        if shown {
            return;
        }
        let pr = crate::transcript::pr_news_parts(text).map_or("", |(pr, _)| pr);
        let e = Exchange {
            at: now,
            direction: "received".into(),
            phase: "message".into(),
            text: text.into(),
            sender: Peer {
                kind: "rush".into(),
                name: format!("rush · {pr}"),
                ..Peer::default()
            },
            ..Exchange::default()
        };
        self.applied += 1;
        self.exchange(&e, now);
    }

    /// Merges rush's archived records of exchanges into a transcript read
    /// back: the agent's copy of a received message becomes its row.
    pub fn restore_exchanges(&mut self, records: &[Exchange]) {
        for e in records {
            let mut e = e.clone();
            e.archived = true;
            let at = e.at;
            self.exchange(&e, at);
        }
    }

    /// The whole of the selected exchange, who to whom first.
    pub fn exchange_text(&self, r: &str) -> Option<String> {
        let rest = r.strip_prefix("exchange:")?;
        let mut parts = rest.split(':');
        let (dir, id) = (parts.next()?, parts.next()?);
        let &(ti, ii) = self.exchanges.get(format!("{dir}:{id}").as_str())?;
        let Some(Item::Exchange(x)) = self.turns.get(ti).and_then(|t| t.items.get(ii)) else {
            return None;
        };
        let e = &x.exchange;
        let mut text = format!(
            "{} → {}\n\n{}",
            peer_label(&e.sender),
            peer_label(&e.receiver),
            e.text
        );
        for image in &e.images {
            text.push('\n');
            text.push_str(image);
        }
        Some(text)
    }

    fn archived_exchange(&mut self, e: &Exchange, key: Box<str>, now: Ms) -> (usize, usize) {
        const NEAR: Ms = 2 * 60 * 1000;
        let now = if e.at != 0 { e.at } else { now };
        let mut target = None;
        let mut replace = None;
        let mut matched_prompt = false;
        let mut best = Ms::MAX;
        // The agent's copy of a message rush injected (PR news, a task's
        // end) reads as its line, not its whole text (Rust-only).
        let line = crate::transcript::injected(&e.text).map(|(_, t)| t);
        let same = |s: &str| !e.text.is_empty() && (s == e.text || line.as_deref() == Some(s));
        if e.direction == "received" && e.phase != "result" && e.at != 0 {
            for (ti, t) in self.turns.iter().enumerate() {
                if t.start == 0 {
                    continue;
                }
                let distance = (t.start - now).abs();
                if distance > NEAR {
                    continue;
                }
                if same(&t.prompt) && distance < best {
                    (target, replace, matched_prompt, best) = (Some(ti), None, true, distance);
                }
                for (ii, it) in t.items.iter().enumerate() {
                    if let Item::Interject(x) = it
                        && same(&x.text)
                        && distance < best
                    {
                        (target, replace, matched_prompt, best) =
                            (Some(ti), Some(ii), false, distance);
                    }
                }
            }
            // Taken mid-turn in a turn begun long before: its copy in the
            // turn it arrived in, the first not yet matched (Rust-only).
            let ran = self
                .turns
                .iter()
                .rposition(|t| t.start != 0 && t.start <= now)
                .filter(|&ti| {
                    let t = &self.turns[ti];
                    t.live || t.end == 0 || now <= t.end + NEAR
                });
            if target.is_none()
                && let Some(ti) = ran
                && let Some(ii) = self.turns[ti].items.iter().position(|it| match it {
                    Item::Interject(x) => same(&x.text),
                    // news_taken's: the transcript's copy, no record yet.
                    Item::Exchange(x) => x.exchange.id.is_empty() && x.exchange.text == e.text,
                    _ => false,
                })
            {
                (target, replace) = (Some(ti), Some(ii));
            }
        }
        if let Some(ti) = target {
            if replace.is_none() {
                let t = &mut self.turns[ti];
                t.prompt.clear();
                t.from = peer_label(&e.sender);
            }
        } else {
            // Report-only records attach by time; they never wake a past
            // turn.
            target = self.turns.iter().rposition(|t| t.start <= now);
            if target.is_none() && !self.turns.is_empty() {
                target = Some(0);
            }
        }
        let ti = target.unwrap_or_else(|| {
            self.turns.push(Turn {
                n: 1,
                start: now,
                end: now,
                from: "agent exchange".into(),
                ..Turn::default()
            });
            self.turns.len() - 1
        });
        let it = Item::Exchange(Box::new(ExchangeItem {
            exchange: e.clone(),
            pictures: Vec::new(),
        }));
        let t = &mut self.turns[ti];
        let ii = match replace {
            Some(ii) => {
                t.items[ii] = it;
                ii
            }
            None if matched_prompt => {
                t.items.insert(0, it);
                // What sat behind it moved down one.
                self.shift_items(ti, 0);
                0
            }
            None => {
                t.items.push(it);
                t.items.len() - 1
            }
        };
        self.turns[ti].touch();
        self.exchanges.insert(key, (ti, ii));
        (ti, ii)
    }

    /// An item was put in at turn ti's index from: what the session keeps
    /// pointing at its items from there on moves down one.
    fn shift_items(&mut self, ti: usize, from: usize) {
        for at in self.exchanges.values_mut() {
            if at.0 == ti && at.1 >= from {
                at.1 += 1;
            }
        }
        for e in &mut self.echoes {
            if e.turn == ti
                && let Some(i) = e.item.as_mut()
                && *i >= from
            {
                *i += 1;
            }
        }
        if let Some((t, i)) = self.streaming.as_mut()
            && *t as usize == ti
            && *i as usize >= from
        {
            *i += 1;
        }
    }
}
