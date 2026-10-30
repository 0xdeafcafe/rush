//! The finishing card: once a session's turn has ended with a pull
//! request open, the card above its box says first what stands between it
//! and merged ("PR #118 can't merge yet: the unit check fails"), then each
//! step on the way, and keys for what to do: f has the agent fix it, m
//! merges, w sets it aside. It never takes the keys by itself; ↑ gives it
//! them. After Go's internal/ui/finish.go (finishFor :51, finishShow :125,
//! needs :144, headline :162, failedChecks :191, finishRows :203,
//! finishStrip :335, finishTabText :359, prFacts :381, fromStatus :409,
//! finishKey :432, fixText :492) and prs.go (prStateColor :808,
//! cellwTrunc :804). Not here yet: the session that shows it (hosting's),
//! asking GitHub and the merge sheet (prs.go), so `build` takes the pull
//! requests ready made and `key` says what the session should do; the
//! number isn't a terminal link, rush_core's spans have none.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use rush_core::text::{Color, Line, width};
use rush_gh::ghpr::{PR, Status};

use crate::style::*;

/// The most checks listed under the steps (finishRows' most).
const MOST: usize = 8;

/// One pull request on the card (finishTab).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FinishTab {
    pub id: String,
    /// The repository GitHub was asked in; "" when only the transcript
    /// knows it.
    pub root: String,
    pub pr: PR,
}

/// The pull request a finishing card is about, and what it says of it
/// (finishCard). An agent on several has them all, a tab each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FinishCard {
    pub id: String,
    pub root: String,
    pub pr: PR,
    /// How it's being merged, while it is.
    pub merging: String,
    /// Whether GitHub is being asked about it again.
    pub asking: bool,
    /// Why its last merge from here didn't go through.
    pub failed: Option<String>,
    pub tabs: Vec<FinishTab>,
    pub at: usize,
}

/// What the session's pull requests stand at, for `build` (finishFor's
/// reads of the agent, m.prs and the session).
pub struct Pick<'a> {
    /// Every pull request GitHub knows the agent's on: its repository's
    /// root, and the pull request.
    pub prs: &'a [(String, PR)],
    /// Its branch's pull request, any harness's, when GitHub lists none.
    pub status: Option<&'a Status>,
    /// Numbers merged from here, GitHub's lists not caught up yet.
    pub merged: &'a HashSet<u64>,
    /// How each number is being merged, and why a merge failed.
    pub merging: &'a HashMap<u64, String>,
    pub merge_failed: &'a HashMap<u64, String>,
    /// Whether GitHub is being asked about the folder again.
    pub asking: bool,
    /// The cards set aside (finishGone).
    pub gone: &'a HashSet<String>,
    /// The tab picked last (finishAt), 0 for none.
    pub at: u64,
    /// The card as last drawn, and whether it has the keys.
    pub old: Option<&'a FinishCard>,
    pub focus: bool,
}

static NO_NUMBERS: LazyLock<HashSet<u64>> = LazyLock::new(HashSet::new);
static NO_WORDS: LazyLock<HashMap<u64, String>> = LazyLock::new(HashMap::new);
static NO_IDS: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);

impl Default for Pick<'_> {
    fn default() -> Self {
        Pick {
            prs: &[],
            status: None,
            merged: &NO_NUMBERS,
            merging: &NO_WORDS,
            merge_failed: &NO_WORDS,
            asking: false,
            gone: &NO_IDS,
            at: 0,
            old: None,
            focus: false,
        }
    }
}

