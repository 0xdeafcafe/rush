//! The machine's caches that rebuild themselves: what Clean up offers when
//! the disk runs low, without starting an agent to look. Rush starts with
//! a few it knows ([`BUILT_IN`]) and learns more from each cleanup agent's
//! plan: a folder the plan marks conservative: yes, directly in
//! ~/Library/Caches or ~/.cache, is kept in rush's folder
//! (cleanup-rules.tsv, path and why, tab separated) and offered from then
//! on. Rust-only: Go had no such rules.
//!
//! Only a folder directly in a cache root can be a rule, never one of
//! rush's own caches, and removal checks that again: what's removed is
//! what a rule names, as it's named, a folder and not a link.

use std::path::Path;

use serde_json::json;

use crate::temp::{agent_event, err_text};

/// The caches rush offers before it has learned any: made again by the
/// tool that made them, and losing no work. Relative to the home folder.
pub const BUILT_IN: [(&str, &str); 10] = [
    ("Library/Caches/go-build", "Go's build cache"),
    (".cache/go-build", "Go's build cache"),
    ("Library/Caches/golangci-lint", "golangci-lint's cache"),
    ("Library/Caches/goimports", "goimports' cache"),
    (
        "Library/Caches/pnpm",
        "pnpm's metadata cache (not its store)",
    ),
    (
        "Library/Caches/com.microsoft.VSCode.ShipIt",
        "VS Code's update download",
    ),
    ("Library/Caches/deno", "Deno's module cache"),
    ("Library/Caches/dev.biomejs.biome", "Biome's downloads"),
    (".cache/act", "act's copies of GitHub actions"),
    (".cache/vitest", "vitest's module cache"),
];

/// The file rush keeps what it learned in, in its folder.
pub const RULES_FILE: &str = "cleanup-rules.tsv";

/// Folders in a cache root that are never a rule: rush's own.
const NEVER: [&str; 2] = ["rush", "agtop"];

/// A folder Clean up may offer: where, and why it's safe to go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRule {
    pub path: String,
    pub why: String,
    /// Learned from a cleanup agent's plan, not built in.
    pub learned: bool,
}

/// A rule's folder as measured: how big, and when it last changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoundCache {
    pub path: String,
    pub why: String,
    pub size: i64,
    pub newest: rush_core::time::Ms,
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The folders whose children may be rules, for home.
fn roots(home: &str) -> [String; 2] {
    let h = home.trim_end_matches('/');
    [format!("{h}/Library/Caches"), format!("{h}/.cache")]
}

/// Whether path may be a rule: a folder's name directly in a cache root
/// under home, not one of rush's own. By its text alone.
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
            })
        })
        .collect()
}

