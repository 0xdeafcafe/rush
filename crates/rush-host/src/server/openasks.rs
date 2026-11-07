//! What the agent asks stays shown until it's answered. A client draws an
//! ask from its line in the ring, and lets every ask of the main agent go
//! when its turn ends: an ask that outlives its turn (Claude Code asks
//! whether to turn on the mods it writes outside any turn) would be
//! gone from the view while the host still waits on it, every message
//! held behind it. So each ask still open when a turn ends is said again
//! after that end, and the ring, trimming old turns, keeps the latest
//! line of each ask still open.
//!
//! Rust-only: Go let both go.

use std::sync::Arc;

use rush_core::event::Event;

use super::{Host, State};
use crate::lines;
use crate::pack::{line_len, pack};

/// An ask the agent made, as it's said again.
#[derive(Clone, Debug)]
pub(super) struct OpenAsk {
    /// When it was asked among the others: they're said in that order.
    pub seq: u64,
    /// Its event line.
    pub line: Arc<[u8]>,
    /// Its latest copy in the ring, once said again.
    pub kept: Option<Arc<[u8]>>,
}

impl Host {
    /// Keeps ev, an approval or question asked as id, to say again.
    pub(super) fn note_ask(&self, st: &mut State, id: &str, ev: &Event) {
        let Ok(line) = lines::event_line(ev) else {
            return;
        };
        st.ask_seq += 1;
        st.open_asks.insert(
            id.to_string(),
            OpenAsk {
                seq: st.ask_seq,
                line: line.into(),
                kept: None,
            },
        );
    }

    /// The asks still waiting on the user, oldest first.
    fn open_ids(st: &State) -> Vec<String> {
        let mut v: Vec<(&String, u64)> = st
            .open_asks
            .iter()
            .filter(|(id, _)| st.pending.contains_key(*id))
            .map(|(id, a)| (id, a.seq))
            .collect();
        v.sort_by_key(|&(_, s)| s);
        v.into_iter().map(|(id, _)| id.clone()).collect()
    }

    /// Says again, after a turn's end, each ask still open, so a client
    /// that let it go with the turn shows it again.
    pub(super) fn reask(&self, st: &mut State) {
        st.open_asks.retain(|id, _| st.pending.contains_key(id));
        for id in Self::open_ids(st) {
            let line = st.open_asks[&id].line.to_vec();
            self.record(st, line);
            let kept = st.ring.last().cloned();
            if let Some(a) = st.open_asks.get_mut(&id) {
                a.kept = kept;
            }
        }
    }

    /// The lines of open asks the trim would drop with ring[..cut], to put
    /// back at its end. One said again is kept when its copy is cut; one
    /// not said again yet is in the turn under way, and is copied when the
    /// cut goes into that turn (inside).
    pub(super) fn asks_cut(&self, st: &mut State, cut: usize, inside: bool) -> Vec<Arc<[u8]>> {
        let mut out = Vec::new();
        for id in Self::open_ids(st) {
            let cut_lines = &st.ring[..cut];
            let Some(a) = st.open_asks.get_mut(&id) else {
                continue;
            };
            match &a.kept {
                Some(k) if cut_lines.iter().any(|l| Arc::ptr_eq(l, k)) => out.push(k.clone()),
                None if inside => {
                    let k: Arc<[u8]> = pack(&a.line).into();
                    a.kept = Some(k.clone());
                    out.push(k);
                }
                _ => {}
            }
        }
        out
    }