/// The card for a session whose turn is over, if it has pull requests
/// open or one merged since (finishFor, from its pick on).
pub fn build(p: &Pick) -> Option<FinishCard> {
    // Every open one, and the latest merged: what was just finished.
    let mut pick: Vec<(String, PR)> = Vec::new();
    let mut merged: Option<usize> = None;
    for (root, pr) in p.prs {
        if pr.state == "OPEN" {
            pick.push((root.clone(), pr.clone()));
        } else if pr.state == "MERGED" && merged.is_none_or(|i| pr.number > pick[i].1.number) {
            if let Some(i) = merged {
                pick.remove(i);
            }
            merged = Some(pick.len());
            pick.push((root.clone(), pr.clone()));
        }
    }
    if pick.is_empty()
        && let Some(s) = p.status
        && (s.open() || s.merged())
    {
        pick.push((String::new(), from_status(s)));
    }
    let mut f = FinishCard {
        asking: p.asking,
        ..FinishCard::default()
    };
    for (root, mut pr) in pick {
        if p.merged.contains(&pr.number) && pr.state == "OPEN" {
            pr.state = "MERGED".into(); // merged from here: GitHub's lists catch up
        }
        let (ok, failed, pending) = pr.counts();
        let id = format!(
            "finish:{}:{}:{ok}/{failed}/{pending}:{}",
            pr.number, pr.state, pr.decision
        );
        if p.gone.contains(&id) {
            continue;
        }
        f.tabs.push(FinishTab { id, root, pr });
    }
    if f.tabs.is_empty() {
        return None;
    }
    f.tabs.sort_by_key(|t| t.pr.number);
    // The one you picked while it's still here; else, one that just
    // changed; else the one that most needs you.
    let mut at = f.tabs.iter().rposition(|t| t.pr.number == p.at);
    if let Some(old) = p.old
        && !p.focus
    {
        for (i, t) in f.tabs.iter().enumerate() {
            if old
                .tabs
                .iter()
                .rev()
                .find(|o| o.pr.number == t.pr.number)
                .is_some_and(|o| o.id != t.id)
            {
                at = Some(i); // it moved on: show it
            }
        }
    }
    let at = at.unwrap_or_else(|| {
        let mut at = 0;
        for (i, t) in f.tabs.iter().enumerate() {
            if needs(&t.pr) > needs(&f.tabs[at].pr) {
                at = i;
            }
        }
        at
    });
    f.show(at, p.merging, p.merge_failed);
    Some(f)
}

impl FinishCard {
    /// Puts tab i up (finishShow); finishAt is its number.
    pub fn show(
        &mut self,
        i: usize,
        merging: &HashMap<u64, String>,
        failed: &HashMap<u64, String>,
    ) {
        let Some(t) = self.tabs.get(i) else {
            return;
        };
        self.at = i;
        self.id = t.id.clone();
        self.root = t.root.clone();
        self.pr = t.pr.clone();
        self.merging = merging.get(&t.pr.number).cloned().unwrap_or_default();
        self.failed = failed.get(&t.pr.number).cloned();
    }

    /// What stands between the pull request and merged, in words, and
    /// its colour (headline).
    pub fn headline(&self) -> (String, Color) {
        let p = &self.pr;
        let n = format!("PR #{}", p.number);
        let (ok, failed, pending) = p.counts();
        if p.state == "MERGED" {
            (format!("{n} is merged"), GREEN)
        } else if !self.merging.is_empty() {
            (format!("{n} is merging…"), ORANGE)
        } else if let Some(e) = &self.failed {
            (format!("{n} didn't merge: {e}"), RED)
        } else if failed == 1 {
            let name = failed_checks(p).into_iter().next().unwrap_or_default();
            let name = if name.is_empty() { "one" } else { name };
            (format!("{n} can't merge yet: the {name} check fails"), RED)
        } else if failed > 1 {
            (format!("{n} can't merge yet: {failed} checks fail"), RED)
        } else if p.decision == "CHANGES_REQUESTED" {
            (
                format!("{n} can't merge yet: changes were asked for"),
                YELLOW,
            )
        } else if pending > 0 {
            (
                format!(
                    "{n} is waiting on its checks: {ok} of {} done",
                    ok + pending
                ),
                BLUE,
            )
        } else if p.draft {
            (format!("{n} is a draft: its checks pass"), BLUE)
        } else if p.decision == "REVIEW_REQUIRED" {
            (
                format!("{n} passes its checks and waits for a review"),
                BLUE,
            )
        } else {
            (format!("{n} is ready to merge"), GREEN)
        }
    }
}

