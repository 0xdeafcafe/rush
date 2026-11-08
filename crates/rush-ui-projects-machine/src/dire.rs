//! A dire disk with nobody there (Rust-only; Go had no such clean). The
//! cleanup agent deletes nothing you haven't approved, but when free disk
//! falls under the dire level (Settings › Disk and memory's "Clean up on
//! its own when disk is under": 0 the lower of 5G and 2% of the disk), its
//! plan has waited DIRE_AWAY for you and you haven't touched rush for as
//! long, rush itself removes the conservative set: only what's made or
//! fetched again and loses no work. Done sessions' scratch untouched for
//! days, build caches (rush_fleet::CACHE_NAMES) in worktrees nobody works
//! in and none would lose work, then pnpm store prune; never a worktree, a
//! stopped session's scratch, an orphan, or anything changed since the
//! plan. Biggest first, only until free space is back over the dire level
//! and DIRE_MARGIN. With a plan ($TMPDIR/cleanup-plan.tsv, care.rs's
//! CARE_PLAN_FILE) only what it marks conservative: yes and rush's own
//! look agrees with; with none yet, what rush knows. Each goes through
//! rush_fleet's checked removals (clean_stale_temp, remove_cache: refused
//! when changed or in use, dry-run aware, logged), and the cleanup session
//! is told what went, its decisions still open for the rest.

use rush_core::agent::Agent;
use rush_core::time::Ms;

use super::care::{CARE_PLAN_FILE, CARE_PLUGIN, Care};
use super::data::*;
use crate::frame::Out;
use crate::model::Model;
use crate::style::disk;

/// How long a plan waits on you, and you're away from rush, before rush
/// cleans the conservative set itself.
pub const DIRE_AWAY: Ms = 30 * 60_000;
/// How long a done session's scratch sits untouched before it's in the set.
pub const DIRE_IDLE: Ms = rush_fleet::ORPHAN_IDLE;
/// How far over the dire level rush frees.
pub const DIRE_MARGIN: u64 = 2 << 30;
/// The dire level unless set: the lower of this and 2% of the disk.
const DIRE_DEFAULT: u64 = 5 << 30;

/// The free disk under which it's dire: gb as set (0 the default, below 0
/// never: 0 here).
pub fn dire_level(total: u64, gb: i64) -> u64 {
    match gb {
        g if g < 0 => 0,
        0 => DIRE_DEFAULT.min(total / 50),
        g => (g as u64) << 30,
    }
}

/// Whether rush may clean on its own: the disk under the dire level, you
/// away from rush for DIRE_AWAY (last_input) and a plan, if there is one
/// (plan_since, 0 when none), unanswered as long.
pub fn dire_due(free: u64, level: u64, plan_since: Ms, last_input: Ms, now: Ms) -> bool {
    level > 0
        && free < level
        && now - last_input >= DIRE_AWAY
        && (plan_since == 0 || now - plan_since >= DIRE_AWAY)
}

/// What the conservative set may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DireKind {
    DoneScratch,
    BuildCache,
    PnpmPrune,
}

/// One thing the conservative clean removes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DireItem {
    pub kind: DireKind,
    pub path: String,
    pub size: i64,
}

/// A rush session's scratch, as rush knows it.
#[derive(Clone, Debug, Default)]
pub struct DireSession {
    pub agent: Agent,
    pub path: String,
    pub size: i64,
    pub done: bool,
    pub running: bool,
    /// How long since it was last active.
    pub idle: Ms,
}

/// A worktree's caches, as measured: path, size, newest change.
#[derive(Clone, Debug, Default)]
pub struct DireWorktree {
    pub path: String,
    /// An agent works in it.
    pub busy: bool,
    pub would_lose: bool,
    pub caches: Vec<(String, i64, Ms)>,
}

/// A line of the plan's file: a path, and whether it's marked conservative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanLine {
    pub path: String,
    pub conservative: bool,
}

