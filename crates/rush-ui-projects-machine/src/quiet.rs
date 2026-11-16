//! The quiet clean (Rust-only; Go had none): once a day, whatever the free
//! disk, while you're away from rush for QUIET_AWAY, the rush that leads
//! removes what's made again or loses nothing, so space doesn't pile up
//! until the disk is dire (colima's VM grew to 75G, two tools' logs to 18G,
//! before anyone looked). Nothing is asked; each removal is logged, and the
//! news says so only when it freed QUIET_SAID or more.
//!
//! What goes (quiet_set), each through rush_fleet's checked removals,
//! which look again first:
//! - done sessions' scratch untouched for DIRE_IDLE, and stopped ones'
//!   (not done, host gone) untouched for QUIET_IDLE (clean_stale_temp);
//! - rush's scratch no session owns, untouched for ORPHAN_IDLE, that no
//!   running session's id starts (remove_temp_orphan);
//! - the build caches of worktrees nobody works in, never a main checkout,
//!   once the worktree outside node_modules and the cache itself sat
//!   untouched for QUIET_IDLE and no process works there (remove_cache): a
//!   session back in one runs its install again;
//! - what a cleanup rule that waits says goes now (logs, Nx workspaces,
//!   Prisma's downloads, rules you made); never one that always goes
//!   (vitest, go-build: those cost a rebuild while in use);
//! - then pnpm store prune, docker's dangling images and its build cache
//!   older than a week when docker answers, and colima's trim. Never docker's
//!   volumes or unused images: those wait for Clean up.
//!
//! Off with Settings' "Clean up done work after" off (cleanup_after 0),
//! and never while a dire clean runs.

use rush_core::time::Ms;
use rush_fleet::How;

use super::data::*;
use super::dire::{DIRE_IDLE, DireSession, DireWorktree};
use crate::frame::Out;
use crate::model::Model;
use crate::style::disk;

const DAY: Ms = 24 * 3_600_000;
/// How often the quiet clean runs at most.
pub const QUIET_EVERY: Ms = DAY;
/// How long you've been away from rush before it runs.
pub const QUIET_AWAY: Ms = 10 * 60_000;
/// How long a stopped session's scratch, or an idle worktree's build
/// caches, sit untouched before they go.
pub const QUIET_IDLE: Ms = 14 * DAY;
/// From this much freed, the news says so.
pub const QUIET_SAID: i64 = 1 << 30;

/// What the quiet clean removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuietKind {
    DoneScratch,
    StoppedScratch,
    Orphan,
    BuildCache,
    RuleCache,
    Command,
}

/// One thing it removes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuietItem {
    pub kind: QuietKind,
    pub path: String,
    pub size: i64,
    pub how: How,
}

impl QuietItem {
    fn new(kind: QuietKind, path: &str, size: i64) -> QuietItem {
        QuietItem {
            kind,
            path: path.into(),
            size,
            how: How::Remove,
        }
    }
}

/// Whether the quiet clean is due: on, none for QUIET_EVERY, you away for
/// QUIET_AWAY.
pub fn quiet_due(on: bool, last: Ms, last_input: Ms, now: Ms) -> bool {
    on && now - last >= QUIET_EVERY && now - last_input >= QUIET_AWAY
}

