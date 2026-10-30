//! Zen shows nothing until an agent needs you, then only that agent: what
//! it last said and what it needs, answered in its dock. Answer it and it
//! stays, to carry on with; mark it done and the next one takes its
//! place. No header or strip: one bar with where you are in the queue and
//! the keys to skip (ctrl+n) or leave (ctrl+z); hold tab to peek at
//! what's working. Port of Go's zen.go (zenQueue :22-35 as of 779c3ea7:
//! waiting agents too, and none whose open failed lately; zenHeld :39,
//! zenPick :51, zenSkip :67, zenQuiet :81, zenBar :107, zenBody :146,
//! zenPeek :180-232, zenPeekLines :236), model.go:1900 (setZen),
//! model.go:2258 (zen's list: only what waits on you), rushmode.go:2149
//! (openFailed) and keys.go:110-131 (ctrl+z, the peek, keys held while
//! nothing's shown).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_fleet::Predicates;

use super::ink::spread;
use crate::frame::{Out, PlaceId, Press};
use crate::model::Model;
use crate::style::*;

/// A look at what's working, for as long as tab is held. Terminals don't
/// say when a key is let go, so a hold is known by its repeats: once they
/// stop coming, tab is up. A tap with no repeats keeps the peek until the
/// next key.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Peek {
    pub on: bool,
    /// Repeats have come: it ends when they stop.
    pub held: bool,
    /// The last tab.
    pub at: Option<Instant>,
    /// Between the last two repeats.
    pub gap: Duration,
}

impl Peek {
    /// How long after the last repeat tab counts as let go.
    pub fn let_go(&self) -> Duration {
        if self.gap.is_zero() {
            return Duration::from_millis(300);
        }
        (self.gap * 3).clamp(Duration::from_millis(150), Duration::from_millis(600))
    }

    fn since(&self) -> Duration {
        self.at.map_or(Duration::MAX, |t| t.elapsed())
    }
}

/// Zen's state.
#[derive(Clone, Debug, Default)]
pub struct Zen {
    pub on: bool,
    /// The agent zen put on screen (zenAt).
    pub at: String,
    pub peek: Peek,
    /// When each agent's Session last failed to open: zen moves past it
    /// for a while (openFailed).
    pub open_failed: HashMap<String, Instant>,
}

/// How long zen moves past an agent whose Session couldn't be opened.
const OPEN_FAILED_FOR: Duration = Duration::from_secs(30);

/// Notes that key's Session couldn't be opened, so zen moves past it for
/// a while (rushmode.go:2149).
pub fn open_failed(m: &mut Model, key: &str) {
    m.kept::<Zen>()
        .open_failed
        .insert(key.to_string(), Instant::now());
}

/// Whether the list shows a: in zen, only the agents waiting on you, a
/// question, a wait, or a finished turn that wants something of you
/// (model.go:2258), and the one zen put on screen, answered, until it's
/// marked done: else a reading would take it off from under you.
pub(super) fn shows(m: &Model, a: &Agent) -> bool {
    let Some(z) = m.zen() else {
        return true;
    };
    (a.key == z.at && !a.done)
        || a.needs_you()
        || a.waiting()
        || crate::screens::session_core::turnwants::wants_you(m, a)
}

impl Model {
    pub(crate) fn zen(&self) -> Option<&Zen> {
        self.kept.peek::<Zen>().filter(|z| z.on)
    }

    /// Every agent waiting on you, oldest first (zenQueue).
    pub(crate) fn zen_queue(&self) -> Vec<usize> {
        let failed = self.kept.peek::<Zen>().map(|z| &z.open_failed);
        let mut out: Vec<usize> = (0..self.agents.len())
            .filter(|&i| {
                let a = &self.agents[i];
                // Couldn't be shown: tried again in a bit.
                let lately = failed
                    .and_then(|f| f.get(&a.key))
                    .is_some_and(|t| t.elapsed() < OPEN_FAILED_FOR);
                !lately && (a.needs_you() || a.waiting())
            })
            .collect();
        out.sort_by_key(|&i| self.agents[i].job.updated_at);
        out
    }