    /// Puts kept back at the ring's end.
    pub(super) fn put_back(st: &mut State, kept: Vec<Arc<[u8]>>) {
        for l in kept {
            st.ring_n += line_len(&l);
            st.ring.push(l);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rush_core::adapter::{self, Conn, Input};
    use rush_core::event::{self, Ask, Event, TurnEnd};

    use super::super::tests::{host, with_conn};
    use super::super::{Host, RING_MAX};
    use crate::lines;
    use crate::pack::unpack;
    use crate::testing::Home;
    use crate::wire::Config;

    struct Quiet;

    impl Conn for Quiet {
        fn send(&self, _: Input) -> adapter::Result<()> {
            Ok(())
        }
        fn answer(&self, _: &str, _: &str) -> adapter::Result<()> {
            Ok(())
        }
        fn interrupt(&self) -> adapter::Result<()> {
            Ok(())
        }
        fn set_model(&self, _: &str) -> adapter::Result<()> {
            Ok(())
        }
        fn set_mode(&self, _: &str) -> adapter::Result<()> {
            Ok(())
        }
        fn close(&self) -> adapter::Result<()> {
            Ok(())
        }
    }

    fn question(id: &str) -> Event {
        Event::Question(event::Question {
            id: id.into(),
            call_id: format!("toolu_{id}"),
            asks: vec![Ask {
                text: "Turn on the mods Claude writes in this session?".into(),
                ..Ask::default()
            }],
            ..event::Question::default()
        })
    }

    /// The ring as written, a line each.
    fn ring(h: &Host) -> Vec<String> {
        h.lock()
            .ring
            .iter()
            .map(|l| {
                let mut b = Vec::new();
                unpack(l, &mut b).unwrap();
                String::from_utf8_lossy(&b).into_owned()
            })
            .collect()
    }

    /// Where the ring has the question asked as id, by line.
    fn asked_at(h: &Host, id: &str) -> Vec<usize> {
        let mark = format!(r#""ID":"{id}""#);
        ring(h)
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("\"question\"") && l.contains(&mark))
            .map(|(i, _)| i)
            .collect()
    }

    fn turn_ends(h: &Host) -> Vec<usize> {
        ring(h)
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains(r#""t":"turn_end""#))
            .map(|(i, _)| i)
            .collect()
    }

    fn setup(id: &str) -> (Host, Arc<dyn Conn>) {
        let h = host(Config {
            id: id.into(),
            ..Config::default()
        });
        let conn: Arc<dyn Conn> = Arc::new(Quiet);
        with_conn(&h, conn.clone());
        h.lock().info.state = "working".into();
        (h, conn)
    }

    fn say(h: &Host, conn: &Arc<dyn Conn>, ev: Event) {
        let mut st = h.lock();
        h.on_agent_event(&mut st, conn, ev);
    }

    // An ask still open when its turn ends is said again after the end,
    // each time; one answered isn't.
    #[test]
    fn an_open_ask_is_said_again_after_its_turn() {
        let _home = Home::new();
        let (h, conn) = setup("oa1");
        say(&h, &conn, question("q1"));
        say(&h, &conn, question("q2"));
        assert_eq!(asked_at(&h, "q1").len(), 1);
        h.do_op({
            let mut o = crate::op::Op::named("allow");
            o.id = "q2".into();
            o
        })
        .ok();
        say(&h, &conn, Event::TurnEnd(TurnEnd::default()));
        let end = turn_ends(&h)[0];
        let q1 = asked_at(&h, "q1");
        assert!(
            q1.len() == 2 && q1[1] > end,
            "q1 is said again after the turn's end: {q1:?}, end {end}"
        );
        assert!(
            asked_at(&h, "q2").iter().all(|&i| i < end),
            "q2 was answered: it isn't said again"
        );
        assert_eq!(h.lock().info.state, "blocked", "still waiting on q1");
        say(&h, &conn, Event::TurnEnd(TurnEnd::default()));
        assert_eq!(asked_at(&h, "q1").len(), 3, "and again after the next");
    }

    // Trimming old turns away keeps the latest line of an ask still open,
    // and lets an answered one go.
    #[test]
    fn the_ring_keeps_an_open_ask_its_turn_was_trimmed_from() {
        let _home = Home::new();
        let (h, conn) = setup("oa2");
        say(&h, &conn, question("q1"));
        say(&h, &conn, question("q2"));
        say(&h, &conn, Event::TurnEnd(TurnEnd::default()));
        h.do_op({
            let mut o = crate::op::Op::named("allow");
            o.id = "q2".into();
            o
        })
        .ok();
        let big = format!(
            r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#,
            "x".repeat(1 << 20)
        );
        {
            let mut st = h.lock();
            for turn in 0..12 {
                h.record(
                    &mut st,
                    lines::echo_line(&format!("turn {turn}"), &[], None),
                );
                for _ in 0..3 {
                    h.record(&mut st, big.clone().into_bytes());
                }
            }
            assert!(
                st.ring_n <= RING_MAX + (64 << 10),
                "ring holds {}",
                st.ring_n
            );
        }
        assert!(
            turn_ends(&h).is_empty(),
            "the turn that asked was trimmed away"
        );
        assert_eq!(asked_at(&h, "q1").len(), 1, "the open ask is kept");
        assert!(asked_at(&h, "q2").is_empty(), "the answered one goes");
    }

    // A turn bigger than the ring, cut inside, keeps its open ask too.
    #[test]
    fn a_cut_inside_the_turn_keeps_its_open_ask() {
        let _home = Home::new();
        let (h, conn) = setup("oa3");
        {
            let mut st = h.lock();
            h.record(&mut st, lines::echo_line("go", &[], None));
        }
        say(&h, &conn, question("q1"));
        let big = format!(
            r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#,
            "x".repeat(1 << 20)
        );
        {
            let mut st = h.lock();
            for _ in 0..20 {
                h.record(&mut st, big.clone().into_bytes());
            }
        }
        assert!(
            !asked_at(&h, "q1").is_empty(),
            "the ask is kept though the turn's start was cut"
        );
    }
}