/// The plan's file read: path, bytes, conservative, why, tab separated.
/// Lines that aren't are left out.
pub fn read_plan(text: &str) -> Vec<PlanLine> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let path = f.first()?.trim();
            let c = f.get(2)?.trim().to_ascii_lowercase();
            if path.is_empty() {
                return None;
            }
            Some(PlanLine {
                path: path.trim_end_matches('/').to_string(),
                conservative: c == "yes" || c == "conservative: yes",
            })
        })
        .collect()
}

/// The conservative set: what rush's own look says is made again and
/// loses no work, unchanged at or after not_since; with a plan, only what
/// it also marks conservative.
pub fn conservative_set(
    sessions: &[DireSession],
    worktrees: &[DireWorktree],
    plan: Option<&[PlanLine]>,
    not_since: Ms,
) -> Vec<DireItem> {
    let mut out = Vec::new();
    for s in sessions {
        if s.done && !s.running && s.idle >= DIRE_IDLE && s.size > 0 && !s.path.is_empty() {
            out.push(DireItem {
                kind: DireKind::DoneScratch,
                path: s.path.clone(),
                size: s.size,
            });
        }
    }
    for w in worktrees.iter().filter(|w| !w.busy && !w.would_lose) {
        for (p, size, newest) in &w.caches {
            if *newest < not_since && *size > 0 {
                out.push(DireItem {
                    kind: DireKind::BuildCache,
                    path: p.clone(),
                    size: *size,
                });
            }
        }
    }
    let Some(plan) = plan else {
        return out;
    };
    out.retain(|it| {
        plan.iter()
            .any(|l| l.conservative && l.path == it.path.trim_end_matches('/'))
    });
    out
}

/// What goes of the set: biggest first, until need bytes are freed, then
/// pnpm store prune when a node_modules went.
pub fn pick_dire(mut set: Vec<DireItem>, need: i64) -> Vec<DireItem> {
    set.sort_by(|a, b| b.size.cmp(&a.size).then(a.path.cmp(&b.path)));
    let (mut out, mut freed) = (Vec::new(), 0);
    for it in set {
        if freed >= need {
            break;
        }
        freed += it.size;
        out.push(it);
    }
    if out.iter().any(|i| i.path.ends_with("/node_modules")) {
        out.push(DireItem {
            kind: DireKind::PnpmPrune,
            path: "pnpm store prune".into(),
            size: 0,
        });
    }
    out
}

/// The whole rule: nothing unless dire_due; else the set picked until
/// free is back over the level and the margin.
#[allow(clippy::too_many_arguments)]
pub fn plan_dire(
    free: u64,
    level: u64,
    plan_since: Ms,
    last_input: Ms,
    now: Ms,
    sessions: &[DireSession],
    worktrees: &[DireWorktree],
    plan: Option<&[PlanLine]>,
) -> Vec<DireItem> {
    if !dire_due(free, level, plan_since, last_input, now) {
        return Vec::new();
    }
    // Nothing changed since the plan; with none, for DIRE_AWAY.
    let not_since = if plan_since > 0 {
        plan_since
    } else {
        now - DIRE_AWAY
    };
    let need = (level + DIRE_MARGIN).saturating_sub(free) as i64;
    pick_dire(conservative_set(sessions, worktrees, plan, not_since), need)
}

/// How often rush tries a dire clean at most.
const DIRE_AGAIN: Ms = 30 * 60_000;

/// When rush last tried one, kept between frames.
#[derive(Default)]
pub struct Dire {
    pub at: Ms,
    pub running: bool,
}

/// The cleanup agent waiting on its plan, and since when: rush's own,
/// blocked on you.
fn waiting_care(m: &Model) -> Option<Agent> {
    m.agents
        .iter()
        .find(|a| {
            a.rush
                && a.started_by == CARE_PLUGIN
                && !a.done
                && a.job.state == rush_core::agent::State::Blocked
        })
        .cloned()
}

