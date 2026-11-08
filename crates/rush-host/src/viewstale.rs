//! When an agent's view stops being live: greyed, the agent told, then
//! put away. Every limit is here, and the rule is plain functions over a
//! view and what its turn did, so it is tested by a table. Rust-only: Go
//! greyed by turns alone in its UI (internal/ui/viewquiet.go) and put a
//! view away after three turns untouched (internal/host/views.go
//! PutAwayStale); the host now decides, writes why into the view's file
//! (View::greyed) and logs each grey and put-away for `rush why`.

use std::collections::HashSet;

use crate::views::{Block, Item, View};

/// A view the agent hasn't touched for this long greys (and the agent is
/// told once, while it works): long enough that a benchmark or a build
/// between two updates doesn't, short enough that a long turn can't keep a
/// dead view looking live.
pub const STALE_MS: i64 = 10 * 60_000;
/// One untouched this long is put away, working or not: the user can
/// bring it back from the past views.
pub const AWAY_MS: i64 = 45 * 60_000;
/// Turns a greyed view may stay untouched before it is put away.
pub const AWAY_TURNS: u32 = 2;

/// Why a view greyed, as its foot says it.
pub const WHY_DONE: &str = "all done";
pub const WHY_TURN: &str = "not updated for a turn";
pub const WHY_BEHIND: &str = "out of date";
pub const WHY_CLOCK: &str = "not updated";

/// What an ended turn did that a view might show: when it started and
/// the changes it made (a push, a merge, a landing), each in Unix ms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub start: i64,
    pub changes: Vec<Change>,
}

/// A change a turn made: when, and its words (what it did and to what:
/// "push", "rush/views").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub at: i64,
    pub words: Vec<String>,
}

/// What becomes of a view.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Live: as it is.
    Keep,
    /// Greys now, for this reason.
    Grey(&'static str),
    /// Already grey and untouched for this many turns since.
    Greyed(u32),
    /// Put away among the past views.
    Away,
}

/// What becomes of v at the end of turn t. A plugin's view and one you
/// brought back are theirs and yours. A greyed one untouched for
/// AWAY_TURNS more turns is put away; else it greys when everything in it
/// is done, when the whole turn left it alone, or when the turn changed
/// what it shows after it was last set.
pub fn at_turn_end(v: &View, t: &Turn) -> Verdict {
    if !v.by.is_empty() || v.pinned {
        return Verdict::Keep;
    }
    let up = v.updated.to_ms();
    let touched = up >= t.start;
    if !v.greyed.is_empty() && !touched {
        let n = v.grey_turns + 1;
        return if n >= AWAY_TURNS {
            Verdict::Away
        } else {
            Verdict::Greyed(n)
        };
    }
    if finished(v) {
        return Verdict::Grey(WHY_DONE);
    }
    if !touched {
        return Verdict::Grey(WHY_TURN);
    }
    if t.changes.iter().any(|c| c.at > up && shows(v, &c.words)) {
        return Verdict::Grey(WHY_BEHIND);
    }
    Verdict::Keep
}

/// What becomes of v at now by the clock alone: put away after AWAY_MS
/// untouched, greyed after STALE_MS.
pub fn by_clock(v: &View, now: i64) -> Verdict {
    if !v.by.is_empty() || v.pinned {
        return Verdict::Keep;
    }
    let age = now - v.updated.to_ms();
    if age >= AWAY_MS {
        Verdict::Away
    } else if age >= STALE_MS && v.greyed.is_empty() {
        Verdict::Grey(WHY_CLOCK)
    } else {
        Verdict::Keep
    }
}

/// The line the agent is told of its stale views, at the end of its next
/// message: the same words every time but the names, as every word is a
/// token. It says it's rush's in plain words, never in a harness's own
/// tags; the conversation leaves it out of your message
/// (rush_convo::transcript::STALE_VIEWS_LEAD).
pub fn note(names: &[String]) -> String {
    format!(
        "{}{}. Set each again with what's current, or clear it.",
        rush_convo::transcript::STALE_VIEWS_LEAD.trim_start(),
        names.join(", ")
    )
}

/// Whether everything v says has a state, and all of it is done: rows
/// and items good or ticked, bars full, nothing busy, failing or waiting.
/// A view of plain facts (no states) isn't finished work.
pub fn finished(v: &View) -> bool {
    let mut n = Marks::default();
    n.tone(&v.tone);
    if let Some(p) = &v.progress {
        n.full(full(p));
    }
    for g in &v.groups {
        for r in &g.rows {
            n.tone(&r.tone);
            if let Some(p) = r.progress {
                n.full(p >= 1.0);
            }
        }
    }
    for b in &v.blocks {
        n.block(b);
    }
    n.done > 0 && n.open == 0
}

