//! Doing a plan's acts, and a whole run: find, plan, apply, and one
//! disk-clean event saying what. Ported from rush-bundled's clean/disk.rs
//! apply and the auto mode's tick (clean/mod.rs auto, kits.rs rust_auto,
//! reap.rs reap).
//!
//! Every removal asks whether anything uses its path right before, and
//! goes only on no; it goes through sweep::remove_tree (which refuses
//! NEVER_TOUCH), go clean -modcache, or git worktree remove unforced, and
//! nothing else. A dry run (rush_core::dryrun) writes each down instead.
//! A tool command runs only when its program is a known tool
//! (KNOWN_TOOLS); anything else stays, "not a known tool".

use std::path::Path;

use rush_core::time::Ms;
use serde_json::{Value, json};

use super::cargo::{freeable, in_use, is_target};
use super::find::{Scan, find_candidates, remove_worktree_git};
use super::plan::plan;
use super::{Act, CARGO_TARGET, Candidate, Done, How, InUse, Kept, Level, Outcome, Rule, level};
use crate::sweep::{never_touch, remove_tree, run_command};
use crate::temp::agent_event;

/// The programs a tool-command rule may run: their own clean-ups, run by
/// bare name (found on PATH), nothing else. A rule file anyone can write
/// to must not be a way to run anything.
pub const KNOWN_TOOLS: &[&str] = &["kache", "go", "cargo", "pnpm", "npm", "yarn", "bun"];

/// Whether command's program is one of KNOWN_TOOLS, by bare name.
pub fn known_tool(command: &[String]) -> bool {
    command
        .first()
        .is_some_and(|p| KNOWN_TOOLS.contains(&p.as_str()))
}

/// Err unless in_use says no for p, now.
fn unused(p: &Path, in_use: &dyn Fn(&Path) -> InUse) -> Result<(), String> {
    match in_use(p) {
        InUse::No => Ok(()),
        InUse::Yes => Err(format!("{}: in use now; it stays", p.display())),
        InUse::Unknown => Err(format!(
            "{}: can't tell whether anything uses it; it stays",
            p.display()
        )),
    }
}

/// kache resolves its own configured store. Check the data it would purge,
/// then let the native command coordinate with concurrent writers.
fn unused_kache_store(in_use: &dyn Fn(&Path) -> InUse) -> Result<(), String> {
    let tool = crate::sweep::find_tool("kache").ok_or("kache isn't installed")?;
    let stats = rush_sys::cmd::output_within(
        std::process::Command::new(tool).args(["stats", "--json"]),
        std::time::Duration::from_secs(10),
    )
    .map_err(|e| format!("couldn't locate kache's store: {e}"))?;
    let report = serde_json::from_slice::<Value>(&stats.out).ok();
    let stores = report.as_ref().and_then(|r| r["stores"].as_array());
    let Some(stores) = stores.filter(|s| stats.ok() && !s.is_empty()) else {
        return Err("couldn't locate kache's store".into());
    };
    for store in stores {
        let root = store["path"]
            .as_str()
            .map(Path::new)
            .filter(|p| p.is_absolute() && store["error"].is_null())
            .ok_or("couldn't locate kache's store")?;
        let data = root.join("store");
        if !super::cargo::real_dir(&data) {
            return Err(format!("{}: no kache store", data.display()));
        }
        unused(&data, in_use)?;
    }
    Ok(())
}

/// Whether a dry run writes a down instead of doing it.
fn dry(a: &Act) -> bool {
    rush_core::dryrun::would("disk-clean", &a.path.to_string_lossy(), &a.why)
}

/// Does one act, once in_use (asked now, right before) says nothing uses
/// its path, and says how much it freed, or why it didn't.
pub fn apply(a: &Act, in_use: &dyn Fn(&Path) -> InUse) -> Result<u64, String> {
    apply_checked(a, in_use, &|| true, &mut Pruned::default())
}

// An artifact becoming busy after earlier removals ends this batch. Keep
// the completed part in the event even when the rest has to wait.
#[derive(Default)]
struct Pruned {
    files: usize,
    bytes: u64,
}

