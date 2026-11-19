//! The machine's caches that rebuild themselves: what Clean up offers when
//! the disk runs low, without starting an agent to look. Rush starts with
//! the ones it knows ([`BUILT_IN`], and Go's module cache), and adds the
//! cache-folder rules of yours in the disk engine's rules
//! (disk-rules.json: proposals you accepted, rules you made in Clean up).
//! Rust-only: Go had no such rules.
//!
//! What a cleanup agent's plan teaches (a folder the plan marks
//! conservative: yes, directly in ~/Library/Caches or ~/.cache) is
//! proposed, never a rule that runs: it waits in Settings › Disk for your
//! yes (learn). The rules learned before that, kept in rush's folder as
//! cleanup-rules.tsv (path and why, tab separated; yours with "yours" and
//! days), are made proposals once (take_learned_in) and no longer read.
//!
//! A learned rule is only ever a folder directly in a cache root, never
//! one of rush's own caches. The built-in ones are an explicit allowlist,
//! some outside the cache roots (~/.nx's workspaces, a Node compile
//! cache, rush's own terraform plugins), some only once untouched for a
//! while (idle), one cleared by its tool rather than removed (Go's module
//! cache, whose files are read-only: `go clean -modcache`). Removal checks
//! all that again, and sweep::remove_tree refuses what rush never touches:
//! what's removed is what a rule names, as it's named, a folder and not a
//! link.

use std::path::Path;

use rush_core::time::Ms;
use serde_json::json;

use crate::disk::{self, Kind, Params, Proposal, Rule, RulesFile};
use crate::sweep::{DAY, HOUR, gigs, never_touch, remove_tree, span};
use crate::temp::{agent_event, err_text};

/// How a rule's folder goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum How {
    /// Removed, through sweep::remove_tree.
    #[default]
    Remove,
    /// Cleared by `go clean -modcache`, never removed by hand.
    GoClean,
    /// Not a folder: a command run once (sweep::end_commands).
    Run(&'static [&'static str]),
}

/// A cache rush knows without having learned it, relative to the home
/// folder.
#[derive(Clone, Copy, Debug)]
pub struct BuiltIn {
    pub path: &'static str,
    pub why: &'static str,
    /// It goes only once nothing in it changed for this long; 0 always.
    pub idle: Ms,
    /// The rule is for each folder in it (named in hex), not the folder.
    pub each: bool,
}

const fn always(path: &'static str, why: &'static str) -> BuiltIn {
    BuiltIn {
        path,
        why,
        idle: 0,
        each: false,
    }
}

/// The caches rush offers before it has learned any: made again by the
/// tool that made them, and losing no work.
pub const BUILT_IN: [BuiltIn; 19] = [
    always("Library/Caches/go-build", "Go's build cache"),
    always(".cache/go-build", "Go's build cache"),
    always("Library/Caches/golangci-lint", "golangci-lint's cache"),
    always("Library/Caches/goimports", "goimports' cache"),
    always(
        "Library/Caches/pnpm",
        "pnpm's metadata cache (not its store)",
    ),
    always(
        "Library/Caches/com.microsoft.VSCode.ShipIt",
        "VS Code's update download",
    ),
    always("Library/Caches/deno", "Deno's module cache"),
    always("Library/Caches/dev.biomejs.biome", "Biome's downloads"),
    always("Library/Caches/dotslash", "dotslash's downloads"),
    always("Library/Caches/Arc", "Arc's browser cache"),
    always(".cache/act", "act's copies of GitHub actions"),
    always(".cache/vitest", "vitest's module cache"),
    always(
        ".langwatch/portless/node-compile-cache",
        "Node's compile cache",
    ),
    always(
        "Library/Caches/rush/terraform-plugins",
        "terraform's plugins as rush keeps them; terraform init downloads them again",
    ),
    BuiltIn {
        path: ".cache/prisma",
        why: "Prisma's engine downloads",
        idle: 30 * DAY,
        each: false,
    },
    BuiltIn {
        path: ".cache/uv",
        why: "uv's package cache; running sessions use it",
        idle: HOUR,
        each: false,
    },
    // Logs nothing reads again, gone once the tool stopped writing them
    // for a week (portless's dev-server logs, 9G; kanban-code's one log,
    // 9G): the tool makes the folder again when it next logs.
    BuiltIn {
        path: ".langwatch/portless/logs",
        why: "portless's dev-server logs",
        idle: 7 * DAY,
        each: false,
    },
    BuiltIn {
        path: ".kanban-code/logs",
        why: "Kanban Code's logs",
        idle: 7 * DAY,
        each: false,
    },
    BuiltIn {
        path: ".nx",
        why: "an Nx workspace's cache",
        idle: 7 * DAY,
        each: true,
    },
];

