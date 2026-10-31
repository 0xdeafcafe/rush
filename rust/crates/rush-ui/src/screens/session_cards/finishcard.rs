//! The finishing card in the open Session: once its turn is over with a
//! pull request open (or one merged since), the card above the box says
//! what stands between it and merged, and f has the agent fix it, m
//! merges, w sets it aside, g opens it, ← → go between its pull requests.
//! It never takes the keys by itself; ↑ gives it them. The card itself is
//! list_extras::finish's; this is the Session's side of it, after Go's
//! internal/ui/finish.go (finishFor :51, finishShow :125, finishKey :432),
//! prs.go (agentPRs :755, fromTranscript :785) and rushmode.go (paneDock's
//! c.finish :3383, cardRows' finish :3121, cardKind :5956, cardID :5975).

use std::collections::{HashMap, HashSet};

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_gh::ghpr::{Check, PR, Status};

use super::card::CardBtn;
use super::{Act, Conn, act, pending, session};
use crate::model::Model;
use crate::screens::list_extras::finish::{self, FinishAct, FinishCard, Pick, TabHit};

/// The Session's side of the card: the card as last built, the ones set
/// aside, and the number of the one picked (c.finish, finishGone,
/// finishAt).
#[derive(Clone, Debug, Default)]
pub struct FinishState {
    pub card: Option<FinishCard>,
    pub gone: HashSet<String>,
    pub at: u64,
    pub hits: Vec<TabHit>,
}

/// Where agent a's pull requests are kept: its row's folder (folderKey).
fn root_of(m: &Model, a: &Agent) -> String {
    let i = m.agents.iter().position(|x| x.key == a.key);
    let root = i.and_then(|i| m.roots.get(i)).map_or("", String::as_str);
    crate::list::folder_key(a, root).to_string()
}

/// A pull request as an agent's transcript says it: its checks only
/// counted, so each stands in nameless (fromTranscript).
fn from_transcript(t: &rush_core::adapter::Pr) -> PR {
    let mut p = PR {
        number: t.number.max(0) as u64,
        title: t.title.clone(),
        url: t.url.clone(),
        state: t.state.clone(),
        decision: t.review.clone(),
        mine: true,
        ..PR::default()
    };
    for (state, n) in [
        ("passed", t.checks.passed),
        ("failed", t.checks.failed),
        ("pending", t.checks.pending),
    ] {
        for _ in 0..n.max(0) {
            p.checks.push(Check {
                state: state.into(),
                ..Check::default()
            });
        }
    }
    p
}

/// Every pull request GitHub knows agent a's on, its branch's or one its
/// transcript names, with the repository's root; then the transcript's
/// own GitHub hasn't listed, with none (agentPRs).
pub fn agent_prs(m: &Model, a: &Agent) -> Vec<(String, PR)> {
    let root = root_of(m, a);
    let data = m
        .kept
        .peek::<crate::screens::projects_machine::data::Data>();
    let gh = data
        .and_then(|d| d.prs.get(&root))
        .map_or(&[][..], |r| r.prs.as_slice());
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for p in gh {
        let known = !a.branch.is_empty() && p.head == a.branch && p.head != p.base
            || a.prs.iter().any(|t| t.number.max(0) as u64 == p.number);
        if known && seen.insert(p.number) {
            out.push((root.clone(), p.clone()));
        }
    }
    for t in &a.prs {
        if seen.insert(t.number.max(0) as u64) {
            out.push((String::new(), from_transcript(t)));
        }
    }
    out
}

/// The agent's pull request found by its branch, as gh's Status.
fn status_of(a: &Agent) -> Option<Status> {
    a.pr.as_ref().map(|p| Status {
        number: p.number,
        title: p.title.clone(),
        url: p.url.clone(),
        branch: p.branch.clone(),
        state: p.state.clone(),
        checks: p.checks.clone(),
        failing: p.failing.clone(),
        review: p.review.clone(),
        head_oid: p.head_oid.clone(),
        merged_at: p.merged_at,
    })
}