fn apply_checked(
    a: &Act,
    in_use: &dyn Fn(&Path) -> InUse,
    needed: &dyn Fn() -> bool,
    pruned: &mut Pruned,
) -> Result<u64, String> {
    let p = &a.path;
    let ps = p.to_string_lossy();
    match &a.how {
        How::Run { command } => {
            if !known_tool(command) {
                return Err(format!("{}: not a known tool", command.join(" ")));
            }
            let args: Vec<&str> = command.iter().map(String::as_str).collect();
            if args.starts_with(&["kache", "clean"]) && args.contains(&"--cache") {
                unused_kache_store(in_use)?;
            }
            let before = rush_sys::sysinfo::read_disk().free;
            run_command(&args, &a.why)?;
            let after = rush_sys::sysinfo::read_disk().free;
            Ok(after.saturating_sub(before))
        }
        _ if !p.is_absolute() => Err(format!("won't delete {ps}: not an absolute path")),
        How::PruneGraph { last_used } => {
            let _lock = super::cargo::lock_graph(p, *last_used)
                .ok_or_else(|| format!("{ps}: kache unit busy, changed or unavailable"))?;
            let bytes = freeable(p);
            // rustc names the incremental root on its command line, and
            // chooses the crate subdirectory itself. The held unit.lock
            // is a sibling of that root.
            unused(p.parent().ok_or("graph has no rustc directory")?, in_use)?;
            if dry(a) {
                return Ok(0);
            }
            remove_tree(&ps)?;
            Ok(bytes)
        }
        How::Remove => {
            if a.kind == CARGO_TARGET && !is_target(p) {
                return Err(format!("{ps}: no longer a target"));
            }
            let n = freeable(p);
            unused(p, in_use)?;
            if dry(a) {
                return Ok(0);
            }
            remove_tree(&ps)?;
            Ok(n)
        }
        How::Prune { paths } => {
            unused(p, in_use)?;
            if dry(a) {
                return Ok(0);
            }
            for q in paths.iter().take_while(|_| needed()) {
                // A profile or deps directory may have been replaced since discovery.
                let within_target = q.strip_prefix(p).is_ok_and(|rel| {
                    !rel.as_os_str().is_empty()
                        && rel
                            .components()
                            .all(|c| matches!(c, std::path::Component::Normal(_)))
                }) && q
                    .ancestors()
                    .skip(1)
                    .take_while(|dir| *dir != p)
                    .all(super::cargo::real_dir);
                if !within_target || !is_target(p) {
                    return Err(format!("{}: no longer in a cargo target", q.display()));
                }
                let bytes = freeable(q);
                unused(q, in_use)?;
                remove_tree(&q.to_string_lossy())?;
                pruned.bytes += bytes;
                pruned.files += 1;
            }
            Ok(pruned.bytes)
        }
        How::RemoveWorktree { repo } => {
            let home = std::env::var("HOME").unwrap_or_default();
            if let Some(r) = never_touch(&ps, &home) {
                return Err(format!("won't delete {ps}: rush never touches {r}"));
            }
            let n = freeable(p);
            unused(p, in_use)?;
            if dry(a) {
                return Ok(0);
            }
            remove_worktree_git(repo, p).map_err(|e| format!("{ps}: {e}"))?;
            Ok(n)
        }
        How::GoClean => {
            let (size, _) = crate::walk(&ps);
            unused(p, in_use)?;
            crate::rules::go_clean(&ps, &a.why)?;
            Ok(size.max(0) as u64)
        }
    }
}

/// What an act did, in a few words.
fn what(a: &Act, pruned: usize) -> String {
    match &a.how {
        How::Remove => "removed".into(),
        How::PruneGraph { .. } => "removed old kache incremental graph".into(),
        How::Prune { .. } => format!(
            "removed {} in it",
            if pruned == 1 {
                "1 old build file".to_string()
            } else {
                format!("{pruned} old build files")
            }
        ),
        How::RemoveWorktree { .. } => "removed the worktree; its branch stays".into(),
        How::GoClean => "cleared with go clean -modcache".into(),
        How::Run { command } => format!("ran {}", command.join(" ")),
    }
}

/// Does acts in order, each asking in_use right before: what went, and
/// what stayed and why, after kept (the plan's).
pub fn apply_all(acts: &[Act], kept: Vec<Kept>, in_use: &dyn Fn(&Path) -> InUse) -> Outcome {
    apply_while(acts, kept, in_use, &|| true)
}