/// How much a pull request needs you, to show first the one that most
/// does: failing or changes asked, then ready, a review, its checks, a
/// draft, and merged last (needs).
pub fn needs(p: &PR) -> u8 {
    let (_, failed, pending) = p.counts();
    if p.state == "MERGED" {
        0
    } else if failed > 0 || p.decision == "CHANGES_REQUESTED" {
        6
    } else if p.draft {
        1
    } else if pending > 0 {
        3
    } else if p.decision == "REVIEW_REQUIRED" {
        4
    } else {
        5
    }
}

/// The names of a pull request's failing checks, "" for one only the
/// transcript counted (failedChecks).
pub fn failed_checks(p: &PR) -> Vec<&str> {
    p.checks
        .iter()
        .filter(|c| c.state == "failed")
        .map(|c| c.name.as_str())
        .collect()
}

/// A pull request's colour: green open, red while its checks fail or
/// changes are asked, blue merged, quiet a draft or closed
/// (prStateColor).
pub fn pr_state_color(p: &PR) -> Color {
    let (_, failed, _) = p.counts();
    if p.state == "MERGED" {
        BLUE
    } else if p.state == "CLOSED" || p.draft {
        DIM
    } else if failed > 0 || p.decision == "CHANGES_REQUESTED" {
        RED
    } else {
        GREEN
    }
}

/// A pull request on its tab: its number, and in a word or a mark where
/// it stands (finishTabText).
pub fn tab_text(p: &PR) -> Line {
    let (ok, failed, pending) = p.counts();
    let (word, c) = if p.state == "MERGED" {
        ("merged".to_string(), BLUE)
    } else if failed > 0 {
        (format!("✗ {failed} failing"), RED)
    } else if p.decision == "CHANGES_REQUESTED" {
        ("changes asked".into(), YELLOW)
    } else if pending > 0 {
        (format!("◌ {ok}/{}", ok + pending), ORANGE)
    } else if p.draft {
        ("draft".into(), DIM)
    } else if p.decision == "REVIEW_REQUIRED" {
        ("review".into(), BLUE)
    } else {
        ("✓ ready".into(), GREEN)
    };
    line([
        sp(format!("#{}", p.number), pr_state_color(p)),
        plain(" "),
        sp(word, c),
    ])
}

fn plural(n: u64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// left, then right at the end of w cells; left alone, fit, when they
/// don't both fit with two between (rushmode.go:2175 spread).
fn spread(left: Line, right: Line, w: usize) -> Line {
    let gap = w as isize - left.width() as isize - right.width() as isize;
    if gap < 2 {
        return fit(left, w);
    }
    let mut out = left;
    out.spans.push(plain(blanks(gap as usize)));
    join(&mut out, right);
    out
}

/// A pull request's title, then how big it is, its comments and where
/// its review stands, as much as GitHub has said (prFacts).
pub fn pr_facts(p: &PR, w: usize) -> Line {
    let mut facts: Vec<Line> = Vec::new();
    if p.added + p.removed > 0 {
        facts.push(line([
            sp(format!("+{}", p.added), GREEN),
            plain(" "),
            sp(format!("−{}", p.removed), RED),
        ]));
    }
    if p.files > 0 {
        facts.push(line([sp(
            format!("{} file{}", p.files, plural(p.files)),
            DIM,
        )]));
    }
    if p.comments > 0 {
        let n = p.comments as u64;
        facts.push(line([sp(format!("{n} comment{}", plural(n)), TEXT)]));
    }
    if !p.waiting.is_empty() {
        facts.push(line([sp(
            format!("review asked of {}", p.waiting.join(", ")),
            DIM,
        )]));
    }
    let mut right = Line::new();
    for (i, f) in facts.into_iter().enumerate() {
        if i > 0 {
            right.spans.push(sp(" · ", FAINT));
        }
        join(&mut right, f);
    }
    let room = 10.max(w as isize - right.width() as isize - 3) as usize;
    let title = rush_draw::truncate(&one_line(&p.title), room, "…").into_owned();
    spread(line([sp(title, TEXT)]), right, w)
}

/// A pull request as its branch's status says it: its checks only told
/// as a whole, one standing in for all (fromStatus).
pub fn from_status(s: &Status) -> PR {
    let draft = s.state == "draft";
    let mut p = PR {
        number: s.number,
        title: s.title.clone(),
        url: s.url.clone(),
        state: if draft {
            "OPEN".into()
        } else {
            s.state.to_uppercase()
        },
        draft,
        ..PR::default()
    };
    p.decision = match s.review.as_str() {
        "approved" => "APPROVED".into(),
        "changes" => "CHANGES_REQUESTED".into(),
        _ => String::new(),
    };
    let check = |name: &str, state: &str| rush_gh::ghpr::Check {
        name: name.into(),
        state: state.into(),
        ..Default::default()
    };
    p.checks = match s.checks.as_str() {
        "pass" => vec![check("", "passed")],
        "fail" => vec![check(&s.failing, "failed")],
        "pending" => vec![check("", "pending")],
        _ => Vec::new(),
    };
    p
}

/// A tab on the strip, to click: from x, w cells, its pull request's
/// number (finishBtns).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabHit {
    pub x: usize,
    pub w: usize,
    pub number: u64,
}