/// Whether a bar is full: 0 of 0 isn't, it says nothing has been
/// counted yet (an empty bar), not that all of it is done.
fn full(p: &crate::views::Progress) -> bool {
    p.total > 0.0 && p.done >= p.total
}

#[derive(Default)]
struct Marks {
    done: usize,
    open: usize,
}

impl Marks {
    fn tone(&mut self, t: &str) {
        match t {
            "good" => self.done += 1,
            "bad" | "warn" | "busy" | "quiet" => self.open += 1,
            _ => {}
        }
    }

    fn full(&mut self, full: bool) {
        if full {
            self.done += 1;
        } else {
            self.open += 1;
        }
    }

    fn block(&mut self, b: &Block) {
        if let Some(p) = &b.progress {
            self.full(full(p));
        }
        match b.kind.as_str() {
            "checklist" => {
                for it in &b.items {
                    self.full(it.done == Some(true));
                }
            }
            // A step with no tone hasn't started.
            "steps" => {
                for it in &b.items {
                    if it.tone.is_empty() {
                        self.open += 1;
                    } else {
                        self.tone(&it.tone);
                    }
                }
            }
            // Cards are done in a good column, waiting in any other.
            "board" => {
                for col in &b.items {
                    for card in &col.items {
                        if col.tone == "good" || card.tone == "good" {
                            self.done += 1;
                        } else {
                            self.open += 1;
                        }
                    }
                }
            }
            _ => {
                self.tone(&b.tone);
                self.items(&b.items);
            }
        }
    }

    fn items(&mut self, its: &[Item]) {
        for it in its {
            self.tone(&it.tone);
            if let Some(d) = it.done {
                self.full(d);
            }
            if let Some(p) = it.progress {
                self.full(p >= 1.0);
            }
            self.items(&it.items);
        }
    }
}

/// The words of a shell command that change what a view of branches,
/// pull requests or CI shows: the change and what it names ("push",
/// "origin", "rush/views", "views"); None for a command that changes none
/// of that.
pub fn change_of(cmd: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    for seg in cmd.split(['\n', ';', '|', '&', '(', ')']) {
        let toks = shell_words(seg);
        let toks: Vec<&str> = toks.iter().map(String::as_str).collect();
        let Some(at) = toks.iter().position(|t| *t == "git" || *t == "gh") else {
            continue;
        };
        let rest = &toks[at + 1..];
        // git's own options before its verb: -C <dir>, -c <k=v>.
        let mut i = 0;
        while i < rest.len() && rest[i].starts_with('-') {
            i += if rest[i] == "-C" || rest[i] == "-c" {
                2
            } else {
                1
            };
        }
        let rest = rest.get(i..).unwrap_or(&[]);
        let (verb, args) = match (toks[at], rest) {
            ("git", [v, args @ ..]) if GIT_VERBS.contains(v) => (*v, args),
            ("gh", ["pr", v, args @ ..]) if GH_PR_VERBS.contains(v) => (*v, args),
            ("gh", ["run", "rerun", args @ ..]) => ("rerun", args),
            _ => continue,
        };
        words.push(verb.to_string());
        if verb == "commit" {
            continue; // its words are a message
        }
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
                continue;
            }
            if a.starts_with('-') {
                skip = matches!(*a, "-m" | "--body" | "--title" | "-t" | "-b" | "--subject");
                continue;
            }
            let a = a.trim_start_matches('+');
            let a = a.rsplit_once(':').map_or(a, |(_, to)| to); // src:dst
            if a.is_empty() || a.contains('=') {
                continue;
            }
            let a = a.to_lowercase();
            if let Some((_, last)) = a.rsplit_once('/') {
                words.push(last.to_string());
            }
            words.push(a);
        }
    }
    (!words.is_empty()).then_some(words)
}

/// seg's words, a quoted run of them one word.
fn shell_words(seg: &str) -> Vec<String> {
    let (mut out, mut w, mut q) = (Vec::new(), String::new(), None);
    for c in seg.chars() {
        match (q, c) {
            (Some(o), c) if c == o => q = None,
            (Some(_), c) => w.push(c),
            (None, '"' | '\'') => q = Some(c),
            (None, c) if c.is_whitespace() => {
                if !w.is_empty() {
                    out.push(std::mem::take(&mut w));
                }
            }
            (None, c) => w.push(c),
        }
    }
    if !w.is_empty() {
        out.push(w);
    }
    out
}

const GIT_VERBS: [&str; 8] = [
    "push",
    "merge",
    "rebase",
    "commit",
    "cherry-pick",
    "reset",
    "branch",
    "tag",
];
const GH_PR_VERBS: [&str; 5] = ["merge", "create", "close", "ready", "edit"];