/// The file rush kept what it learned in, in its folder, before rules
/// waited for your yes: read once, to make its lines proposals.
pub const RULES_FILE: &str = "cleanup-rules.tsv";

/// Folders in a cache root that are never a learned rule: rush's own.
const NEVER: [&str; 2] = ["rush", "agtop"];

/// A folder Clean up may offer: where, and why it's safe to go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheRule {
    pub path: String,
    pub why: String,
    /// Learned from a cleanup agent's plan, not built in.
    pub learned: bool,
    /// Made by you in Clean up, from what it found big and untouched: it
    /// may be anywhere diskmap::may_be_yours allows, and goes only once
    /// untouched for idle.
    pub yours: bool,
    pub idle: Ms,
    pub each: bool,
    pub how: How,
}

/// A rule's folder as measured: how big, when it last changed, and whether
/// the rule says it goes (ok) and in a few words why or why not (note).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoundCache {
    pub path: String,
    pub why: String,
    pub size: i64,
    pub newest: Ms,
    pub how: How,
    pub ok: bool,
    pub note: String,
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The folders whose children may be learned rules, for home.
fn roots(home: &str) -> [String; 2] {
    let h = home.trim_end_matches('/');
    [format!("{h}/Library/Caches"), format!("{h}/.cache")]
}

/// Whether path may be a learned rule: a folder's name directly in a cache
/// root under home, not one of rush's own. By its text alone.
pub fn may_be_rule(path: &str, home: &str) -> bool {
    if home.is_empty() || path.contains("/../") || path.ends_with("/..") {
        return false;
    }
    let p = Path::new(path);
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
        return false;
    };
    let name = name.to_string_lossy();
    if name.starts_with('.') || NEVER.contains(&name.as_ref()) {
        return false;
    }
    let parent = parent.to_string_lossy();
    roots(home).iter().any(|r| *r == parent)
}

/// Go's module cache for home: GOMODCACHE, else GOPATH's first /pkg/mod,
/// else ~/go/pkg/mod. None when that's somewhere rush never touches, or
/// not a folder of its own.
pub fn go_mod_cache(home: &str) -> Option<String> {
    let h = home.trim_end_matches('/');
    if h.is_empty() {
        return None;
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let p = env("GOMODCACHE")
        .or_else(|| env("GOPATH").and_then(|g| g.split(':').next().map(|f| format!("{f}/pkg/mod"))))
        .unwrap_or_else(|| format!("{h}/go/pkg/mod"));
    let p = crate::worktree::clean(&p);
    let shallow = Path::new(&p).components().count() < 3;
    (p.starts_with('/') && !shallow && p != h && never_touch(&p, h).is_none()).then_some(p)
}

/// path with home put back as ~, as the rules file keeps it.
fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home.trim_end_matches('/')) {
        Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// ~ in p made the home folder.
fn untilde(p: &str, home: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => format!("{}/{rest}", home.trim_end_matches('/')),
        _ => p.to_string(),
    }
}

/// The rules file's text read: path and why per line, and for one you
/// made, "yours" and the days it waits untouched. What may not be a rule
/// is left out.
pub fn read_rules(text: &str, home: &str) -> Vec<CacheRule> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let path = untilde(f.next()?.trim().trim_end_matches('/'), home);
            let why = f.next().unwrap_or("").trim().to_string();
            if f.next().map(str::trim) == Some("yours") {
                let days: i64 = f.next().and_then(|d| d.trim().parse().ok()).unwrap_or(30);
                return crate::diskmap::may_be_yours(&path, home).then_some(CacheRule {
                    path,
                    why,
                    learned: true,
                    yours: true,
                    idle: days.max(1) * DAY,
                    ..CacheRule::default()
                });
            }
            may_be_rule(&path, home).then_some(CacheRule {
                path,
                why,
                learned: true,
                ..CacheRule::default()
            })
        })
        .collect()
}