/// The card's top row with several pull requests: a tab each, its
/// number and where it stands, the one shown lit (finishStrip).
pub fn strip(f: &FinishCard, w: usize, hits: &mut Vec<TabHit>) -> Line {
    let mut out = Line::new();
    let mut x = 3; // past the card's edge and its room
    for (i, t) in f.tabs.iter().enumerate() {
        let mut tab = Line::new();
        if i == f.at {
            tab.spans.push(spb("▸", BRIGHT));
            let mut lit = line([spb(" ", BRIGHT)]);
            join(&mut lit, tab_text(&t.pr));
            lit.spans.push(spb(" ", BRIGHT));
            for mut s in lit.spans {
                s.style.bg = SEL_BG;
                s.style.mods = s.style.mods.with(rush_core::text::Mods::BOLD);
                tab.spans.push(s);
            }
        } else {
            tab.spans.push(plain("  "));
            join(&mut tab, tab_text(&t.pr));
            tab.spans.push(plain(" "));
        }
        let tw = tab.width();
        if x + tw > w {
            out.spans.push(sp(format!(" +{}", f.tabs.len() - i), DIM));
            break;
        }
        hits.push(TabHit {
            x,
            w: tw,
            number: t.pr.number,
        });
        join(&mut out, tab);
        x += tw;
    }
    out
}

