//! Which of the conversation's views (Preset) fits right now: the counts
//! rush reads off a session's last finished turn (Digest), simple rules
//! that pick a view from them with a short reason, and, when the rules
//! can't tell, the question put to a quick model: the counts only, never
//! a word of the conversation. Read from the generic model (Kind, Status,
//! exit codes, the checks read from shell commands), whichever agent ran
//! the session. New in the Rust port: Go rush had no such views.

use std::collections::HashSet;

use rush_core::time::Ms;
use rush_core::tool::Kind;

use crate::model::{Item, Session, Status, Turn};
use crate::render::{Preset, check_names};

/// Away this long, a look at the session is coming back to it.
pub const AWAY: Ms = 10 * 60_000;
/// A turn this long or longer is a long one: where its time went is worth
/// a look.
pub const LONG_TURN: Ms = 2 * 60_000;
/// A turn with this many steps or fewer is mostly talk.
const FEW_STEPS: usize = 2;

/// When a view is suggested: you came back to the session (opened it, or
/// looked again after being away), or a turn ended while it was open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Moment {
    Back,
    Ended,
}

impl Moment {
    /// Its name where what you took and turned down is kept.
    pub fn name(self) -> &'static str {
        match self {
            Moment::Back => "back",
            Moment::Ended => "ended",
        }
    }
}

/// What rush counts of a session's last finished turn, and of the time
/// since you last looked: numbers and check kinds, never its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Digest {
    /// Edits that went through (edits, writes, deletes, moves), and the
    /// files they touched.
    pub edits: usize,
    pub files: usize,
    /// Tool calls, subagents' included.
    pub steps: usize,
    /// The checks it ran (tests, build, clippy, lint…), each with whether
    /// its last run passed.
    pub checks: Vec<(&'static str, bool)>,
    /// How many words the agent said.
    pub words: usize,
    /// The turn's time, and how it split: thinking (the rest), in tools
    /// (their spans, overlaps once), waiting on you.
    pub turn: Ms,
    pub thinking: Ms,
    pub tools: Ms,
    pub waiting: Ms,
    /// How long since you last looked (0: not since rush started), and the
    /// turns that finished since.
    pub away: Ms,
    pub unseen: usize,
}

/// The view the rules pick, and why in a few words; None when they can't
/// tell.
pub type Ruled = Option<(Preset, String)>;

/// The digest of session s's last finished turn, at now, last having
/// looked at it at seen (0 when you haven't since rush started). None
/// before any turn has finished.
pub fn digest(s: &Session, seen: Ms, now: Ms) -> Option<Digest> {
    let t = s.turns.iter().rev().find(|t| !t.live)?;
    let mut d = Digest {
        away: if seen > 0 { (now - seen).max(0) } else { 0 },
        unseen: if seen > 0 {
            s.turns
                .iter()
                .rev()
                .take_while(|t| t.live || t.end > seen)
                .filter(|t| !t.live)
                .count()
        } else {
            0
        },
        ..Digest::default()
    };
    let mut files = HashSet::new();
    let mut spans: Vec<(Ms, Ms)> = Vec::new();
    for &id in &t.steps {
        let st = s.step(id);
        if st.kind == Kind::Todo {
            continue;
        }
        d.steps += 1;
        if st.status == Status::Ok
            && matches!(
                st.kind,
                Kind::Edit | Kind::Write | Kind::Notebook | Kind::Delete | Kind::Move
            )
        {
            d.edits += 1;
            let p = &st.call().input.path;
            if !p.is_empty() {
                files.insert(p.clone());
            }
        }
        let end = if st.end != 0 { st.end } else { st.start };
        if st.start == 0 || end <= st.start {
            continue;
        }
        if st.status == Status::Waiting || st.kind == Kind::Question {
            d.waiting += end - st.start;
        } else {
            spans.push((st.start, end));
        }
    }
    d.files = files.len();
    d.tools = merged(spans);
    d.checks = checks(s, t);
    d.words = t
        .items
        .iter()
        .map(|it| match it {
            Item::Text { text, .. } => text.split_whitespace().count(),
            _ => 0,
        })
        .sum();
    d.turn = (t.end - t.start).max(0);
    d.thinking = (d.turn - d.tools - d.waiting).max(0);
    Some(d)
}

/// The time spans cover, overlaps counted once.
fn merged(mut spans: Vec<(Ms, Ms)>) -> Ms {
    spans.sort_unstable();
    let mut total = 0;
    let mut cur: Option<(Ms, Ms)> = None;
    for (a, b) in spans {
        match cur {
            Some((ca, cb)) if a <= cb => cur = Some((ca, cb.max(b))),
            Some((ca, cb)) => {
                total += cb - ca;
                cur = Some((a, b));
            }
            None => cur = Some((a, b)),
        }
    }
    total + cur.map_or(0, |(a, b)| b - a)
}