/// The built-in rules for home, then the learned ones, each path once.
pub fn rules_with(learned: Vec<CacheRule>, home: &str) -> Vec<CacheRule> {
    let mut out: Vec<CacheRule> = Vec::new();
    let h = home.trim_end_matches('/');
    let built = BUILT_IN.iter().map(|(p, why)| CacheRule {
        path: format!("{h}/{p}"),
        why: (*why).to_string(),
        learned: false,
    });
    for r in built.chain(learned) {
        if !home.is_empty() && !out.iter().any(|o| o.path == r.path) {
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

/// The rules' folders that are there, measured, biggest first. Off the UI:
/// it walks each.
pub fn measure_caches(rules: &[CacheRule]) -> Vec<FoundCache> {
    let mut out: Vec<FoundCache> = rules
        .iter()
        .filter(|r| is_dir(&r.path))
        .map(|r| {
            let (size, newest) = crate::walk(&r.path);
            FoundCache {
                path: r.path.clone(),
                why: r.why.clone(),
                size,
                newest,
            }
        })
        .filter(|f| f.size > 0)
        .collect();
    out.sort_by(|a, b| b.size.cmp(&a.size).then(a.path.cmp(&b.path)));
    out
}

/// Deletes the folder a rule names, as it's named: refused when no rule
/// names it, it may not be one, or it's a link. It says how much it freed.
/// A cache that's changing is still one; the tool that made it makes it
/// again.
pub fn remove_rule_cache(path: &str, rules: &[CacheRule], why: &str) -> Result<i64, String> {
    let r = (|| {
        let home = home();
        if !rules.iter().any(|r| r.path == path) || !may_be_rule(path, &home) {
            return Err(format!(
                "won't delete {path}: no cleanup rule names it as a cache"
            ));
        }
        if !is_dir(path) {
            return Err(format!("{path}: gone already, or not a folder"));
        }
        let (size, _) = crate::walk(path);
        if rush_core::dryrun::would("remove-rule-cache", path, why) {
            return Ok(0);
        }
        std::fs::remove_dir_all(path)
            .map(|_| size)
            .map_err(|e| e.to_string())
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

    #[test]
    fn learns_only_new_conservative_caches() {
        let plan = "path\tbytes\tconservative\twhy\n\
            /Users/me/Library/Caches/Arc\t9\tyes\tArc's cache\n\
            ~/.cache/prisma/\t9\tconservative: yes\tprisma engines\n\
            /Users/me/Library/Caches/go-build\t9\tyes\tbuilt in already\n\
            /Users/me/Library/Caches/ms-playwright\t9\tno\tneeds a reinstall\n\
            /Users/me/Source/w/node_modules\t9\tyes\ta worktree's\n\
            /Users/me/Library/Caches/rush\t9\tyes\trush's own\n\
            docker image prune -f\t0\tyes\ta command\n\
            /Users/me/Library/Caches/Arc\t9\tyes\ttwice\n";
        let have = rules_with(Vec::new(), H);
        let got: Vec<(String, String)> = learn_from_plan(plan, &have, H)
            .into_iter()
            .map(|r| (r.path, r.why))
            .collect();
        assert_eq!(
            got,
            [
                ("/Users/me/Library/Caches/Arc".into(), "Arc's cache".into()),
                ("/Users/me/.cache/prisma".into(), "prisma engines".into()),
            ]
        );
    }

    #[test]
    fn reads_the_rules_file() {
        let got = read_rules(
            "~/Library/Caches/Arc\tArc's cache\n/Users/me/.cache/x/\n~/Documents\tnot a cache\n",
            H,
        );
        let paths: Vec<&str> = got.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/Users/me/Library/Caches/Arc", "/Users/me/.cache/x"]
        );
        assert_eq!(
            tilde("/Users/me/Library/Caches/Arc", H),
            "~/Library/Caches/Arc"
        );
        let all = rules_with(got, H);
        assert_eq!(all.len(), BUILT_IN.len() + 2);
        assert!(
            all.iter().all(|r| may_be_rule(&r.path, H)),
            "built in may be rules"
        );
    }

    #[test]
    fn learns_measures_and_removes() {
        let home = home();
        let arc = format!("{home}/Library/Caches/Arc");
        std::fs::create_dir_all(format!("{arc}/a")).unwrap();
        std::fs::write(format!("{arc}/a/f"), vec![0u8; 8192]).unwrap();
        let plan = format!("{arc}\t8192\tyes\tArc's cache\n");
        let new = learn(&plan).unwrap();
        assert_eq!(new.len(), 1);
        assert!(learn(&plan).unwrap().is_empty(), "learned once");
        let rules = cache_rules();
        let found = measure_caches(&rules);
        let f = found
            .iter()
            .find(|f| f.path == arc)
            .expect("Arc's cache measured");
        assert!(f.size > 0);
        assert_eq!(f.why, "Arc's cache");
        let not_rule = format!("{home}/Library/Caches/Other");
        std::fs::create_dir_all(&not_rule).unwrap();
        assert!(remove_rule_cache(&not_rule, &rules, "test").is_err());
        assert!(remove_rule_cache(&arc, &rules, "test").unwrap() > 0);
        assert!(!Path::new(&arc).exists());
        assert!(Path::new(&not_rule).exists());
    }
}