/// Makes a rule of a folder Clean up found big and untouched, as you
/// asked (r there): kept with the disk engine's rules, on (disk-rules.json,
/// update_rules), it goes from now on once untouched for idle. Refused
/// where a rule you make may not be, in a git checkout, or not a folder.
pub fn make_rule(path: &str, why: &str, idle: Ms) -> Result<CacheRule, String> {
    let home = home();
    if !crate::diskmap::may_be_yours(path, &home) {
        return Err(format!(
            "won't make a rule of {path}: rush keeps out of there"
        ));
    }
    if !is_dir(path) {
        return Err(format!("{path}: gone, or not a folder"));
    }
    if Path::new(path)
        .ancestors()
        .take_while(|a| a.to_string_lossy().len() > home.trim_end_matches('/').len())
        .any(|a| std::fs::symlink_metadata(a.join(".git")).is_ok())
    {
        return Err(format!(
            "won't make a rule of {path}: it's in a git checkout"
        ));
    }
    let days = (idle / DAY).max(1);
    let w = why.replace(['\t', '\n'], " ");
    let made = CacheRule {
        path: path.to_string(),
        why: w,
        learned: true,
        yours: true,
        idle: days * DAY,
        ..CacheRule::default()
    };
    let rule = as_disk_rule(&made, &home);
    disk::update_rules(|f| {
        f.rules.retain(|r| r.id != rule.id);
        f.rules.push(rule);
    })
    .map_err(|e| e.to_string())?;
    agent_event(
        None,
        "cleanup-rule-made",
        json!({ "path": path, "why": why, "idleDays": days }),
    );
    Ok(made)
}

/// A cache rule's id in the disk engine's rules: "cache:" and its path
/// below home, as the built-in ones; or its whole path.
fn rule_id(path: &str, home: &str) -> String {
    let t = tilde(path, home);
    match t.strip_prefix("~/") {
        Some(rest) => format!("cache:{rest}"),
        None => format!("cache:{t}"),
    }
}

fn idle_secs(idle: Ms) -> Params {
    Params {
        idle_secs: Some((idle.max(0) / 1000) as u64),
        ..Params::default()
    }
}

/// r as a disk engine rule, on: a cache folder at r's idle; one you made
/// at both levels, a learned one only when low (as the built-in ones).
pub fn as_disk_rule(r: &CacheRule, home: &str) -> Rule {
    let why = if r.why.trim().is_empty() {
        "a cache its tool makes again".to_string()
    } else {
        r.why.clone()
    };
    Rule {
        id: rule_id(&r.path, home),
        title: why.clone(),
        kind: Kind::CacheDir {
            path: tilde(&r.path, home),
            why,
            each: r.each,
            go_clean: false,
        },
        enabled: true,
        normal: r.yours.then(|| idle_secs(r.idle)),
        low: Some(idle_secs(r.idle)),
    }
}

/// The cache-folder rules of yours in f that are on (proposals you
/// accepted, rules you made), as Clean up's rules: the built-in ones and
/// Go's module cache are left to rules_with. One outside a cache root is
/// yours, by where it is.
pub fn accepted_cache_rules(f: &RulesFile, home: &str) -> Vec<CacheRule> {
    let built: Vec<String> = disk::built_in_rules().into_iter().map(|r| r.id).collect();
    f.rules
        .iter()
        .filter(|r| r.enabled && !built.contains(&r.id))
        .filter_map(|r| {
            let Kind::CacheDir {
                path,
                why,
                each,
                go_clean: false,
            } = &r.kind
            else {
                return None;
            };
            if path.is_empty() {
                return None;
            }
            let p = untilde(path.trim_end_matches('/'), home);
            let idle = r
                .low
                .as_ref()
                .or(r.normal.as_ref())
                .and_then(|p| p.idle_secs)
                .unwrap_or(0);
            Some(CacheRule {
                yours: !may_be_rule(&p, home),
                path: p,
                why: why.clone(),
                learned: true,
                idle: (idle as Ms).saturating_mul(1000),
                each: *each,
                how: How::Remove,
            })
        })
        .collect()
}

/// The built-in rules for home, Go's module cache, then the learned ones,
/// each path once.
pub fn rules_with(learned: Vec<CacheRule>, home: &str) -> Vec<CacheRule> {
    let mut out: Vec<CacheRule> = Vec::new();
    if home.is_empty() {
        return out;
    }
    let h = home.trim_end_matches('/');
    let built = BUILT_IN.iter().map(|b| CacheRule {
        path: format!("{h}/{}", b.path),
        why: b.why.to_string(),
        idle: b.idle,
        each: b.each,
        ..CacheRule::default()
    });
    let go = go_mod_cache(home).map(|path| CacheRule {
        path,
        why: "Go's module cache; builds fetch it again".into(),
        how: How::GoClean,
        ..CacheRule::default()
    });
    for r in built.chain(go).chain(learned) {
        if !out.iter().any(|o| o.path == r.path) {
            out.push(r);
        }
    }
    out
}

fn rules_path() -> std::path::PathBuf {
    rush_state::dir().join(RULES_FILE)
}

/// Every rule: built in, and yours in the disk engine's rules
/// (accepted_cache_rules); only the built-in ones when those can't be
/// read.
pub fn cache_rules() -> Vec<CacheRule> {
    let home = home();
    let yours = disk::load_rules()
        .map(|f| accepted_cache_rules(&f, &home))
        .unwrap_or_default();
    rules_with(yours, &home)
}