/// The checks the main agent's shell steps in t ran, in order, each with
/// whether its last run passed (as the Code view reads them).
fn checks(s: &Session, t: &Turn) -> Vec<(&'static str, bool)> {
    let mut out: Vec<(&'static str, bool)> = Vec::new();
    for it in &t.items {
        let Item::Step(id) = it else { continue };
        let st = s.step(*id);
        if st.kind != Kind::Shell || matches!(st.status, Status::Running | Status::Waiting) {
            continue;
        }
        let passed = !(st.status == Status::Failed || st.exit > 0);
        for n in check_names(&st.call().input.command) {
            match out.iter_mut().find(|c| c.0 == n) {
                Some(c) => c.1 = passed,
                None => out.push((n, passed)),
            }
        }
    }
    out
}

/// "1 file", "4 files".
fn count(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// The view the rules pick for d at moment, with a short reason; None when
/// they can't tell, and a quick model may be asked.
pub fn rules(d: &Digest, moment: Moment) -> Ruled {
    if moment == Moment::Back && d.unseen >= 1 {
        let n = count(d.unseen, "turn");
        return Some((Preset::CatchUp, format!("{n} finished while you were away")));
    }
    if d.files > 0 && !d.checks.is_empty() {
        let names: Vec<&str> = d.checks.iter().map(|c| c.0).collect();
        let mut why = format!(
            "changed {}, ran {}",
            count(d.files, "file"),
            names.join(", ")
        );
        let failed: Vec<&str> = d.checks.iter().filter(|c| !c.1).map(|c| c.0).collect();
        if !failed.is_empty() {
            why.push_str(&format!(" ({} failed)", failed.join(", ")));
        }
        return Some((Preset::Code, why));
    }
    if d.turn >= LONG_TURN && d.tools * 2 > d.turn {
        return Some((
            Preset::Trail,
            format!(
                "took {} min, {} of them in tools",
                minutes(d.turn),
                minutes(d.tools)
            ),
        ));
    }
    if d.steps <= FEW_STEPS && d.files == 0 && d.words > 0 {
        let steps = if d.steps == 0 {
            "no steps".to_string()
        } else {
            count(d.steps, "step")
        };
        return Some((Preset::Chat, format!("mostly talk, {steps}")));
    }
    if d.files == 0 && d.checks.is_empty() && d.turn < LONG_TURN {
        return Some((
            Preset::Normal,
            format!("{}, nothing changed", count(d.steps, "step")),
        ));
    }
    None
}

/// ms as whole minutes, at least 1.
fn minutes(ms: Ms) -> i64 {
    ((ms + 30_000) / 60_000).max(1)
}

/// What the quick model is told it does.
pub const SYSTEM: &str = "You pick which view of a coding agent's conversation fits best right now, from counts about its last turn. Views: normal (everything, as usual), chat (what did we say? just the words), code (what did it change? diffs and checks), catch-up (what happened while I was away? one summary card), trail (where did the time go? steps on a time axis). Answer with the view's name and a reason of at most eight plain words.";

/// The shape of its answer.
pub const SCHEMA: &str = r#"{"type":"object","properties":{"view":{"type":"string","enum":["normal","chat","code","catch-up","trail"]},"reason":{"type":"string"}},"required":["view","reason"]}"#;

/// The question put to the quick model: d's counts, the moment, the view
/// in use and the last views chosen. Numbers and view names only.
pub fn prompt(d: &Digest, moment: Moment, using: Preset, recent: &[Preset]) -> String {
    let checks: Vec<serde_json::Value> = d
        .checks
        .iter()
        .map(|(k, ok)| serde_json::json!({"kind": k, "passed": ok}))
        .collect();
    let recent: Vec<&str> = recent.iter().map(|p| p.name()).collect();
    let v = serde_json::json!({
        "moment": match moment {
            Moment::Back => "the user came back to the session",
            Moment::Ended => "a turn just ended while the user watched",
        },
        "lastTurn": {
            "edits": d.edits,
            "filesChanged": d.files,
            "steps": d.steps,
            "checks": checks,
            "wordsSaid": d.words,
            "seconds": d.turn / 1000,
            "thinkingSeconds": d.thinking / 1000,
            "toolSeconds": d.tools / 1000,
            "waitingOnUserSeconds": d.waiting / 1000,
        },
        "minutesAway": d.away / 60_000,
        "unseenTurns": d.unseen,
        "viewInUse": using.name(),
        "recentViews": recent,
    });
    format!("Which view fits now?\n{v}")
}

/// The view and reason in the quick model's answer, its reason one line of
/// at most 60 characters; None when it can't be read.
pub fn answer(b: &[u8]) -> Option<(Preset, String)> {
    let v: serde_json::Value = serde_json::from_slice(b).ok()?;
    let name = v.get("view")?.as_str()?.trim().to_lowercase();
    let p = Preset::ALL
        .into_iter()
        .find(|p| p.name() == name || p.title().to_lowercase() == name)?;
    let reason: String = v
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let reason = match reason.char_indices().nth(60) {
        Some((at, _)) => format!("{}…", &reason[..at]),
        None => reason,
    };
    Some((p, reason))
}