/// Looks at the disk as last read (free, total) and, when it's dire and
/// you're away, cleans the conservative set off the UI. The rush that
/// leads only (watch_machine's caller).
pub fn watch_dire(m: &mut Model, free: u64, total: u64) {
    let level = dire_level(total, data(m).cfg.cleanup_dire_gb);
    let now = m.now;
    let d = m.kept::<Dire>();
    if d.running || (d.at > 0 && now - d.at < DIRE_AGAIN) || level == 0 || free >= level {
        return;
    }
    let care = waiting_care(m);
    let plan_since = care.as_ref().map_or(0, |a| a.job.updated_at.max(1));
    let started = m.kept::<Care>().started;
    let last_input = m.last_input.max(started);
    if !dire_due(free, level, plan_since, last_input, now) {
        return;
    }
    let st = data(m);
    let sessions: Vec<DireSession> = agents(m)
        .filter(|g| g.a.rush && !g.a.job.id.is_empty())
        .map(|g| DireSession {
            agent: g.a.clone(),
            path: rush_fleet::temp_dir(&g.a.job.id),
            size: st.temp_of(&g.a.key),
            done: m.group_of.get(&g.a.key).map(String::as_str) == Some(crate::list::FINISHED),
            running: g.a.pid != 0 || g.busy(),
            idle: untouched(st, &g, now),
        })
        .collect();
    let wts: Vec<DireWorktree> = st
        .wts
        .iter()
        .map(|w| DireWorktree {
            path: w.path.clone(),
            busy: running(m, &w.agents).is_some(),
            would_lose: w.verdict().kind == rush_fleet::VerdictKind::WouldLose || !w.err.is_empty(),
            caches: Vec::new(),
        })
        .collect();
    let d = m.kept::<Dire>();
    d.running = true;
    d.at = now;
    m.later(
        move || {
            dire_clean(
                free, level, plan_since, last_input, now, sessions, wts, care,
            )
        },
        |m, r| {
            m.kept::<Dire>().running = false;
            on_dire(m, r)
        },
    );
}

/// What a dire clean did: what went and how much it freed, what stayed and
/// why, and the cleanup session to tell.
pub struct DireDone {
    pub removed: Vec<(String, i64)>,
    pub kept: Vec<String>,
    pub care: Option<Agent>,
}

/// Measures the caches, reads the plan, picks the set and removes it, each
/// through rush_fleet's checked removals. Off the UI.
#[allow(clippy::too_many_arguments)]
fn dire_clean(
    free: u64,
    level: u64,
    plan_since: Ms,
    last_input: Ms,
    now: Ms,
    sessions: Vec<DireSession>,
    mut wts: Vec<DireWorktree>,
    care: Option<Agent>,
) -> DireDone {
    for w in wts.iter_mut().filter(|w| !w.busy && !w.would_lose) {
        w.caches = rush_fleet::caches_in(&w.path)
            .into_iter()
            .map(|p| {
                let (size, newest) = rush_fleet::walk(&p);
                (p, size, newest)
            })
            .collect();
    }
    let plan = care.as_ref().and_then(|a| {
        let p = std::path::Path::new(&rush_fleet::temp_dir(&a.job.id)).join(CARE_PLAN_FILE);
        std::fs::read_to_string(p).ok().map(|t| read_plan(&t))
    });
    let picked = plan_dire(
        free,
        level,
        plan_since,
        last_input,
        now,
        &sessions,
        &wts,
        plan.as_deref(),
    );
    let not_since = if plan_since > 0 {
        plan_since
    } else {
        now - DIRE_AWAY
    };
    let free_wts: Vec<String> = wts
        .iter()
        .filter(|w| !w.busy && !w.would_lose)
        .map(|w| w.path.clone())
        .collect();
    let why = format!(
        "the disk is dire ({} free, under {}) and nobody answered the cleanup's plan",
        disk(free as i64),
        disk(level as i64)
    );
    let mut done = DireDone {
        removed: Vec::new(),
        kept: Vec::new(),
        care,
    };
    for it in picked {
        let r = match it.kind {
            DireKind::DoneScratch => match sessions.iter().find(|s| s.path == it.path) {
                Some(s) => {
                    rush_fleet::clean_stale_temp(&s.agent, DIRE_IDLE, now).and_then(|went| {
                        if went {
                            Ok(it.size)
                        } else {
                            Err(format!("{}: changed lately or in use; it stays", it.path))
                        }
                    })
                }
                None => Err(format!("{}: no session's", it.path)),
            },
            DireKind::BuildCache => {
                rush_fleet::remove_cache(&it.path, &free_wts, not_since, &why, done.care.as_ref())
            }
            DireKind::PnpmPrune => pnpm_prune(&why).map(|_| 0),
        };
        match r {
            Ok(n) => done.removed.push((it.path, n)),
            Err(e) => done.kept.push(e),
        }
    }
    let mut f = serde_json::Map::new();
    f.insert("why".into(), why.into());
    f.insert(
        "removed".into(),
        done.removed
            .iter()
            .map(|(p, _)| p.clone())
            .collect::<Vec<_>>()
            .into(),
    );
    f.insert(
        "bytes".into(),
        done.removed.iter().map(|r| r.1).sum::<i64>().into(),
    );
    if let Some(a) = &done.care {
        f.insert("session".into(), a.job.id.clone().into());
    }
    rush_events::log("dire-clean", f);
    done
}