/// What a cleanup agent's plan (cleanup-plan.tsv: path, bytes,
/// conservative, why) teaches, given the rules there are: each line marked
/// conservative: yes whose path may be a rule and isn't one yet.
pub fn learn_from_plan(plan: &str, have: &[CacheRule], home: &str) -> Vec<CacheRule> {
    let mut out: Vec<CacheRule> = Vec::new();
    for l in plan.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        let (Some(path), Some(c)) = (f.first(), f.get(2)) else {
            continue;
        };
        let c = c.trim().to_ascii_lowercase();
        if c != "yes" && c != "conservative: yes" {
            continue;
        }
        let path = untilde(path.trim().trim_end_matches('/'), home);
        if !may_be_rule(&path, home) || have.iter().chain(out.iter()).any(|r| r.path == path) {
            continue;
        }
        let why = f.get(3).map_or("", |w| w.trim()).to_string();
        out.push(CacheRule {
            path,
            why,
            learned: true,
            ..CacheRule::default()
        });
    }
    out
}

/// What plan says of path: its bytes, as the plan measured them.
fn plan_bytes(plan: &str, path: &str, home: &str) -> Option<i64> {
    plan.lines().find_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        (untilde(f.first()?.trim().trim_end_matches('/'), home) == path)
            .then(|| f.get(1)?.trim().parse().ok())
            .flatten()
    })
}

/// Learns from a cleanup agent's plan: what it teaches (learn_from_plan)
/// is proposed in Settings › Disk, never made a rule that runs; from (the
/// plan as "<session>/cleanup-plan.tsv") once, so one dismissed isn't
/// proposed again. What it proposed; empty when nothing new.
pub fn learn(from: &str, plan: &str) -> Result<Vec<CacheRule>, String> {
    let home = home();
    let new = learn_from_plan(plan, &cache_rules(), &home);
    if new.is_empty() {
        return Ok(new);
    }
    let at = rush_core::time::now();
    let props = new
        .iter()
        .map(|r| {
            let size = plan_bytes(plan, &r.path, &home)
                .map(|b| format!(", {}", gigs(b)))
                .unwrap_or_default();
            Proposal {
                rule: as_disk_rule(r, &home),
                evidence: format!(
                    "{}{size}, in a cleanup agent's plan, marked conservative: yes (only something made or fetched again): {}",
                    tilde(&r.path, &home),
                    if r.why.is_empty() { "no why given" } else { r.why.as_str() }
                ),
                by: String::new(),
                at,
            }
        })
        .collect();
    let got = disk::propose(props, Vec::new(), "a cleanup agent's plan", at, from)
        .map_err(|e| e.to_string())?;
    Ok(new
        .into_iter()
        .filter(|r| got.added.contains(&rule_id(&r.path, &home)))
        .collect())
}

/// The cache rules learned before rules waited for your yes
/// (cleanup-rules.tsv: learned from cleanup agents' plans, or made by you
/// in Clean up) made proposals, once (the rules file's learnedTakenIn), so
/// nothing a past agent learned runs without your yes. How many were
/// proposed. A file that can't be read is an error, and tried again
/// next time.
pub fn take_learned_in() -> Result<usize, String> {
    let home = home();
    let at = rush_core::time::now();
    let mut n = 0;
    let mut err = String::new();
    disk::update_rules(|f| {
        if f.learned_taken_in {
            return;
        }
        let text = match std::fs::read_to_string(rules_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                err = format!("{}: {e}", rules_path().display());
                return;
            }
        };
        let props = read_rules(&text, &home)
            .into_iter()
            .map(|r| Proposal {
                rule: as_disk_rule(&r, &home),
                evidence: if r.yours {
                    format!(
                        "{}: you made it in Clean up, before rules waited for your yes: it went once untouched for {}. It was in cleanup-rules.tsv.",
                        tilde(&r.path, &home),
                        span(r.idle)
                    )
                } else {
                    format!(
                        "{}: learned from a cleanup agent's plan before rules waited for your yes ({}). It was in cleanup-rules.tsv, and Clean up offered it.",
                        tilde(&r.path, &home),
                        if r.why.is_empty() { "no why given" } else { r.why.as_str() }
                    )
                },
                by: if r.yours {
                    "you, in Clean up".into()
                } else {
                    "a cleanup agent's plan".into()
                },
                at,
            })
            .collect();
        n = disk::add_proposals(f, props, &home).added.len();
        f.learned_taken_in = true;
    })
    .map_err(|e| e.to_string())?;
    if !err.is_empty() {
        return Err(err);
    }
    if n > 0 {
        agent_event(
            None,
            "cleanup-rules-proposed",
            json!({ "from": RULES_FILE, "proposed": n }),
        );
    }
    Ok(n)
}

