//! The machine's caches that rebuild themselves: what Clean up offers when
//! the disk runs low, without starting an agent to look. Rush starts with
//! the ones it knows ([`BUILT_IN`], and Go's module cache) and learns more
//! from each cleanup agent's plan: a folder the plan marks conservative:
//! yes, directly in ~/Library/Caches or ~/.cache, is kept in rush's folder
//! (cleanup-rules.tsv, path and why, tab separated) and offered from then
//! on. Rust-only: Go had no such rules.
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

use crate::sweep::{DAY, HOUR, never_touch, remove_tree, span};
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
pub const BUILT_IN: [BuiltIn; 17] = [
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
    BuiltIn {
        path: ".nx",
        why: "an Nx workspace's cache",
        idle: 7 * DAY,
        each: true,
    },
];

/// The file rush keeps what it learned in, in its folder.
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

/// The rules file's text read: path and why per line. What may not be a
/// rule is left out.
pub fn read_rules(text: &str, home: &str) -> Vec<CacheRule> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(2, '\t');
            let path = untilde(f.next()?.trim().trim_end_matches('/'), home);
            let why = f.next().unwrap_or("").trim().to_string();
            may_be_rule(&path, home).then_some(CacheRule {
                path,
                why,
                learned: true,
                ..CacheRule::default()
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

/// Every rule: built in, and learned (rush's folder's cleanup-rules.tsv).
pub fn cache_rules() -> Vec<CacheRule> {
    let home = home();
    let learned = std::fs::read_to_string(rules_path())
        .map(|t| read_rules(&t, &home))
        .unwrap_or_default();
    rules_with(learned, &home)
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

/// Learns from a cleanup agent's plan, kept in rush's folder: what it
/// learned, empty when nothing new.
pub fn learn(plan: &str) -> Result<Vec<CacheRule>, String> {
    let home = home();
    let new = learn_from_plan(plan, &cache_rules(), &home);
    if new.is_empty() {
        return Ok(new);
    }
    let p = rules_path();
    let mut text = std::fs::read_to_string(&p).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for r in &new {
        let why = r.why.replace(['\t', '\n'], " ");
        text.push_str(&format!("{}\t{why}\n", tilde(&r.path, &home)));
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, text).map_err(|e| e.to_string())?;
    agent_event(
        None,
        "cleanup-rules-learned",
        json!({ "paths": new.iter().map(|r| r.path.clone()).collect::<Vec<_>>() }),
    );
    Ok(new)
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
        if r.learned {
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
fn go_clean(path: &str, why: &str) -> Result<(), String> {
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
        let learned = read_rules("~/Library/Caches/Arc2\tArc's other\n", H);
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
            ("/Users/me/.nx/daemon", None),
            (
                "/Users/me/.cache/uv",
                Some("uv's package cache; running sessions use it"),
            ),
            ("/Users/me/Library/Caches/Arc2", Some("Arc's other")),
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

    #[test]
    fn learns_measures_and_removes() {
        let home = home();
        let sp = format!("{home}/Library/Caches/Spotify");
        std::fs::create_dir_all(format!("{sp}/a")).unwrap();
        std::fs::write(format!("{sp}/a/f"), vec![0u8; 8192]).unwrap();
        let plan = format!("{sp}\t8192\tyes\tSpotify's cache\n");
        let new = learn(&plan).unwrap();
        assert_eq!(new.len(), 1);
        assert!(learn(&plan).unwrap().is_empty(), "learned once");
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

    fn set_mode(p: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}