    /// The agent zen has on screen: the one it put there, answered or
    /// not, until it's gone (zenHeld).
    pub(crate) fn zen_held(&self) -> Option<usize> {
        let z = self.zen()?;
        self.agents
            .iter()
            .position(|a| a.key == self.sel && a.key == z.at)
    }

    /// Zen on or off (setZen).
    pub(crate) fn set_zen(&mut self, on: bool) {
        if on && self.place != PlaceId::Agents {
            self.set_place(PlaceId::Agents);
        }
        let z = self.kept::<Zen>();
        z.on = on;
        z.peek = Peek::default();
        if !on {
            return;
        }
        if let Some(&i) = self.zen_queue().first() {
            self.zen_show(i);
        }
    }

    /// Puts agent i on zen's screen, its conversation open.
    fn zen_show(&mut self, i: usize) {
        let key = self.agents[i].key.clone();
        self.kept::<Zen>().at = key.clone();
        self.sel = key.clone();
        if self.session.as_ref().is_none_or(|v| v.key != key) {
            self.open(i);
        }
    }

    /// Keeps the selection on the agent zen showed you, answered or not;
    /// once it's gone, the oldest one waiting takes its place (zenPick).
    pub(crate) fn zen_pick(&mut self) {
        if self.zen().is_none() || self.zen_held().is_some() {
            return;
        }
        if let Some(&i) = self.zen_queue().first() {
            self.zen_show(i);
        }
    }

    /// The next agent waiting on you: from an answered one, the oldest
    /// (zenSkip).
    pub(crate) fn zen_skip(&mut self) {
        let q = self.zen_queue();
        let mut next = 0;
        for (n, &i) in q.iter().enumerate() {
            if self.agents[i].key == self.sel {
                next = (n + 1) % q.len();
            }
        }
        if let Some(&i) = q.get(next)
            && self.agents[i].key != self.sel
        {
            self.zen_show(i);
        }
    }

    /// The screen when nothing needs you (zenQuiet).
    pub(crate) fn zen_quiet(&self, w: usize, h: usize) -> Vec<Line> {
        let t = self.tally();
        let mut out = vec![Line::new(); h / 3];
        let center = |l: Line| {
            let pad = w.saturating_sub(l.width()) / 2;
            let mut r = line([plain(blanks(pad))]);
            join(&mut r, l);
            r
        };
        out.push(center(line([sp("✓ nothing needs you", GREEN)])));
        out.push(Line::new());
        let mut counts = Line::new();
        let mut add = |l: Line| {
            if counts.width() > 0 {
                counts.spans.push(sp(" · ", DIM));
            }
            join(&mut counts, l);
        };
        if t.working > 0 {
            add(line([sp(format!("✻ {} working", t.working), ORANGE)]));
        }
        if t.busy > 0 {
            add(line([sp(format!("◌ {} in background", t.busy), DIM)]));
        }
        add(line([sp(format!("{} finished", t.done), DIM)]));
        out.push(center(counts));
        out.push(Line::new());
        out.push(Line::new());
        out.push(center(line([
            sp("◇ zen", YELLOW),
            sp(" shows an agent here when one needs you", DIM),
        ])));
        out.push(Line::new());
        out.push(center(keys(&[
            ("hold tab", "what's working"),
            ("ctrl+z", "leave zen"),
        ])));
        out
    }