/// The card drawn w wide: the headline, the failing checks, the steps
/// with each check under them, and its keys (finishRows). focus is
/// whether it has the keys; hits gets the strip's tabs.
pub fn rows(f: &FinishCard, focus: bool, w: usize, hits: &mut Vec<TabHit>) -> Vec<Line> {
    let p = &f.pr;
    let (head, col) = f.headline();
    let bar = sp("▍", if focus { ORANGE } else { col });
    let inner = w.saturating_sub(4);
    let mut out = Vec::new();
    let mut row = |s: Line| {
        let mut l = line([bar.clone(), plain("  ")]);
        join(&mut l, fit(s, inner));
        out.push(on_ground(l, w, CHROME_BG));
    };
    hits.clear();
    if f.tabs.len() > 1 {
        row(strip(f, inner, hits));
    } else {
        row(Line::new()); // room above, as below
    }
    let mut num = Line::new();
    if f.asking {
        num.spans.push(sp("◌ ", ORANGE));
        num.spans.push(sp("asking GitHub  ", DIM));
    }
    num.spans.push(sp(format!("#{} ↗", p.number), BLUE));
    row(spread(line([spb(head, BRIGHT)]), num, inner));
    row(pr_facts(p, inner));
    for name in failed_checks(p) {
        if !name.is_empty() {
            row(line([sp("✗ ", RED), sp(name, TEXT)]));
        }
    }
    row(Line::new());
    let (ok, failed, pending) = p.counts();
    // ✓ done, ✗ stopped, – not needed here, ▸ where it is now, ○ still
    // to come.
    let mut here = false;
    let step = |mark: &str, c: Color, s: &str, t: Color| line([sp(mark, c), plain(" "), sp(s, t)]);
    let next = |s: &str, here: &mut bool| {
        if *here {
            return step("○", DIM, s, DIM);
        }
        *here = true;
        line([spb(format!("▸ {s}"), ORANGE)])
    };
    let opened = if p.draft {
        "opened as a draft"
    } else {
        "opened"
    };
    let mut steps = vec![
        step("✓", GREEN, "pushed", TEXT),
        step("✓", GREEN, opened, TEXT),
    ];
    let total = ok + failed + pending;
    let checks = format!("checks {ok} of {total}");
    steps.push(if total == 0 {
        step("–", DIM, "no checks", DIM)
    } else if failed > 0 {
        here = true;
        step("✗", RED, &checks, TEXT)
    } else if pending > 0 {
        here = true;
        step("◌", ORANGE, &checks, TEXT)
    } else {
        step("✓", GREEN, &checks, TEXT)
    });
    steps.push(match p.decision.as_str() {
        "APPROVED" => step("✓", GREEN, "approved", TEXT),
        "CHANGES_REQUESTED" => {
            here = true;
            step("✗", YELLOW, "changes asked for", TEXT)
        }
        "REVIEW_REQUIRED" => next("review", &mut here),
        _ => step("–", DIM, "no review needed", DIM),
    });
    steps.push(if p.state == "MERGED" {
        step("✓", GREEN, "merged", TEXT)
    } else if !f.merging.is_empty() {
        step("◌", ORANGE, &format!("merging ({})…", f.merging), TEXT)
    } else if f.failed.is_some() {
        step("✗", RED, "merge failed", TEXT)
    } else {
        next("merge", &mut here)
    });
    let mut all = Line::new();
    for (i, s) in steps.into_iter().enumerate() {
        if i > 0 {
            all.spans.push(plain("   "));
        }
        join(&mut all, s);
    }
    row(all);
    let named: Vec<_> = p.checks.iter().filter(|c| !c.name.is_empty()).collect();
    for (i, ch) in named.iter().enumerate() {
        if i == MOST {
            row(line([
                sp("└─ ", FAINT),
                sp(format!("{} more", named.len() - MOST), DIM),
            ]));
            break;
        }
        let branch = if i == named.len() - 1 {
            "└─ "
        } else {
            "├─ "
        };
        let (mark, mc) = match ch.state.as_str() {
            "passed" => ("✓", GREEN),
            "skipped" => ("–", DIM),
            "failed" => ("✗", RED),
            _ => ("◌", ORANGE),
        };
        row(line([
            sp(branch, FAINT),
            sp(mark, mc),
            plain(" "),
            sp(ch.name.clone(), TEXT),
            plain("  "),
            sp(ch.workflow.clone(), DIM),
        ]));
    }
    row(Line::new());
    let k = |key: &str, label: &str| line([spb(key, TEXT), plain(" "), sp(label, SUB)]);
    let mut keys = Vec::new();
    if failed > 0 || p.decision == "CHANGES_REQUESTED" {
        keys.push(k("f", "fix it with this agent"));
    }
    if p.state == "OPEN" && f.merging.is_empty() {
        let mut label = "merge";
        if failed + pending > 0 || !p.decision.is_empty() && p.decision != "APPROVED" {
            label = "merge anyway";
        }
        if f.failed.is_some() {
            label = "merge again";
        }
        keys.push(k("m", label));
    }
    keys.push(k("w", "set aside"));
    keys.push(k("g", "GitHub"));
    if f.tabs.len() > 1 {
        keys.push(k("← →", "the others"));
    }
    if !focus {
        keys.push(line([sp("↑ gives it the keys", FAINT)]));
    }
    let mut kl = Line::new();
    for (i, l) in keys.into_iter().enumerate() {
        if i > 0 {
            kl.spans.push(plain("    "));
        }
        join(&mut kl, l);
    }
    row(kl);
    row(Line::new());
    out
}