fn is_dir(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// A name an each-rule takes: a workspace's hash.
fn hex_name(n: &str) -> bool {
    n.len() >= 6 && n.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The folders a rule is for: its own, or each one in it an each-rule
/// takes.
fn targets(r: &CacheRule) -> Vec<String> {
    if !r.each {
        return vec![r.path.clone()];
    }
    let Ok(rd) = std::fs::read_dir(&r.path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| hex_name(&e.file_name().to_string_lossy()))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// Whether a rule says a folder whose newest change is newest goes now,
/// and why or why not, in a few words ("" for one that always goes).
pub fn decide(idle: Ms, newest: Ms, now: Ms) -> (bool, String) {
    if idle == 0 {
        return (true, String::new());
    }
    let ago = now - newest;
    if ago >= idle {
        (true, format!("untouched for {}", span(ago)))
    } else {
        (
            false,
            format!(
                "written {} ago; goes once untouched for {}",
                span(ago),
                span(idle)
            ),
        )
    }
}

/// The rules' folders that are there, measured and decided, biggest
/// first. Off the UI: it walks each.
pub fn measure_caches(rules: &[CacheRule], now: Ms) -> Vec<FoundCache> {
    let mut out: Vec<FoundCache> = Vec::new();
    for r in rules {
        for path in targets(r).into_iter().filter(|p| is_dir(p)) {
            let (size, newest) = crate::walk(&path);
            if size <= 0 {
                continue;
            }
            let (ok, note) = decide(r.idle, newest, now);
            out.push(FoundCache {
                path,
                why: r.why.clone(),
                size,
                newest,
                how: r.how,
                ok,
                note,
            });
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size).then(a.path.cmp(&b.path)));
    out
}

/// The rule that names path, as it's named, among rules: one whose folder
/// it is, or an each-rule's workspace in it. Built-in rules only as rush
/// builds them for home, learned ones only where a rule may be; never
/// anything rush never touches.
pub fn rule_for<'a>(path: &str, rules: &'a [CacheRule], home: &str) -> Option<&'a CacheRule> {
    if home.is_empty() || crate::worktree::clean(path) != path || never_touch(path, home).is_some()
    {
        return None;
    }
    let ours = rules_with(Vec::new(), home);
    let allowed = |r: &CacheRule| {
        if r.yours {
            crate::diskmap::may_be_yours(&r.path, home) && r.idle > 0
        } else if r.learned {
            may_be_rule(&r.path, home)
        } else {
            ours.iter().any(|o| o == r)
        }
    };
    rules.iter().find(|r| {
        allowed(r)
            && (r.path == path && !r.each
                || r.each
                    && Path::new(path).parent() == Some(Path::new(&r.path))
                    && Path::new(path)
                        .file_name()
                        .is_some_and(|n| hex_name(&n.to_string_lossy())))
    })
}

/// go clean -modcache for the cache at path (or a dry run writes it down).
pub(crate) fn go_clean(path: &str, why: &str) -> Result<(), String> {
    if rush_core::dryrun::would("go-clean-modcache", path, why) {
        return Ok(());
    }
    let Some(go) = crate::sweep::find_tool("go") else {
        return Err(format!(
            "{path}: go isn't installed, and its read-only files go only through go clean -modcache"
        ));
    };
    let mut cmd = std::process::Command::new(go);
    cmd.args(["clean", "-modcache"]).env("GOMODCACHE", path);
    let o = rush_sys::cmd::output_within(&mut cmd, std::time::Duration::from_secs(600))
        .map_err(|e| format!("go clean -modcache: {e}"))?;
    if !o.ok() {
        return Err(format!("go clean -modcache: {}", o.err_text()));
    }
    Ok(())
}

/// Deletes the folder a rule names, as it's named: refused when no rule
/// names it, it may not be one, it's a link, or a rule that waits for it
/// to sit untouched finds it changed since. Go's module cache goes through
/// go clean -modcache. It says how much it freed. A cache that's changing
/// is still one; the tool that made it makes it again.
pub fn remove_rule_cache(path: &str, rules: &[CacheRule], why: &str) -> Result<i64, String> {
    let r = (|| {
        let home = home();
        let Some(rule) = rule_for(path, rules, &home) else {
            return Err(format!(
                "won't delete {path}: no cleanup rule names it as a cache"
            ));
        };
        if !is_dir(path) {
            return Err(format!("{path}: gone already, or not a folder"));
        }
        let (size, newest) = crate::walk(path);
        let (ok, note) = decide(rule.idle, newest, rush_core::time::now());
        if !ok {
            return Err(format!("{path}: {note}; it stays"));
        }
        match rule.how {
            How::GoClean => go_clean(path, why).map(|_| size),
            How::Run(_) => Err(format!("{path}: a command, not a folder")),
            How::Remove => {
                if rush_core::dryrun::would("remove-rule-cache", path, why) {
                    return Ok(0);
                }
                remove_tree(path).map(|_| size)
            }
        }
    })();
    agent_event(
        None,
        "cache-removed",
        json!({
            "path": path,
            "why": why,
            "bytes": r.as_ref().copied().unwrap_or(0),
            "err": err_text(&r),
        }),
    );
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "/Users/me";

    #[test]
    fn what_may_be_a_rule() {
        for (p, want) in [
            ("/Users/me/Library/Caches/go-build", true),
            ("/Users/me/.cache/act", true),
            ("/Users/me/Library/Caches/rush", false),
            ("/Users/me/Library/Caches/agtop", false),
            ("/Users/me/Library/Caches", false),
            ("/Users/me/Library/Caches/go-build/01", false),
            ("/Users/me/Library/Caches/../Mail", false),
            ("/Users/me/Library/Caches/.hidden", false),
            ("/Users/me/Source/x/node_modules", false),
            ("/Users/other/Library/Caches/go-build", false),
            ("/Users/me/Library/Application Support/x", false),
        ] {
            assert_eq!(may_be_rule(p, H), want, "{p}");
        }
        assert!(!may_be_rule("/Library/Caches/x", ""), "no home: never");
    }

    // The built-in allowlist: what it names, an each-rule's hex workspaces,
    // never what rush never touches, never a learned rule outside a cache
    // root, never one made up that rush doesn't build.
    #[test]
    fn which_rule_names_a_path() {
        let learned = read_rules(
            "~/Library/Caches/Arc2\tArc's other\n\
             ~/.zed/old-logs\tZed's logs\tyours\t30\n\
             ~/Documents/old\tnot yours to make\tyours\t30\n\
             ~/.zed\ttoo shallow\tyours\t30\n",
            H,
        );
        assert_eq!(learned.len(), 2, "{learned:?}");
        assert!(learned[1].yours && learned[1].idle == 30 * DAY);
        let mut rules = rules_with(learned, H);
        rules.push(CacheRule {
            path: "/Users/me/Documents".into(),
            why: "made up".into(),
            ..CacheRule::default()
        });
        rules.push(CacheRule {
            path: "/Users/me/.claude/projects".into(),
            why: "made up".into(),
            learned: true,
            ..CacheRule::default()
        });
        for (p, want) in [
            (
                "/Users/me/Library/Caches/dotslash",
                Some("dotslash's downloads"),
            ),
            (
                "/Users/me/Library/Caches/rush/terraform-plugins",
                Some("terraform's plugins as rush keeps them; terraform init downloads them again"),
            ),
            (
                "/Users/me/.langwatch/portless/node-compile-cache",
                Some("Node's compile cache"),
            ),
            (
                "/Users/me/.nx/9f2fa26914b570a8",
                Some("an Nx workspace's cache"),
            ),
            ("/Users/me/.nx", None),
            ("/Users/me/.kanban-code/logs", Some("Kanban Code's logs")),
            (
                "/Users/me/.langwatch/portless/logs",
                Some("portless's dev-server logs"),
            ),
            ("/Users/me/.kanban-code", None),
            ("/Users/me/.nx/daemon", None),
            (
                "/Users/me/.cache/uv",
                Some("uv's package cache; running sessions use it"),
            ),
            ("/Users/me/Library/Caches/Arc2", Some("Arc's other")),
            ("/Users/me/.zed/old-logs", Some("Zed's logs")),
            ("/Users/me/Documents/old", None),
            ("/Users/me/Library/Caches/rush", None),
            ("/Users/me/Documents", None),
            ("/Users/me/.claude/projects", None),
            ("/Users/me/Library/Caches/dotslash/", None),
            ("/Users/me/Library/Caches/x/../dotslash", None),
        ] {
            assert_eq!(rule_for(p, &rules, H).map(|r| r.why.as_str()), want, "{p}");
        }
    }

    #[test]
    fn decides_by_idle() {
        let now = 100 * DAY;
        for (idle, ago, ok, note) in [
            (0, 0, true, ""),
            (
                HOUR,
                20 * 60_000,
                false,
                "written 20m ago; goes once untouched for 1h",
            ),
            (HOUR, 2 * HOUR, true, "untouched for 2h"),
            (
                30 * DAY,
                3 * DAY,
                false,
                "written 3 days ago; goes once untouched for 30 days",
            ),
            (30 * DAY, 40 * DAY, true, "untouched for 40 days"),
            (7 * DAY, 8 * DAY, true, "untouched for 8 days"),
        ] {
            assert_eq!(
                decide(idle, now - ago, now),
                (ok, note.to_string()),
                "{idle} {ago}"
            );
        }
    }

    #[test]
    fn learns_only_new_conservative_caches() {
        let plan = "path\tbytes\tconservative\twhy\n\
            /Users/me/Library/Caches/Spotify\t9\tyes\tSpotify's cache\n\
            ~/.cache/huggingface/\t9\tconservative: yes\tmodels\n\
            /Users/me/Library/Caches/go-build\t9\tyes\tbuilt in already\n\
            /Users/me/Library/Caches/ms-playwright\t9\tno\tneeds a reinstall\n\
            /Users/me/Source/w/node_modules\t9\tyes\ta worktree's\n\
            /Users/me/Library/Caches/rush\t9\tyes\trush's own\n\
            docker image prune -f\t0\tyes\ta command\n\
            /Users/me/Library/Caches/Spotify\t9\tyes\ttwice\n";
        let have = rules_with(Vec::new(), H);
        let got: Vec<(String, String)> = learn_from_plan(plan, &have, H)
            .into_iter()
            .map(|r| (r.path, r.why))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "/Users/me/Library/Caches/Spotify".into(),
                    "Spotify's cache".into()
                ),
                ("/Users/me/.cache/huggingface".into(), "models".into()),
            ]
        );
    }

    #[test]
    fn reads_the_rules_file() {
        let got = read_rules(
            "~/Library/Caches/Spotify\tSpotify's cache\n/Users/me/.cache/x/\n~/Documents\tnot a cache\n",
            H,
        );
        let paths: Vec<&str> = got.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/Users/me/Library/Caches/Spotify", "/Users/me/.cache/x"]
        );
        assert_eq!(
            tilde("/Users/me/Library/Caches/Arc", H),
            "~/Library/Caches/Arc"
        );
        let all = rules_with(got, H);
        let go = usize::from(go_mod_cache(H).is_some());
        assert_eq!(all.len(), BUILT_IN.len() + go + 2);
        assert!(
            all.iter()
                .all(|r| rule_for(&r.path, &all, H).is_some() || r.each),
            "each rule names its own folder"
        );
    }

    #[test]
    fn go_mod_cache_never_somewhere_shallow_or_untouchable() {
        let c = go_mod_cache(H);
        assert!(
            c.as_deref()
                .is_none_or(|c| c.ends_with("/pkg/mod") || c.len() > 3)
        );
        assert_eq!(go_mod_cache(""), None);
    }

    /// rush's folder, for this test's thread only.
    struct RushDir(tempfile::TempDir);

    impl RushDir {
        fn new() -> RushDir {
            let d = tempfile::tempdir().unwrap();
            rush_state::set_thread_dir(Some(d.path().canonicalize().unwrap()));
            RushDir(d)
        }
    }

    impl Drop for RushDir {
        fn drop(&mut self) {
            rush_state::set_thread_dir(None);
        }
    }

    // What a plan teaches is proposed, never a rule that runs: Clean up
    // neither offers nor removes it until it's accepted; a plan is taken
    // once, and the same proposal isn't made twice.
    #[test]
    fn learns_measures_and_removes() {
        let _dir = RushDir::new();
        let home = home();
        let sp = format!("{home}/Library/Caches/Spotify");
        std::fs::create_dir_all(format!("{sp}/a")).unwrap();
        std::fs::write(format!("{sp}/a/f"), vec![0u8; 8192]).unwrap();
        let plan = format!("{sp}\t8192\tyes\tSpotify's cache\n");
        let new = learn("s1/cleanup-plan.tsv", &plan).unwrap();
        assert_eq!(new.len(), 1);
        let f = disk::load_rules().unwrap();
        assert_eq!(f.proposed.len(), 1, "{f:?}");
        let p = &f.proposed[0];
        assert_eq!(p.rule.id, "cache:Library/Caches/Spotify");
        assert_eq!(p.by, "a cleanup agent's plan");
        assert!(p.evidence.contains("Spotify's cache"), "{}", p.evidence);
        assert!(!f.rules.iter().any(|r| r.id == p.rule.id), "not a rule");
        assert!(cache_rules().iter().all(|r| r.path != sp), "not offered");
        assert!(remove_rule_cache(&sp, &cache_rules(), "test").is_err());
        assert!(Path::new(&sp).exists());
        assert!(
            learn("s1/cleanup-plan.tsv", &plan).unwrap().is_empty(),
            "a plan is taken once"
        );
        assert!(
            learn("s2/cleanup-plan.tsv", &plan).unwrap().is_empty(),
            "the same proposal isn't made twice"
        );
        assert_eq!(disk::load_rules().unwrap().proposed.len(), 1);
        // Accepted, it's a rule Clean up offers.
        disk::update_rules(|f| {
            let p = f.proposed.remove(0);
            f.rules.push(p.rule);
        })
        .unwrap();
        // A read-only bundle, as dotslash keeps them.
        let ds = format!("{home}/Library/Caches/dotslash");
        let app = format!("{ds}/x/Tool.app/Contents");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(format!("{app}/bin"), "x").unwrap();
        set_mode(&format!("{app}/bin"), 0o444);
        set_mode(&app, 0o555);
        set_mode(&format!("{ds}/x/Tool.app"), 0o555);
        // uv, written just now: it stays.
        let uv = format!("{home}/.cache/uv");
        std::fs::create_dir_all(&uv).unwrap();
        std::fs::write(format!("{uv}/f"), "x").unwrap();
        let rules = cache_rules();
        let now = rush_core::time::now();
        let found = measure_caches(&rules, now);
        let f = found
            .iter()
            .find(|f| f.path == sp)
            .expect("Spotify's cache measured");
        assert!(f.size > 0 && f.ok);
        assert_eq!(f.why, "Spotify's cache");
        let u = found.iter().find(|f| f.path == uv).expect("uv measured");
        assert!(!u.ok, "uv written just now");
        assert!(remove_rule_cache(&uv, &rules, "test").is_err());
        assert!(Path::new(&uv).exists());
        let not_rule = format!("{home}/Library/Caches/Other");
        std::fs::create_dir_all(&not_rule).unwrap();
        assert!(remove_rule_cache(&not_rule, &rules, "test").is_err());
        assert!(remove_rule_cache(&sp, &rules, "test").unwrap() > 0);
        assert!(remove_rule_cache(&ds, &rules, "test").unwrap() > 0);
        assert!(!Path::new(&sp).exists() && !Path::new(&ds).exists());
        assert!(Path::new(&not_rule).exists());
    }

    // cleanup-rules.tsv's lines become proposals once, by who made them,
    // and run nowhere; what isn't a rule is left out. A rule you make in
    // Clean up is one, on, in the disk engine's rules.
    #[test]
    fn learned_rules_taken_in_once() {
        let dir = RushDir::new();
        let home = home();
        std::fs::write(
            dir.0.path().join(RULES_FILE),
            "~/Library/Caches/Spotify\tSpotify's cache\n\
             ~/.zed/old-logs\tZed's logs\tyours\t30\n\
             ~/Documents\tnot a cache\n",
        )
        .unwrap();
        assert_eq!(take_learned_in(), Ok(2));
        let f = disk::load_rules().unwrap();
        assert!(f.learned_taken_in);
        let got: Vec<(&str, &str, bool, bool)> = f
            .proposed
            .iter()
            .map(|p| {
                (
                    p.rule.id.as_str(),
                    p.by.as_str(),
                    p.rule.normal.is_some(),
                    p.rule.low.is_some(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    "cache:Library/Caches/Spotify",
                    "a cleanup agent's plan",
                    false,
                    true
                ),
                ("cache:.zed/old-logs", "you, in Clean up", true, true),
            ]
        );
        assert!(
            f.proposed[1].evidence.contains("30 days"),
            "{}",
            f.proposed[1].evidence
        );
        let rules = cache_rules();
        assert!(
            rules
                .iter()
                .all(|r| !r.path.ends_with("/Spotify") && !r.path.ends_with("/old-logs")),
            "nothing learned runs"
        );
        assert_eq!(take_learned_in(), Ok(0), "once");
        assert_eq!(disk::load_rules().unwrap().proposed.len(), 2);
        // Made in Clean up: a rule, on, at both levels.
        let logs = format!("{home}/.kanban/old-logs");
        std::fs::create_dir_all(&logs).unwrap();
        let made = make_rule(&logs, "Kanban's old logs", 30 * DAY).unwrap();
        assert!(made.yours && made.idle == 30 * DAY);
        let f = disk::load_rules().unwrap();
        let r = f
            .rules
            .iter()
            .find(|r| r.id == "cache:.kanban/old-logs")
            .expect("made");
        assert!(r.enabled);
        assert_eq!(r.normal.as_ref().unwrap().idle_secs, Some(30 * 24 * 3600));
        let c = cache_rules();
        let mine = c.iter().find(|r| r.path == logs).expect("offered");
        assert!(mine.yours && mine.idle == 30 * DAY);
        assert!(rule_for(&logs, &c, &home).is_some());
    }

    fn set_mode(p: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}