/// What goes, by what was measured: sessions' scratch by how long each sat,
/// the orphans that are idle and no running session uses (orphans: path,
/// size, idle, used), the build caches of worktrees nobody works in that
/// sat QUIET_IDLE, the rules' due caches (rules), then ends.
pub fn quiet_set(
    sessions: &[DireSession],
    worktrees: &[DireWorktree],
    orphans: &[(String, i64, bool, bool)],
    rules: Vec<QuietItem>,
    ends: Vec<QuietItem>,
    now: Ms,
) -> Vec<QuietItem> {
    let mut out = Vec::new();
    for s in sessions
        .iter()
        .filter(|s| !s.running && s.size > 0 && !s.path.is_empty())
    {
        if s.done && s.idle >= DIRE_IDLE {
            out.push(QuietItem::new(QuietKind::DoneScratch, &s.path, s.size));
        } else if !s.done && s.idle >= QUIET_IDLE {
            out.push(QuietItem::new(QuietKind::StoppedScratch, &s.path, s.size));
        }
    }
    for (p, size, idle, used) in orphans {
        if *idle && !*used && *size > 0 {
            out.push(QuietItem::new(QuietKind::Orphan, p, *size));
        }
    }
    for w in worktrees.iter().filter(|w| !w.busy && !w.touched) {
        for (p, size, newest) in &w.caches {
            if *size > 0 && now - *newest >= QUIET_IDLE {
                out.push(QuietItem::new(QuietKind::BuildCache, p, *size));
            }
        }
    }
    for r in rules {
        if !out.iter().any(|o| o.path == r.path) {
            out.push(r);
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size).then(a.path.cmp(&b.path)));
    out.extend(ends);
    out
}

/// The commands the quiet clean ends with, of those there are: pnpm's
/// prune, docker's dangling images and old build cache while docker
/// answers, and colima's trim last.
pub fn quiet_ends(
    has: &dyn Fn(&str) -> bool,
    docker_answers: bool,
    colima: bool,
) -> Vec<QuietItem> {
    let cmd = |args: &'static [&'static str]| QuietItem {
        how: How::Run(args),
        ..QuietItem::new(QuietKind::Command, &args.join(" "), 0)
    };
    let mut out = Vec::new();
    if has("pnpm") {
        out.push(cmd(rush_fleet::PNPM_PRUNE));
    }
    if has("docker") && docker_answers {
        out.push(cmd(rush_fleet::DOCKER_IMAGE_PRUNE));
        out.push(cmd(rush_fleet::DOCKER_OLD_BUILD_PRUNE));
    }
    if colima && has("colima") {
        out.push(cmd(rush_fleet::COLIMA_TRIM));
    }
    out
}

/// When the last quiet clean ran, kept across restarts so a restart
/// doesn't run another.
#[derive(Default)]
pub struct Quiet {
    pub at: Ms,
    pub read: bool,
    pub running: bool,
}

fn stamp_path() -> std::path::PathBuf {
    rush_state::cache_path("quiet-clean")
}