    /// The row pinned above zen's agent: that it's zen, which of the
    /// waiting agents this is and where, and the keys to move on or
    /// leave, so a long reply never scrolls them away (zenBar).
    pub(crate) fn zen_bar(&self, a: &Agent, w: usize) -> Line {
        let q = self.zen_queue();
        let pos = q
            .iter()
            .position(|&i| self.agents[i].key == a.key)
            .map_or(0, |p| p + 1);
        let mut left = line([spb("◇ zen", YELLOW), plain("   ")]);
        if pos == 0 {
            // Answered, or never waiting: say so rather than "needs you".
            join(
                &mut left,
                line([
                    sp("✓ answered", GREEN),
                    plain("  "),
                    sp(format!("{} waiting", q.len()), DIM),
                ]),
            );
        } else {
            join(
                &mut left,
                line([
                    spb("● needs you", YELLOW),
                    plain("  "),
                    sp(format!("{pos} of {}", q.len()), SUB),
                ]),
            );
            if a.job.updated_at != 0 {
                let waited = (self.now - a.job.updated_at).max(0) / 1000 * 1000;
                left.spans
                    .push(sp(format!(" · waiting {}", dur(waited)), DIM));
            }
        }
        let mut where_ = tildify(&a.job.cwd);
        if !a.branch.is_empty() {
            where_ = format!("{where_} · {}", a.branch);
        }
        let mut next = Line::new();
        if q.len() > 1 || pos == 0 && !q.is_empty() {
            next = line([
                spb("[ctrl+n]", YELLOW),
                plain(" "),
                sp("next", YELLOW),
                sp("  ·  ", FAINT),
            ]);
        }
        let pairs = [("hold tab", "what's working"), ("ctrl+z", "leave zen")];
        let room = (w as isize - left.width() as isize - next.width() as isize - 4).max(20);
        let mut right = next;
        join(&mut right, keys_fit(room as usize, &pairs));
        let room = w as isize - left.width() as isize - right.width() as isize - 6;
        if room > 12 {
            let n = (room as usize).min(rush_core::text::width(&where_));
            left.spans.push(plain("   "));
            left.spans.push(sp(fit_str(&where_, n), DIM));
        }
        spread(left, right, w)
    }

    /// What it said and what it needs, when there's no conversation to
    /// show (zenBody, for an agent rush can't read).
    fn zen_body(&self, a: &Agent, w: usize) -> Vec<Line> {
        let said = if a.job.needs.is_empty() {
            a.job.detail.clone()
        } else {
            a.job.needs.clone()
        };
        let mut out = vec![Line::new(), line([plain("    "), sp("it said", DIM)])];
        for l in wrap(&said, w.saturating_sub(8).min(100)) {
            out.push(line([plain("    "), sp(l, TEXT)]));
        }
        if !a.job.needs.is_empty() {
            out.push(Line::new());
            out.push(line([
                plain("    "),
                sp("it needs  ", DIM),
                sp(one_line(&a.job.needs), YELLOW),
            ]));
        }
        out
    }

    /// What's working, row by row as in Agents, the busiest first
    /// (zenPeekLines).
    pub(crate) fn zen_peek_lines(&self, w: usize, h: usize) -> Vec<Line> {
        let mut busy: Vec<usize> = (0..self.agents.len())
            .filter(|&i| {
                let a = &self.agents[i];
                !a.needs_you() && (a.job.live() || a.busy())
            })
            .collect();
        busy.sort_by(|&i, &j| {
            let (a, b) = (&self.agents[i], &self.agents[j]);
            b.job
                .live()
                .cmp(&a.job.live())
                .then(b.job.updated_at.cmp(&a.job.updated_at))
        });
        let held = self.kept.peek::<Zen>().is_some_and(|z| z.peek.held);
        let back = if held {
            line([sp("let go of tab to go back", FAINT)])
        } else {
            keys(&[("tab", "again to go back")])
        };
        let mut out = vec![
            spread(
                line([
                    spb("◇ zen", YELLOW),
                    plain("   "),
                    spb("what's working", TEXT),
                ]),
                back,
                w,
            ),
            Line::new(),
        ];
        if busy.is_empty() {
            out.push(Line::new());
            out.push(line([
                plain("   "),
                sp("nothing is working right now", DIM),
            ]));
            return out;
        }
        let name_col = self.name_column(w);
        for i in busy {
            if out.len() >= h {
                break;
            }
            out.push(self.agent_line(i, w, w, false, name_col, false, None));
        }
        out
    }