/// What a key on the card asks of the session (finishKey's commands).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishAct {
    /// Not the card's: the key goes on.
    Pass,
    /// Taken, nothing more to do.
    Used,
    /// The card gives the keys back and this goes to the agent (f).
    Send(String),
    /// The merge sheet for the pull request shown (m).
    Merge,
    /// rush hasn't heard from GitHub about it: say so.
    Flash(&'static str),
    /// The pull request in the browser (g).
    Browse(String),
    /// Every tab set aside: the card is gone.
    Gone,
}

/// The card's keys, only while it has them (finishKey). "pr:N" is a
/// click on a tab; gone gets what w sets aside.
pub fn key(
    f: &mut FinishCard,
    focus: bool,
    s: &str,
    gone: &mut HashSet<String>,
    merging: &HashMap<u64, String>,
    failed_merges: &HashMap<u64, String>,
) -> FinishAct {
    if !focus {
        return FinishAct::Pass;
    }
    if let Some(n) = s.strip_prefix("pr:") {
        if let Some(i) = f.tabs.iter().position(|t| t.pr.number.to_string() == n) {
            f.show(i, merging, failed_merges);
        }
        return FinishAct::Used;
    }
    let d: isize = match s {
        "left" | "shift+tab" | "h" => -1,
        "right" | "tab" | "l" => 1,
        _ => 0,
    };
    if d != 0 && f.tabs.len() > 1 {
        let n = f.tabs.len() as isize;
        f.show(
            ((f.at as isize + d + n) % n) as usize,
            merging,
            failed_merges,
        );
        return FinishAct::Used;
    }
    let (_, failed, _) = f.pr.counts();
    match s {
        "f" => {
            if failed == 0 && f.pr.decision != "CHANGES_REQUESTED" {
                return FinishAct::Used;
            }
            FinishAct::Send(fix_text(&f.pr))
        }
        "m" => {
            if f.pr.state != "OPEN" {
                FinishAct::Used
            } else if f.root.is_empty() {
                FinishAct::Flash("rush hasn't heard from GitHub about this pull request yet")
            } else {
                FinishAct::Merge
            }
        }
        "w" => {
            // Just this one: the others stay on the card.
            gone.insert(f.id.clone());
            if f.at < f.tabs.len() {
                f.tabs.remove(f.at);
            }
            if f.tabs.is_empty() {
                return FinishAct::Gone;
            }
            let at = f.at.min(f.tabs.len() - 1);
            f.show(at, merging, failed_merges);
            FinishAct::Used
        }
        "g" => FinishAct::Browse(f.pr.url.clone()),
        _ => FinishAct::Pass,
    }
}

/// What f asks the agent: the failing checks by name, or the changes a
/// reviewer asked for (fixText).
pub fn fix_text(p: &PR) -> String {
    let names = failed_checks(p);
    if !names.is_empty() {
        let named = names.join(", ");
        let which = if names.len() == 1 && !names[0].is_empty() {
            format!("The {} check", names[0])
        } else if !named.trim_matches([',', ' ']).is_empty() {
            format!("These checks ({named})")
        } else {
            "Its failing checks".to_string()
        };
        return format!(
            "PR #{} can't merge yet. {which} fails. Read why in its log (gh run view --log-failed), fix it, push, and tell me once the checks pass.",
            p.number
        );
    }
    format!(
        "PR #{0} had changes asked for in review. Read the review (gh pr view {0} --comments), make the changes, push, and tell me what you changed.",
        p.number
    )
}

/// Whether every row is at most w cells: the card never spills.
pub fn fits(rows: &[Line], w: usize) -> bool {
    rows.iter().all(|r| width(&r.plain()) <= w)
}