fn read_stamp() -> Ms {
    std::fs::read_to_string(stamp_path())
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

/// Runs the quiet clean off the UI when it's due. The rush that leads only
/// (watch_machine's caller, beside watch_dire).
pub fn watch_quiet(m: &mut Model) {
    let now = m.now;
    let on = data(m).cfg.cleanup_after() > 0;
    let dire = m.kept::<super::dire::Dire>().running;
    let q = m.kept::<Quiet>();
    if !q.read {
        q.read = true;
        q.at = read_stamp();
    }
    if q.running || dire {
        return;
    }
    let last = q.at;
    let started = m.kept::<super::care::Care>().started;
    let last_input = m.last_input.max(started);
    if !quiet_due(on, last, last_input, now) {
        return;
    }
    let st = data(m);
    let sessions: Vec<DireSession> = agents(m)
        .filter(|g| g.a.rush && !g.a.job.id.is_empty())
        .map(|g| DireSession {
            agent: g.a.clone(),
            path: rush_fleet::temp_dir(&g.a.job.id),
            size: st.temp_of(&g.a.key).max(1),
            done: m.group_of.get(&g.a.key).map(String::as_str) == Some(crate::list::FINISHED),
            running: g.a.pid != 0 || g.busy(),
            idle: untouched(st, &g, now),
        })
        .collect();
    let wts: Vec<DireWorktree> = st
        .wts
        .iter()
        .filter(|w| w.path != w.repo && w.err.is_empty() && w.in_use.is_empty())
        .map(|w| DireWorktree {
            path: w.path.clone(),
            busy: running(m, &w.agents).is_some(),
            would_lose: false,
            touched: false,
            caches: Vec::new(),
        })
        .collect();
    let live = running_ids(m);
    let q = m.kept::<Quiet>();
    q.running = true;
    q.at = now;
    m.later(
        move || {
            let _ = rush_state::write_bytes(&stamp_path(), now.to_string().as_bytes());
            quiet_clean(now, sessions, wts, live)
        },
        |m, r| {
            m.kept::<Quiet>().running = false;
            on_quiet(m, r)
        },
    );
}

/// What a quiet clean did.
pub struct QuietDone {
    pub removed: Vec<(QuietKind, String, i64)>,
    pub before: u64,
    pub after: u64,
}

/// Measures, picks and removes, each through rush_fleet's checked
/// removals. Off the UI.
fn quiet_clean(
    now: Ms,
    sessions: Vec<DireSession>,
    mut wts: Vec<DireWorktree>,
    live: Vec<String>,
) -> QuietDone {
    let before = rush_sys::sysinfo::read_disk().free;
    let cwds = rush_fleet::process_cwds();
    for w in wts.iter_mut().filter(|w| !w.busy) {
        let f = rush_fleet::wt_facts(std::slice::from_ref(&w.path), &cwds)
            .remove(&w.path)
            .unwrap_or_default();
        w.touched = f.works_here || now - f.newest_outside < QUIET_IDLE;
        if w.touched {
            continue;
        }
        w.caches = rush_fleet::caches_in(&w.path)
            .into_iter()
            .map(|p| {
                let (size, newest) = rush_fleet::walk(&p);
                (p, size, newest)
            })
            .collect();
    }
    let orphans = rush_fleet::find_temp_orphans();
    let orphan_rows: Vec<(String, i64, bool, bool)> = orphans
        .iter()
        .map(|o| {
            (
                o.path.clone(),
                o.size,
                o.idle(now),
                o.used_by(&live).is_some(),
            )
        })
        .collect();
    let home = std::env::var("HOME").unwrap_or_default();
    let rules = rush_fleet::cache_rules();
    // Only rules that wait: one that always goes costs a rebuild in use.
    let rule_items: Vec<QuietItem> = rush_fleet::measure_caches(&rules, now)
        .into_iter()
        .filter(|c| {
            c.ok && c.how == How::Remove
                && rush_fleet::rule_for(&c.path, &rules, &home).is_some_and(|r| r.idle > 0)
        })
        .map(|c| QuietItem::new(QuietKind::RuleCache, &c.path, c.size))
        .collect();
    let has = |t: &str| rush_fleet::find_tool(t).is_some();
    let docker = has("docker") && rush_fleet::docker_reclaim().is_some();
    let ends = quiet_ends(&has, docker, rush_fleet::colima(&home));
    let picked = quiet_set(&sessions, &wts, &orphan_rows, rule_items, ends, now);
    let free_wts: Vec<String> = wts
        .iter()
        .filter(|w| !w.busy && !w.touched)
        .map(|w| w.path.clone())
        .collect();
    let why = "the quiet clean: made again or losing nothing, untouched for long".to_string();
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for it in picked {
        let r = match it.kind {
            QuietKind::DoneScratch | QuietKind::StoppedScratch => {
                let idle = if it.kind == QuietKind::DoneScratch {
                    DIRE_IDLE
                } else {
                    QUIET_IDLE
                };
                match sessions.iter().find(|s| s.path == it.path) {
                    Some(s) => rush_fleet::clean_stale_temp(&s.agent, idle, now).and_then(|went| {
                        if went {
                            Ok(it.size)
                        } else {
                            Err(format!("{}: changed lately or in use", it.path))
                        }
                    }),
                    None => Err(format!("{}: no session's", it.path)),
                }
            }
            QuietKind::Orphan => match orphans.iter().find(|o| o.path == it.path) {
                Some(o) => rush_fleet::remove_temp_orphan(o, &live),
                None => Err(format!("{}: gone", it.path)),
            },
            QuietKind::BuildCache => {
                rush_fleet::remove_cache(&it.path, &free_wts, now - QUIET_IDLE, &why, None)
            }
            QuietKind::RuleCache => rush_fleet::remove_rule_cache(&it.path, &rules, &why),
            QuietKind::Command => match it.how {
                How::Run(args) => rush_fleet::run_command(args, &why).map(|_| 0),
                _ => Err(format!("{}: not a command", it.path)),
            },
        };
        match r {
            Ok(n) => removed.push((it.kind, it.path, n)),
            Err(e) => kept.push(e),
        }
    }
    let after = rush_sys::sysinfo::read_disk().free;
    let mut f = serde_json::Map::new();
    f.insert("why".into(), why.into());
    f.insert(
        "removed".into(),
        removed
            .iter()
            .map(|r| r.1.clone())
            .collect::<Vec<_>>()
            .into(),
    );
    f.insert("kept".into(), kept.into());
    f.insert("freeBefore".into(), before.into());
    f.insert("freeAfter".into(), after.into());
    rush_events::log("quiet-clean", f);
    QuietDone {
        removed,
        before,
        after,
    }
}

/// Says what a quiet clean did, in the news, only when it freed
/// QUIET_SAID or more as df measures it.
fn on_quiet(m: &mut Model, r: QuietDone) -> Out {
    let freed = (r.after as i64 - r.before as i64).max(0);
    if freed < QUIET_SAID {
        return Out::Idle;
    }
    m.news(&quiet_words(&r.removed, freed), "");
    Out::Draw
}

/// What went, in a few words: what df shows freed, and of what.
pub fn quiet_words(removed: &[(QuietKind, String, i64)], freed: i64) -> String {
    let count = |k: QuietKind| removed.iter().filter(|r| r.0 == k).count();
    let mut parts = Vec::new();
    for (k, one, many) in [
        (
            QuietKind::BuildCache,
            "an idle worktree's build cache",
            "idle worktrees' build caches",
        ),
        (
            QuietKind::StoppedScratch,
            "a stopped session's scratch",
            "stopped sessions' scratch",
        ),
        (
            QuietKind::DoneScratch,
            "a done session's scratch",
            "done sessions' scratch",
        ),
        (
            QuietKind::Orphan,
            "scratch no session owns",
            "scratch no session owns",
        ),
        (
            QuietKind::RuleCache,
            "a cache a rule lets go",
            "caches rules let go",
        ),
    ] {
        match count(k) {
            0 => {}
            1 => parts.push(one.to_string()),
            n => parts.push(format!("{n} {many}")),
        }
    }
    let cmds: Vec<&str> = removed
        .iter()
        .filter(|r| r.0 == QuietKind::Command)
        .map(|r| r.1.as_str())
        .collect();
    if !cmds.is_empty() {
        parts.push(format!("ran {}", cmds.join(", ")));
    }
    format!(
        "rush tidied in the background: freed {} as df measures it · {}",
        disk(freed),
        parts.join(" · ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Ms = 1_793_190_896_789;
    const G: i64 = 1 << 30;

    #[test]
    fn due_once_a_day_while_away() {
        for (on, last, input, want, why) in [
            (
                true,
                NOW - 2 * DAY,
                NOW - 20 * 60_000,
                true,
                "a day since, away",
            ),
            (true, NOW - 2 * DAY, NOW - 60_000, false, "you're here"),
            (true, NOW - 3_600_000, NOW - DAY, false, "ran an hour ago"),
            (false, 0, 0, false, "off"),
            (true, 0, NOW - DAY, true, "never ran"),
        ] {
            assert_eq!(quiet_due(on, last, input, NOW), want, "{why}");
        }
    }

    fn sess(path: &str, done: bool, running: bool, idle_days: i64) -> DireSession {
        DireSession {
            path: path.into(),
            size: G,
            done,
            running,
            idle: idle_days * DAY,
            ..DireSession::default()
        }
    }

    // What goes: done scratch after 3 days, stopped after 14, never a
    // running session's; idle orphans no running session uses; idle
    // worktrees' caches after 14 days; the rules' due caches; the
    // commands last.
    #[test]
    fn picks_what_loses_nothing() {
        let sessions = [
            sess("/s/done-old", true, false, 4),
            sess("/s/done-new", true, false, 1),
            sess("/s/stopped-old", false, false, 15),
            sess("/s/stopped-new", false, false, 10),
            sess("/s/running", false, true, 30),
        ];
        let wts = [
            DireWorktree {
                path: "/w/idle".into(),
                caches: vec![
                    ("/w/idle/node_modules".into(), 3 * G, NOW - 20 * DAY),
                    ("/w/idle/.next".into(), G, NOW - DAY),
                ],
                ..DireWorktree::default()
            },
            DireWorktree {
                path: "/w/touched".into(),
                touched: true,
                caches: vec![("/w/touched/node_modules".into(), 3 * G, NOW - 20 * DAY)],
                ..DireWorktree::default()
            },
            DireWorktree {
                path: "/w/busy".into(),
                busy: true,
                caches: vec![("/w/busy/node_modules".into(), 3 * G, NOW - 20 * DAY)],
                ..DireWorktree::default()
            },
        ];
        let orphans = [
            ("/o/idle".to_string(), G / 2, true, false),
            ("/o/used".to_string(), G, true, true),
            ("/o/new".to_string(), G, false, false),
        ];
        let rules = vec![QuietItem::new(QuietKind::RuleCache, "/r/logs", 9 * G)];
        let has = |_: &str| true;
        let ends = quiet_ends(&has, true, true);
        let got: Vec<String> = quiet_set(&sessions, &wts, &orphans, rules, ends, NOW)
            .into_iter()
            .map(|i| i.path)
            .collect();
        assert_eq!(
            got,
            [
                "/r/logs",
                "/w/idle/node_modules",
                "/s/done-old",
                "/s/stopped-old",
                "/o/idle",
                "pnpm store prune",
                "docker image prune -f",
                "docker builder prune -f --filter until=168h",
                "colima ssh -- sudo fstrim -a",
            ]
        );
    }

    // Docker that doesn't answer, or isn't there, and no colima: only
    // what's there runs.
    #[test]
    fn ends_with_what_there_is() {
        let names = |v: Vec<QuietItem>| v.into_iter().map(|i| i.path).collect::<Vec<_>>();
        let pnpm = |t: &str| t == "pnpm";
        assert_eq!(names(quiet_ends(&pnpm, true, false)), ["pnpm store prune"]);
        let all = |_: &str| true;
        assert_eq!(
            names(quiet_ends(&all, false, true)),
            ["pnpm store prune", "colima ssh -- sudo fstrim -a"],
            "docker hung: none of its commands"
        );
        assert!(
            !names(quiet_ends(&all, true, true))
                .iter()
                .any(|p| p.contains("volume") || p.contains("-a -f")),
            "never volumes, never unused images"
        );
    }

    #[test]
    fn says_what_went() {
        let r = vec![
            (
                QuietKind::BuildCache,
                "/w/a/node_modules".to_string(),
                3 * G,
            ),
            (
                QuietKind::BuildCache,
                "/w/b/node_modules".to_string(),
                3 * G,
            ),
            (QuietKind::RuleCache, "/r/logs".to_string(), 9 * G),
            (QuietKind::Command, "pnpm store prune".to_string(), 0),
        ];
        assert_eq!(
            quiet_words(&r, 14 * G),
            "rush tidied in the background: freed 14.0G as df measures it · 2 idle worktrees' build caches · a cache a rule lets go · ran pnpm store prune"
        );
    }
}