    /// Takes the keys while peeking. Repeats of tab keep it open; any
    /// other key, or tab pressed afresh, closes it, and goes no further
    /// (peekKey).
    fn peek_key(&mut self, s: &str) -> Out {
        let p = &mut self.kept::<Zen>().peek;
        let since = p.since();
        // The first repeat comes after the keyboard's repeat delay; the
        // rest much quicker.
        let repeat = s == "tab"
            && (p.held && since < p.let_go() || !p.held && since < Duration::from_secs(1));
        if !repeat {
            *p = Peek::default();
            return Out::Draw;
        }
        if p.held {
            p.gap = since;
        }
        p.held = true;
        p.at = Some(Instant::now());
        let at = Instant::now() + p.let_go();
        self.ask_wake(at);
        Out::Idle
    }

    /// Ends a held peek once the repeats have stopped (peekCheck); while
    /// held, the next look is asked for.
    pub(crate) fn peek_check(&mut self) {
        let p = self.kept::<Zen>().peek;
        if !p.on || !p.held {
            return;
        }
        if p.since() >= p.let_go() {
            self.kept::<Zen>().peek = Peek::default();
            return;
        }
        let at = p.at.unwrap_or_else(Instant::now) + p.let_go();
        self.ask_wake(at);
    }
}

/// Zen's keys, before anything else has them (keys.go:110-131).
pub(super) fn key(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    if m.confirm.is_some() || m.sheet.is_some() {
        return Out::Pass;
    }
    if s == "ctrl+z" {
        let on = m.zen().is_none();
        m.set_zen(on);
        return Out::Draw;
    }
    if m.zen().is_none() {
        return Out::Pass;
    }
    // In zen, tab held down peeks at what's working; letting go (or,
    // tapped, any key) comes back.
    if m.kept::<Zen>().peek.on {
        return m.peek_key(s);
    }
    if s == "tab" {
        m.kept::<Zen>().peek = Peek {
            on: true,
            at: Some(Instant::now()),
            ..Peek::default()
        };
        return Out::Draw;
    }
    if s == "ctrl+n" {
        m.zen_skip();
        return Out::Draw;
    }
    // While nothing is shown, or the agent held hasn't connected yet (its
    // Session not read: `m.host == nil`), keys wait rather than land in
    // some box you can't see (keys.go:126).
    let connected = m.zen_held().is_some_and(|i| {
        let key = &m.agents[i].key;
        m.session.as_ref().is_some_and(|v| &v.key == key && v.read)
    });
    if !connected {
        return match s {
            "ctrl+q" | "ctrl+c" | "?" => Out::Pass,
            _ => Out::Idle,
        };
    }
    Out::Pass
}