fn apply_while(
    acts: &[Act],
    kept: Vec<Kept>,
    in_use: &dyn Fn(&Path) -> InUse,
    needed: &dyn Fn() -> bool,
) -> Outcome {
    let mut o = Outcome {
        left: kept,
        ..Outcome::default()
    };
    for a in acts.iter().take_while(|_| needed()) {
        let mut pruned = Pruned::default();
        let freed = match apply_checked(a, in_use, needed, &mut pruned) {
            Ok(n) => n,
            Err(why) => {
                o.left.push(Kept {
                    path: a.path.clone(),
                    rule: a.rule.clone(),
                    why,
                });
                if pruned.files == 0 {
                    continue;
                }
                pruned.bytes
            }
        };
        if matches!(a.how, How::Prune { .. }) && pruned.files == 0 {
            continue;
        }
        o.freed += freed;
        o.did.push(Done {
            rule: a.rule.clone(),
            path: a.path.clone(),
            what: what(a, pruned.files),
            freed,
        });
    }
    o
}

/// The disk-clean event's fields: the free bytes, the level, what was
/// done, what was left and why, what it freed; why it did nothing when it
/// didn't run.
pub fn event_fields(free: u64, lv: Option<Level>, o: &Outcome, why: &str) -> Value {
    let mut f = json!({ "free": free, "freed": o.freed });
    if let Some(lv) = lv {
        f["level"] = json!(lv.name());
    }
    if !why.is_empty() {
        f["why"] = json!(why);
    }
    let path = |p: &Path| {
        if p.as_os_str().is_empty() {
            String::new()
        } else {
            format!("{}: ", p.display())
        }
    };
    if !o.did.is_empty() {
        f["did"] = json!(
            o.did
                .iter()
                .map(|d| format!("{}{} ({}, {})", path(&d.path), d.what, d.rule, d.freed))
                .collect::<Vec<_>>()
        );
    }
    if !o.left.is_empty() {
        f["left"] = json!(
            o.left
                .iter()
                .map(|k| format!("{}{} ({})", path(&k.path), k.why, k.rule))
                .collect::<Vec<_>>()
        );
    }
    f
}

/// One run at free bytes free: the candidates the engine finds in scan
/// and those handed in (plugins'), planned by rules at the level and
/// applied, each removal asking in_use right before. One disk-clean event
/// says what it did, or why it did nothing.
pub fn run(scan: &Scan, handed: Vec<Candidate>, rules: &[Rule], free: u64, now: Ms) -> Outcome {
    let Some(lv) = level(free) else {
        let why = if free == 0 {
            "couldn't read the disk's free space"
        } else {
            "enough free"
        };
        agent_event(
            None,
            "disk-clean",
            event_fields(free, None, &Outcome::default(), why),
        );
        return Outcome::default();
    };
    run_while(
        scan,
        handed,
        rules,
        free,
        lv,
        now,
        || matches!(level(rush_sys::sysinfo::read_disk().free), Some(next) if next == lv || next == Level::Low),
    )
}

/// run at level lv whatever free says: the dire clean's, which cleans as
/// hard as low however its own level is set. free is only said.
pub fn run_at(
    scan: &Scan,
    handed: Vec<Candidate>,
    rules: &[Rule],
    free: u64,
    lv: Level,
    now: Ms,
) -> Outcome {
    run_while(scan, handed, rules, free, lv, now, || true)
}

fn run_while(
    scan: &Scan,
    handed: Vec<Candidate>,
    rules: &[Rule],
    free: u64,
    lv: Level,
    now: Ms,
    still_needed: impl Fn() -> bool,
) -> Outcome {
    let mut cands = find_candidates(scan, rules, lv, now);
    cands.extend(handed);
    let (acts, kept) = plan(&cands, rules, lv, now);
    let o = apply_while(&acts, kept, &in_use, &still_needed);
    let why = if !acts.is_empty() && o.did.is_empty() && o.left.is_empty() {
        "disk level changed before removal"
    } else if o.did.is_empty() && o.left.is_empty() {
        "nothing a rule says goes"
    } else {
        ""
    };
    agent_event(None, "disk-clean", event_fields(free, Some(lv), &o, why));
    o
}