/// Words that say a view shows branches, pull requests or CI: any change
/// in the list above can make it wrong.
const VCS_WORDS: [&str; 18] = [
    "branch", "branches", "ci", "checks", "pr", "prs", "pull", "merge", "merged", "land", "landed",
    "push", "pushed", "commit", "commits", "ahead", "behind", "rebase",
];

/// Whether v shows what a change of these words touches: it names what
/// the change named, or it is a view of branches, pull requests or CI.
fn shows(v: &View, change: &[String]) -> bool {
    let ws = view_words(v);
    VCS_WORDS.iter().any(|w| ws.contains(*w))
        || change.iter().any(|w| {
            w.len() > 2
                && !GIT_VERBS.contains(&w.as_str())
                && !GH_PR_VERBS.contains(&w.as_str())
                && !matches!(w.as_str(), "origin" | "head")
                && ws.contains(w)
        })
}

/// v's words, lower case, split where neither a letter, a digit nor one
/// of a branch's "/-_.#" is.
fn view_words(v: &View) -> HashSet<String> {
    let mut b = String::new();
    let mut add = |s: &str| {
        b.push_str(s);
        b.push(' ');
    };
    add(&v.title);
    add(&v.summary);
    add(&v.note);
    for s in &v.stats {
        add(&s.label);
    }
    for g in &v.groups {
        add(&g.title);
        for r in &g.rows {
            add(&r.text);
            add(&r.meta);
        }
    }
    fn items(its: &[Item], add: &mut dyn FnMut(&str)) {
        for it in its {
            add(&it.text);
            add(&it.meta);
            for c in &it.cells {
                add(c);
            }
            items(&it.items, add);
        }
    }
    for bl in &v.blocks {
        add(&bl.title);
        add(&bl.text);
        for c in &bl.columns {
            add(c);
        }
        items(&bl.items, &mut add);
    }
    let lower = b.to_lowercase();
    let mut out = HashSet::new();
    for w in lower.split(|c: char| !c.is_alphanumeric() && !"/-_.#".contains(c)) {
        let w = w.trim_matches(['.', '-', '#']);
        if w.is_empty() {
            continue;
        }
        if let Some((_, last)) = w.rsplit_once('/') {
            out.insert(last.to_string());
        }
        out.insert(w.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::{Group, Progress, Row};
    use rush_state::Time;

    const T0: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;

    fn row(text: &str, tone: &str) -> Row {
        Row {
            text: text.into(),
            tone: tone.into(),
            ..Row::default()
        }
    }

    fn view(title: &str, rows: Vec<Row>, at: i64) -> View {
        View {
            name: "v".into(),
            title: title.into(),
            groups: vec![Group {
                rows,
                ..Group::default()
            }],
            updated: Time::from_ms(at),
            ..View::default()
        }
    }

    fn turn(start: i64, changes: &[(i64, &str)]) -> Turn {
        Turn {
            start,
            changes: changes
                .iter()
                .map(|(at, cmd)| Change {
                    at: *at,
                    words: change_of(cmd).expect(cmd),
                })
                .collect(),
        }
    }

    #[test]
    fn at_turn_end_rule() {
        let branches = || {
            view(
                "Branches",
                vec![row("rush/views", "busy"), row("rush/perf", "good")],
                T0 + MIN,
            )
        };
        let tests = || view("Test run", vec![row("ui", "busy")], T0 + MIN);
        let done = || {
            view(
                "Tests",
                vec![row("ui", "good"), row("host", "good")],
                T0 + MIN,
            )
        };
        let mut greyed = tests();
        greyed.updated = Time::from_ms(T0 - 10 * MIN);
        greyed.greyed = WHY_TURN.into();
        let mut greyed_once = greyed.clone();
        greyed_once.grey_turns = 1;
        let mut pinned = tests();
        pinned.pinned = true;
        pinned.updated = Time::from_ms(T0 - 10 * MIN);
        let mut plugin = done();
        plugin.by = "prwatch".into();
        let mut facts = tests();
        facts.groups[0].rows = vec![row("node", ""), row("db", "")];
        let push = [(T0 + 2 * MIN, "git push origin rush/views")];
        let cases: Vec<(&str, View, Turn, Verdict)> = vec![
            (
                "set this turn, still running",
                tests(),
                turn(T0, &[]),
                Verdict::Keep,
            ),
            (
                "all done greys at once",
                done(),
                turn(T0, &[]),
                Verdict::Grey(WHY_DONE),
            ),
            (
                "left alone for the whole turn",
                tests(),
                turn(T0 + 5 * MIN, &[]),
                Verdict::Grey(WHY_TURN),
            ),
            (
                "a push after its last set",
                branches(),
                turn(T0, &push),
                Verdict::Grey(WHY_BEHIND),
            ),
            (
                "a landing it names",
                view("Work", vec![row("views branch", "busy")], T0 + MIN),
                turn(T0, &[(T0 + 2 * MIN, "gh pr merge 12 --squash")]),
                Verdict::Grey(WHY_BEHIND),
            ),
            (
                "set again after the push",
                branches(),
                turn(T0, &[(T0 + MIN / 2, "git push")]),
                Verdict::Keep,
            ),
            (
                "a push says nothing of a test run",
                tests(),
                turn(T0, &push),
                Verdict::Keep,
            ),
            (
                "greyed, one more turn untouched",
                greyed.clone(),
                turn(T0, &[]),
                Verdict::Greyed(1),
            ),
            (
                "greyed, two more turns: put away",
                greyed_once,
                turn(T0, &[]),
                Verdict::Away,
            ),
            ("pinned stays", pinned, turn(T0, &[]), Verdict::Keep),
            ("a plugin's stays", plugin, turn(T0, &[]), Verdict::Keep),
            (
                "plain facts aren't done",
                facts,
                turn(T0, &[]),
                Verdict::Keep,
            ),
        ];
        for (what, v, t, want) in cases {
            assert_eq!(at_turn_end(&v, &t), want, "{what}");
        }
    }

    #[test]
    fn by_clock_rule() {
        let v = view("Tests", vec![row("ui", "busy")], T0);
        let mut grey = v.clone();
        grey.greyed = WHY_CLOCK.into();
        for (what, v, now, want) in [
            ("fresh", &v, T0 + 9 * MIN, Verdict::Keep),
            ("ten minutes", &v, T0 + 10 * MIN, Verdict::Grey(WHY_CLOCK)),
            ("already grey", &grey, T0 + 20 * MIN, Verdict::Keep),
            ("45 minutes", &grey, T0 + 45 * MIN, Verdict::Away),
        ] {
            assert_eq!(by_clock(v, now), want, "{what}");
        }
    }

    #[test]
    fn finished_views() {
        let block = |kind: &str, items: Vec<Item>| View {
            title: "x".into(),
            blocks: vec![Block {
                kind: kind.into(),
                items,
                ..Block::default()
            }],
            ..View::default()
        };
        let item = |tone: &str, done: Option<bool>| Item {
            text: "a".into(),
            tone: tone.into(),
            done,
            ..Item::default()
        };
        let bar = |done: f64, total: f64| {
            let mut v = block("list", vec![item("good", None)]);
            v.progress = Some(Progress {
                done,
                total,
                ..Progress::default()
            });
            v
        };
        for (what, v, want) in [
            (
                "list all good",
                block("list", vec![item("good", None); 2]),
                true,
            ),
            (
                "one busy",
                block("list", vec![item("good", None), item("busy", None)]),
                false,
            ),
            (
                "checklist ticked",
                block("checklist", vec![item("", Some(true)); 3]),
                true,
            ),
            (
                "checklist one to go",
                block("checklist", vec![item("", Some(true)), item("", None)]),
                false,
            ),
            (
                "steps not started",
                block("steps", vec![item("good", None), item("", None)]),
                false,
            ),
            ("no states", block("table", vec![item("", None)]), false),
            ("a bar not full", bar(3.0, 5.0), false),
            ("a bar full", bar(5.0, 5.0), true),
            ("a bar of 0 of 0", bar(0.0, 0.0), false),
        ] {
            assert_eq!(finished(&v), want, "{what}");
        }
    }

    #[test]
    fn changes_of_commands() {
        let w = |s: &[&str]| Some(s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        for (cmd, want) in [
            (
                "git push origin rush/views-live",
                w(&["push", "origin", "views-live", "rush/views-live"]),
            ),
            (
                "cd x && git -C ../r push -u origin HEAD:rush/a",
                w(&["push", "origin", "a", "rush/a"]),
            ),
            ("git commit -m 'fix the push'", w(&["commit"])),
            ("gh pr merge 12 --squash", w(&["merge", "12"])),
            ("gh pr create --title 'x y' --body z", w(&["create"])),
            ("git status && cargo test", None),
            ("gh pr view 12", None),
        ] {
            assert_eq!(change_of(cmd), want, "{cmd}");
        }
    }

    /// The agent's prompt says the limits above, not others.
    #[test]
    fn the_prompt_says_these_limits() {
        let p = crate::views::VIEWS_PROMPT;
        for want in [
            format!("{} min", STALE_MS / MIN),
            format!("{AWAY_TURNS} turns later"),
        ] {
            assert!(p.contains(&want), "{want:?} not in the prompt");
        }
    }

    #[test]
    fn the_note_is_one_short_stable_line() {
        let n = note(&["prs".into(), "tests".into()]);
        assert_eq!(
            n,
            "rush: stale views: prs, tests. Set each again with what's current, or clear it."
        );
        assert!(!n.contains('\n') && !n.contains('<') && n.len() < 90);
    }
}