/// Zen is the whole screen: the peek, the quiet, or the agent under its
/// bar (view.go:1211-1229).
pub(super) fn over(m: &mut Model, out: &mut Vec<Line>) {
    if m.zen().is_none() {
        return;
    }
    m.peek_check();
    // The pane, two cells in from the edge and three narrower, as Go's
    // pane is with the list put away.
    let (w, h) = (m.w, m.h);
    let pw = w.saturating_sub(3);
    let mut rows = if m.kept::<Zen>().peek.on {
        m.zen_peek_lines(pw, h)
    } else if let Some(i) = m.zen_held() {
        let a = m.agents[i].clone();
        let mut rows = vec![m.zen_bar(&a, pw)];
        let (now, tick) = (m.now, m.tick);
        match m.session.as_mut().filter(|v| v.key == a.key) {
            Some(v) if v.read => {
                v.top = 1;
                v.sync(pw, now, tick);
                rows.extend(v.body(h.saturating_sub(1)));
            }
            _ => rows.extend(m.zen_body(&a, pw)),
        }
        rows
    } else {
        m.zen_quiet(pw, h)
    };
    rows.truncate(h);
    rows.resize(h, Line::new());
    *out = rows
        .into_iter()
        .map(|l| {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            fit(r, w)
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::agent::State;

    fn agent(key: &str, state: State, needs: &str, updated: i64) -> Agent {
        Agent {
            key: key.into(),
            display_name: key.into(),
            pid: 42,
            job: rush_core::agent::Job {
                id: key.into(),
                state,
                needs: needs.into(),
                detail: "halfway through typing".into(),
                updated_at: updated,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn model() -> Model {
        let mut m = super::super::feed::tests::model(160, 40);
        let now = m.clock.unwrap();
        set(
            &mut m,
            vec![
                agent("a", State::Blocked, "which way?", now - 60_000),
                agent("b", State::Working, "", now - 30_000),
                agent("c", State::Done, "", now - 10_000),
            ],
        );
        m
    }

    fn set(m: &mut Model, agents: Vec<Agent>) {
        let n = agents.len();
        m.set_fleet(crate::worker::Fleet {
            agents,
            roots: vec![String::new(); n],
            ..Default::default()
        });
    }

    fn frame(m: &mut Model) -> String {
        m.frame().iter().map(|l| l.plain() + "\n").collect()
    }

    // Zen is only the agent that needs you: no header, only the keys to
    // leave, and the screen still exactly filled.
    #[test]
    fn hides_the_ui() {
        let mut m = model();
        m.press("ctrl+z");
        let f = frame(&mut m);
        assert_eq!(f.lines().count(), m.h, "zen fills the screen exactly");
        for ui in ["Settings", "back to the list"] {
            assert!(!f.contains(ui), "zen hides {ui:?}:\n{f}");
        }
        assert!(f.contains("ctrl+z leave zen"), "{f}");
        assert!(f.contains("needs you") && f.contains("1 of 1"), "{f}");

        // Answered, it stays until it's gone.
        let mut agents = m.agents.clone();
        agents[0].job.state = State::Done;
        set(&mut m, agents.clone());
        let f = frame(&mut m);
        assert!(f.contains("answered"), "{f}");
        agents.remove(0);
        set(&mut m, agents);
        let f = frame(&mut m);
        assert!(
            f.contains("nothing needs you") && f.contains("ctrl+z leave zen"),
            "{f}"
        );
        assert!(f.contains("✻ 1 working") && f.contains("1 finished"), "{f}");
        m.press("ctrl+z");
        assert!(m.zen().is_none());
    }

    // In zen, tab held peeks at what's working and letting go comes back;
    // a tap peeks until the next key, which goes no further.
    #[test]
    fn peeks() {
        let mut m = model();
        m.press("ctrl+z");
        m.press("tab");
        let f = frame(&mut m);
        assert!(m.kept::<Zen>().peek.on, "tab peeks");
        assert!(
            f.contains("what's working") && f.contains("tab again"),
            "{f}"
        );
        m.press("tab"); // the keyboard's repeat: still held
        let p = m.kept::<Zen>().peek;
        assert!(p.on && p.held, "a repeat keeps it open, held");
        m.kept::<Zen>().peek.at = Some(Instant::now() - Duration::from_secs(1));
        m.peek_check();
        assert!(!m.kept::<Zen>().peek.on, "letting go ends the peek");

        m.press("tab");
        m.kept::<Zen>().peek.at = Some(Instant::now() - Duration::from_secs(2));
        let before = m.sel.clone();
        m.press("x");
        assert!(
            !m.kept::<Zen>().peek.on && m.sel == before,
            "a key after a tap only ends it"
        );
    }

    // With nothing waiting, zen at 120×30 is Go's `rush --render 120x30
    // ctrl+z`: a third of the way down, each line centred in the pane two
    // cells in.
    #[test]
    fn quiet_frame_at_go_s_width() {
        let mut m = model();
        m.w = 120;
        m.h = 30;
        let now = m.clock.unwrap();
        set(&mut m, vec![agent("b", State::Working, "", now - 30_000)]);
        m.press("ctrl+z");
        let f = m.frame();
        assert_eq!(f.len(), 30);
        let lead = |l: &Line| {
            let p = l.plain();
            p.chars().take_while(|c| *c == ' ').count()
        };
        let centred = |s: &str| 2 + (117 - rush_core::text::width(s)) / 2;
        assert_eq!(f[10].plain().trim(), "✓ nothing needs you");
        assert_eq!(lead(&f[10]), centred("✓ nothing needs you"));
        assert_eq!(lead(&f[10]), 51, "where Go puts it");
        assert_eq!(f[12].plain().trim(), "✻ 1 working · 0 finished");
        assert_eq!(lead(&f[15]), 38);
        assert_eq!(
            f[17].plain().trim(),
            "hold tab what's working  ·  ctrl+z leave zen"
        );
        assert!(f.iter().all(|l| l.width() == 120));
    }

    #[test]
    fn skips_to_the_next() {
        let mut m = model();
        let now = m.clock.unwrap();
        let mut agents = m.agents.clone();
        agents.push(agent("d", State::Blocked, "ok?", now - 5_000));
        set(&mut m, agents);
        m.press("ctrl+z");
        assert_eq!(m.sel, "a", "the oldest waiting first");
        m.press("ctrl+n");
        assert_eq!(m.sel, "d");
        let f = frame(&mut m);
        assert!(f.contains("2 of 2") && f.contains("[ctrl+n] next"), "{f}");
        m.press("ctrl+n");
        assert_eq!(m.sel, "a", "round again");
    }

    // zen.go:23-35: a waiting agent (seen, still blocked) is in the
    // queue too, oldest first; one whose Session failed to open lately
    // is passed over for a while.
    #[test]
    fn queue_takes_waiting_and_skips_failed_opens() {
        let mut m = model();
        let now = m.clock.unwrap();
        let mut agents = m.agents.clone();
        let mut w = agent("w", State::Blocked, "", now - 120_000);
        w.seen = true;
        agents.push(w);
        set(&mut m, agents);
        let keys = |m: &Model| -> Vec<String> {
            m.zen_queue()
                .into_iter()
                .map(|i| m.agents[i].key.clone())
                .collect()
        };
        assert_eq!(keys(&m), ["w", "a"], "waiting too, oldest first");
        open_failed(&mut m, "w");
        assert_eq!(keys(&m), ["a"], "a failed open is passed over");
        m.kept::<Zen>()
            .open_failed
            .insert("w".into(), Instant::now() - Duration::from_secs(31));
        assert_eq!(keys(&m), ["w", "a"], "and tried again after a while");
    }

    // model.go:2258: in zen the list keeps only what waits on you; out of
    // it, everything.
    #[test]
    fn list_in_zen_is_only_what_waits() {
        let mut m = model();
        let shown = |m: &Model| -> Vec<String> {
            m.agents
                .iter()
                .filter(|a| shows(m, a))
                .map(|a| a.key.clone())
                .collect()
        };
        assert_eq!(shown(&m), ["a", "b", "c"]);
        m.press("ctrl+z");
        assert_eq!(shown(&m), ["a"]);
    }

    // keys.go:126: held, but its Session not connected yet, keys wait
    // rather than land in a box you can't see; ctrl+n still moves on.
    #[test]
    fn keys_wait_while_the_held_agent_connects() {
        let mut m = model();
        m.press("ctrl+z");
        let held = m.zen_held().map(|i| m.agents[i].key.clone());
        let held = held.expect("an agent held");
        let press = |m: &mut Model, s: &str| key(m, &Press::of(s).unwrap());
        m.session = Some(crate::session::SessionView::new(
            held,
            crate::session::Source::Hosted,
            0,
        ));
        assert!(matches!(press(&mut m, "x"), Out::Idle), "connecting");
        assert!(matches!(press(&mut m, "ctrl+c"), Out::Pass));
        if let Some(v) = m.session.as_mut() {
            v.read = true;
        }
        assert!(matches!(press(&mut m, "x"), Out::Pass), "connected");
    }
}
