//! What the rules say to do with the candidates at a level, and why the
//! rest a rule would act on stay: no disk, no clock, no processes. Ported
//! from rush-bundled's clean/disk.rs plan as of 239f592a (a working
//! session's target is pruned, never removed whole), reap.rs refusals
//! (worktrees), and this crate's rules.rs decide (cache folders).
//!
//! Only what a rule would act on is said to stay: something not idle long
//! enough yet is neither done nor kept. Of several enabled rules of one
//! kind, the first decides. Commands run last, after every removal, as
//! kache's store was emptied after the targets.

use rush_core::time::Ms;

use super::cargo::{old_artifacts, superseded_artifacts};
use super::{
    Act, CACHE_DIR, CARGO_TARGET, Candidate, How, InUse, Kept, Kind, Level, Params, Rule,
    TOOL_COMMAND, WORKTREE,
};
use crate::rules::decide;
use crate::sweep::span;

fn ms(secs: u64) -> Ms {
    (secs as Ms).saturating_mul(1000)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The first enabled rule of a kind that runs at lv, with its numbers.
fn first(rules: &[Rule], lv: Level, is: impl Fn(&Kind) -> bool) -> Option<(&Rule, &Params)> {
    rules
        .iter()
        .filter(|r| is(&r.kind))
        .find_map(|r| r.at(lv).map(|p| (r, p)))
}

fn kept(c: &Candidate, rule: &Rule, why: impl Into<String>) -> Kept {
    Kept {
        path: c.path.clone(),
        rule: rule.id.clone(),
        why: why.into(),
    }
}

fn act(c: &Candidate, rule: &Rule, why: String, how: How) -> Act {
    Act {
        rule: rule.id.clone(),
        path: c.path.clone(),
        kind: c.kind.clone(),
        why,
        bytes: c.bytes_freeable,
        how,
    }
}

/// What the rules do at lv with cands at now: the acts, in the
/// candidates' order and commands last, and what a rule would act on that
/// stays, with why. Pure: whether anything uses a path is as the
/// candidate says (yes keeps it; apply asks again before each removal and
/// goes only on no).
pub fn plan(cands: &[Candidate], rules: &[Rule], lv: Level, now: Ms) -> (Vec<Act>, Vec<Kept>) {
    let (mut acts, mut keep) = (Vec::new(), Vec::new());
    for c in cands {
        match c.kind.as_str() {
            CARGO_TARGET => cargo(c, rules, lv, now, &mut acts, &mut keep),
            WORKTREE => worktree(c, rules, lv, now, &mut acts, &mut keep),
            CACHE_DIR => cache_dir(c, rules, lv, now, &mut acts, &mut keep),
            TOOL_COMMAND => {}
            _ => plugin(c, rules, lv, now, &mut acts, &mut keep),
        }
    }
    for r in rules {
        if let Kind::ToolCommand { command, why } = &r.kind
            && r.at(lv).is_some()
            && !command.is_empty()
        {
            acts.push(Act {
                rule: r.id.clone(),
                path: Default::default(),
                kind: TOOL_COMMAND.into(),
                why: why.clone(),
                bytes: 0,
                how: How::Run {
                    command: command.clone(),
                },
            });
        }
    }
    (acts, keep)
}

/// A cargo target: whole once no build touched it for the target rule's
/// idle and nothing uses it, unless a session is working on it; else its
/// artifacts past the artifact rule's age, and its crates' builds past
/// the newest keep once superseded old.
fn cargo(
    c: &Candidate,
    rules: &[Rule],
    lv: Level,
    now: Ms,
    acts: &mut Vec<Act>,
    keep: &mut Vec<Kept>,
) {
    let f = &c.facts;
    let whole = first(rules, lv, |k| *k == Kind::CargoTarget);
    if let Some((r, p)) = whole
        && !f.working
        && let (Some(idle), Some(last)) = (p.idle_secs, f.last_build)
        && now - last >= ms(idle)
    {
        if f.in_use == InUse::Yes {
            keep.push(kept(c, r, "idle, but in use"));
        } else {
            let why = format!("no build touched it for {}", span(now - last));
            acts.push(act(c, r, why, How::Remove));
        }
        return;
    }
    let parts = first(rules, lv, |k| *k == Kind::CargoArtifacts);
    let mut paths = Vec::new();
    if let Some((_, p)) = parts {
        if let Some(age) = p.age_secs {
            paths.extend(old_artifacts(&f.artifacts, now, ms(age)));
        }
        if let (Some(n), Some(age)) = (p.keep, p.superseded_secs) {
            paths.extend(superseded_artifacts(&f.artifacts, now, n as usize, ms(age)));
        }
    }
    paths.sort();
    paths.dedup();
    let Some((r, _)) = parts.or(whole) else {
        return;
    };
    if paths.is_empty() {
        if f.working {
            keep.push(kept(c, r, "a session is working on it"));
        }
        return;
    }
    if f.in_use == InUse::Yes {
        keep.push(kept(c, r, "has old builds, but a build is using it"));
        return;
    }
    let why = format!(
        "{} older than the rule's age, or superseded",
        plural(paths.len(), "build file", "build files")
    );
    acts.push(act(c, r, why, How::Prune { paths }));
}

/// The reasons an idle worktree stays, none when it can go: reap.rs's
/// refusals, and whether anything has it open.
fn refusals(c: &Candidate) -> Vec<String> {
    let f = &c.facts;
    let mut why = Vec::new();
    if !f.err.is_empty() {
        why.push(f.err.clone());
    }
    if f.locked {
        why.push("locked".into());
    }
    match f.changed {
        None => why.push("git status failed".into()),
        Some(0) => {}
        Some(n) => why.push(plural(n, "uncommitted file", "uncommitted files")),
    }
    match f.held {
        None => why.push("can't tell what runs there".into()),
        Some(0) => {}
        Some(n) => why.push(format!(
            "{} working there",
            plural(n, "process", "processes")
        )),
    }
    if f.merged.is_empty() {
        why.push("its commits aren't all in the default branch".into());
    }
    if f.in_use == InUse::Yes {
        why.push("something has a file open in it, or names it".into());
    }
    if f.repo.is_none() {
        why.push("its main checkout isn't known".into());
    }
    why
}

/// A linked worktree no session is working in, idle for the rule's idle:
/// it goes when refusals has nothing, else it stays with them. The main
/// checkout never goes.
fn worktree(
    c: &Candidate,
    rules: &[Rule],
    lv: Level,
    now: Ms,
    acts: &mut Vec<Act>,
    keep: &mut Vec<Kept>,
) {
    let f = &c.facts;
    let Some((r, p)) = first(rules, lv, |k| *k == Kind::Worktree) else {
        return;
    };
    if f.main || f.working {
        return;
    }
    let Some(idle) = f.idle(now) else {
        return;
    };
    if idle < ms(p.idle_secs.unwrap_or(0)) {
        return;
    }
    let why = refusals(c);
    if !why.is_empty() {
        keep.push(kept(c, r, why.join(", ")));
        return;
    }
    let Some(repo) = f.repo.clone() else {
        return;
    };
    let why = format!(
        "idle {}, {}, clean, nothing runs there",
        span(idle),
        f.merged
    );
    acts.push(act(c, r, why, How::RemoveWorktree { repo }));
}

/// A cache folder: by the rule that found it, once nothing in it changed
/// for its idle.
fn cache_dir(
    c: &Candidate,
    rules: &[Rule],
    lv: Level,
    now: Ms,
    acts: &mut Vec<Act>,
    keep: &mut Vec<Kept>,
) {
    let Some(r) = rules.iter().find(|r| r.id == c.rule) else {
        return;
    };
    let (Kind::CacheDir { why, go_clean, .. }, Some(p)) = (&r.kind, r.at(lv)) else {
        return;
    };
    let idle = ms(p.idle_secs.unwrap_or(0));
    let newest = match c.facts.mtime {
        Some(t) => t,
        None if idle == 0 => 0,
        None => return,
    };
    let (ok, note) = decide(idle, newest, now);
    if !ok {
        return;
    }
    if c.facts.in_use == InUse::Yes {
        keep.push(kept(c, r, "in use"));
        return;
    }
    let why = if note.is_empty() {
        why.clone()
    } else {
        format!("{why}; {note}")
    };
    let how = if *go_clean { How::GoClean } else { How::Remove };
    acts.push(act(c, r, why, how));
}

/// What a plugin reported: by the first plugin-candidate rule of its kind,
/// once idle for the rule's idle, unless a session is working on it.
fn plugin(
    c: &Candidate,
    rules: &[Rule],
    lv: Level,
    now: Ms,
    acts: &mut Vec<Act>,
    keep: &mut Vec<Kept>,
) {
    let Some((r, p)) = first(
        rules,
        lv,
        |k| matches!(k, Kind::PluginCandidate { kind } if *kind == c.kind),
    ) else {
        return;
    };
    let idle = ms(p.idle_secs.unwrap_or(0));
    let ago = c.facts.idle(now);
    if idle > 0 && ago.is_none_or(|a| a < idle) {
        return;
    }
    if c.facts.working {
        keep.push(kept(c, r, "a session is working on it"));
        return;
    }
    if c.facts.in_use == InUse::Yes {
        keep.push(kept(c, r, "in use"));
        return;
    }
    let mut why = if c.why.is_empty() {
        r.title.clone()
    } else {
        c.why.clone()
    };
    if let Some(a) = ago.filter(|_| idle > 0) {
        why = format!("{why}; untouched for {}", span(a));
    }
    acts.push(act(c, r, why, How::Remove));
}