/// pnpm store prune, when pnpm is there.
fn pnpm_prune(why: &str) -> Result<(), String> {
    if rush_core::dryrun::would("pnpm-store-prune", "pnpm store prune", why) {
        return Ok(());
    }
    let out = std::process::Command::new("pnpm")
        .args(["store", "prune"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("pnpm store prune: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pnpm store prune: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Says what a dire clean did: the news, a notification, and the cleanup
/// session told (queued), its plan's decisions still open for the rest.
fn on_dire(m: &mut Model, r: DireDone) -> Out {
    if r.removed.is_empty() {
        return Out::Idle;
    }
    let freed: i64 = r.removed.iter().map(|x| x.1).sum();
    let said = dire_words(&r.removed, freed);
    m.news(
        &format!("{said} · the rest of the cleanup's plan waits for you"),
        "",
    );
    m.flash(&said);
    if let Some(a) = r.care {
        let text = format!(
            "rush's note, not the user's answer: the disk was dire and nobody answered your plan for 30 minutes, so rush itself removed only the conservative part:\n{}\n{}\nThe rest of your plan still waits for the user's approval: delete nothing, and end this turn asking the same decisions again for what's left, with those paths taken out.",
            r.removed
                .iter()
                .map(|(p, n)| format!("- {p} · {}", disk(*n)))
                .collect::<Vec<_>>()
                .join("\n"),
            if r.kept.is_empty() {
                String::new()
            } else {
                format!("Kept: {}", r.kept.join("; "))
            }
        );
        let h = crate::host::hosts(m);
        let id = a.job.id.clone();
        m.later(
            move || h.send_to(&id, &text, false),
            |m, r| {
                if let Err(e) = r {
                    m.fail(&e);
                }
                Out::Draw
            },
        );
    }
    Out::Draw
}

/// What went, in a few words.
pub fn dire_words(removed: &[(String, i64)], freed: i64) -> String {
    format!(
        "disk was dire: rush freed {} itself ({} thing{}, only what's made again)",
        disk(freed),
        removed.len(),
        if removed.len() == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: u64 = 1 << 30;
    const MIN: Ms = 60_000;
    const DAY: Ms = 24 * 60 * MIN;
    const NOW: Ms = 1_793_190_896_789;

    #[test]
    fn levels() {
        for (total, gb, want) in [
            (1000 * G, 0, 5 * G),
            (100 * G, 0, 2 * G),
            (1000 * G, -1, 0),
            (1000 * G, 3, 3 * G),
        ] {
            assert_eq!(dire_level(total, gb), want, "dire_level({total}, {gb})");
        }
    }

    #[test]
    fn due_only_dire_and_away() {
        let away = NOW - 31 * MIN;
        let here = NOW - 5 * MIN;
        for (free, plan_since, last_input, want, why) in [
            (G, 0, away, true, "dire, away, no plan yet"),
            (G, away, away, true, "dire, away, the plan unanswered 31m"),
            (G, here, away, false, "the plan only 5m old"),
            (G, away, here, false, "you were here 5m ago"),
            (G, 0, here, false, "you're here"),
            (6 * G, away, away, false, "not dire"),
            (6 * G, 0, 0, false, "not dire, never touched"),
        ] {
            assert_eq!(
                dire_due(free, 5 * G, plan_since, last_input, NOW),
                want,
                "{why}"
            );
        }
        assert!(!dire_due(0, 0, 0, 0, NOW), "off: never");
    }

    fn ses(path: &str, gb: i64, done: bool, running: bool, idle: Ms) -> DireSession {
        DireSession {
            path: path.into(),
            size: gb << 30,
            done,
            running,
            idle,
            ..DireSession::default()
        }
    }

    fn wt(path: &str, busy: bool, would_lose: bool, caches: &[(&str, i64, Ms)]) -> DireWorktree {
        DireWorktree {
            path: path.into(),
            busy,
            would_lose,
            caches: caches
                .iter()
                .map(|(n, gb, at)| (format!("{path}/{n}"), gb << 30, *at))
                .collect(),
        }
    }

    // What's in the conservative set, and what of it goes: only with the
    // disk dire and you away; done sessions' scratch untouched for days
    // and caches unchanged since the plan in worktrees nobody works in and
    // none would lose; biggest first, until there's room; pnpm store prune
    // after a node_modules; a plan narrows it to what it marks
    // conservative. Never a stopped or running session's scratch.
    #[test]
    fn picks_the_conservative_set() {
        let plan_at = NOW - 40 * MIN;
        let old = plan_at - DAY;
        let sessions = [
            ses("/s/done-old", 3, true, false, 4 * DAY),
            ses("/s/done-fresh", 9, true, false, DAY),
            ses("/s/stopped", 9, false, false, 10 * DAY),
            ses("/s/done-running", 9, true, true, 10 * DAY),
        ];
        let wts = [
            wt(
                "/w/free",
                false,
                false,
                &[
                    ("target", 4, old),
                    ("node_modules", 1, old),
                    ("dist", 9, plan_at + MIN),
                ],
            ),
            wt("/w/busy", true, false, &[("target", 9, old)]),
            wt("/w/lose", false, true, &[("target", 9, old)]),
        ];
        let away = NOW - 31 * MIN;
        let plan = [
            PlanLine {
                path: "/w/free/node_modules".into(),
                conservative: true,
            },
            PlanLine {
                path: "/w/free/target".into(),
                conservative: false,
            },
            PlanLine {
                path: "/s/stopped".into(),
                conservative: true,
            },
        ];
        // (free, last input, plan, what goes, why)
        type Case<'a> = (u64, Ms, Option<&'a [PlanLine]>, Vec<&'a str>, &'a str);
        let cases: Vec<Case> = vec![
            (6 * G, away, None, vec![], "not dire"),
            (G, NOW - MIN, None, vec![], "dire, but you're here"),
            (
                5 * G - (1 << 20),
                away,
                None,
                vec!["/w/free/target"],
                "a little short: the biggest only",
            ),
            (
                G,
                away,
                None,
                vec!["/w/free/target", "/s/done-old"],
                "far short: biggest first until there's room",
            ),
            (
                G,
                away,
                Some(&plan[..]),
                vec!["/w/free/node_modules", "pnpm store prune"],
                "only what the plan marks conservative and rush agrees with",
            ),
        ];
        for (free, last_input, plan, want, why) in cases {
            let got = plan_dire(free, 5 * G, plan_at, last_input, NOW, &sessions, &wts, plan);
            let got: Vec<String> = got.into_iter().map(|i| i.path).collect();
            assert_eq!(got, want, "{why}");
        }
        // The whole set, as rush's own look has it.
        let all: Vec<String> = conservative_set(&sessions, &wts, None, plan_at)
            .into_iter()
            .map(|i| i.path)
            .collect();
        assert_eq!(
            all,
            ["/s/done-old", "/w/free/target", "/w/free/node_modules"]
        );
    }

    #[test]
    fn reads_the_plan() {
        let got = read_plan(
            "/a/target\t4000\tyes\ta cache\n/b/\t1\tconservative: yes\tx\n/c\t1\tno\tworktree\nnot a line\n",
        );
        let want = [("/a/target", true), ("/b", true), ("/c", false)].map(|(p, c)| PlanLine {
            path: p.into(),
            conservative: c,
        });
        assert_eq!(got, want);
    }
}