/// How each of root's pull requests is being merged from here, why the
/// last merge failed, and which were merged, by number (m.prs.merging,
/// mergeFailed, merged by "root#n").
fn merges(m: &Model, root: &str) -> (HashMap<u64, String>, HashMap<u64, String>, HashSet<u64>) {
    let (mut merging, mut failed, mut merged) = (HashMap::new(), HashMap::new(), HashSet::new());
    let Some(d) = m
        .kept
        .peek::<crate::screens::projects_machine::data::Data>()
    else {
        return (merging, failed, merged);
    };
    let num = |k: &str| {
        k.strip_prefix(root)
            .and_then(|r| r.strip_prefix('#'))
            .and_then(|n| n.parse::<u64>().ok())
    };
    for (k, how) in &d.merging {
        if let Some(n) = num(k) {
            merging.insert(n, how.clone());
        }
    }
    for (k, (why, _)) in &d.merge_failed {
        if let Some(n) = num(k) {
            failed.insert(n, why.clone());
        }
    }
    for k in &d.merged {
        if let Some(n) = num(k) {
            merged.insert(n);
        }
    }
    (merging, failed, merged)
}

/// The finishing card for session key, if any: its turn over, with pull
/// requests open, or one merged since (finishFor). Kept on c as drawn.
pub fn sync(m: &Model, key: &str, c: &mut Conn) {
    let Some(a) = m.agents.iter().find(|a| a.key == key) else {
        c.finish.card = None;
        return;
    };
    // Its turn over: what it left running (a follower waiting on the
    // checks) doesn't hide what the pull request needs.
    if a.job.live() || session(m, key).is_none() || !pending(m, key).is_empty() {
        c.finish.card = None;
        return;
    }
    let prs = agent_prs(m, a);
    let status = status_of(a);
    let root = root_of(m, a);
    let (merging, failed, merged) = merges(m, &root);
    let asking = m
        .kept
        .peek::<crate::screens::projects_machine::data::Data>()
        .is_some_and(|d| d.asking.contains(&root));
    let old = c.finish.card.take();
    c.finish.card = finish::build(&Pick {
        prs: &prs,
        status: status.as_ref(),
        merged: &merged,
        merging: &merging,
        merge_failed: &failed,
        asking,
        gone: &c.finish.gone,
        at: c.finish.at,
        old: old.as_ref(),
        focus: c.card_focus,
    });
}

/// The card's rows, w wide, its tabs kept as buttons on its first row
/// (finishRows, finishBtns).
pub fn rows(c: &mut Conn, w: usize) -> Vec<Line> {
    let Some(f) = c.finish.card.as_ref() else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    let out = finish::rows(f, c.card_focus, w, &mut hits);
    for h in &hits {
        c.btns.push(CardBtn {
            x: h.x,
            y: 0,
            w: h.w,
            key: format!("pr:{}", h.number),
        });
    }
    c.finish.hits = hits;
    out
}

/// The card's keys, only while it has them; Pass when the key isn't its
/// (finishKey).
pub fn key(m: &mut Model, key: &str, c: &mut Conn, s: &str) -> crate::frame::Out {
    use crate::frame::Out;
    let Some(mut f) = c.finish.card.take() else {
        return Out::Pass;
    };
    let root = m
        .agents
        .iter()
        .find(|a| a.key == key)
        .map(|a| root_of(m, a))
        .unwrap_or_default();
    let (merging, failed, _) = merges(m, &root);
    let r = finish::key(
        &mut f,
        c.card_focus,
        s,
        &mut c.finish.gone,
        &merging,
        &failed,
    );
    c.finish.at = f.pr.number;
    let pr = f.pr.clone();
    let froot = f.root.clone();
    c.finish.card = Some(f);
    match r {
        FinishAct::Pass => Out::Pass,
        FinishAct::Used => Out::Draw,
        FinishAct::Send(text) => {
            c.card_focus = false;
            act(m, key, Act::Send(text))
        }
        FinishAct::Merge => {
            crate::screens::projects_machine::prsheet::ask_merge_pr(m, &froot, &pr);
            Out::Draw
        }
        FinishAct::Flash(why) => {
            m.flash_err(why);
            Out::Draw
        }
        FinishAct::Browse(url) => {
            crate::screens::projects_machine::place::browse(m, &url);
            Out::Draw
        }
        FinishAct::Gone => {
            c.finish.card = None;
            c.card_focus = false;
            Out::Draw
        }
    }
}
