//! Where the disk goes, measured without an agent: what docker says it
//! could free, how much of colima's VM disk macOS could have back, the
//! home folder's biggest folders, and what's big and untouched that no
//! cleanup rule names. Rust-only: Go had none.
//!
//! - docker_reclaim: `docker system df`'s reclaimable, by kind (images,
//!   build cache, volumes), the sizes of Clean up's docker rows.
//! - colima_disk: colima's VM disk file as macOS holds it, and what's used
//!   in it: the difference is what a trim gives back; nearly full inside,
//!   docker hangs.
//! - map_home: one walk of the home folder (never into a git checkout,
//!   what rush never touches, your own folders, or Library but its Caches
//!   and Logs), kept for MAP_FOR so Clean up doesn't walk it each time: the
//!   size of each folder at the top, and what's 1G or more and untouched for
//!   30 days that no rule names, each with a guess at what it is (logs, a
//!   cache, scratch) by its name.
//! - may_be_yours: where a rule you make in Clean up may be.

use std::path::Path;
use std::time::Duration;

use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

use crate::rules::CacheRule;
use crate::sweep::{DAY, HOUR, never_touch};

/// A command's output within limit, or None (not there, failed, too slow).
fn output(args: &[&str], limit: Duration) -> Option<String> {
    let (name, rest) = args.split_first()?;
    let tool = crate::sweep::find_tool(name)?;
    let mut cmd = std::process::Command::new(tool);
    cmd.args(rest);
    let o = rush_sys::cmd::output_within(&mut cmd, limit).ok()?;
    o.ok().then(|| String::from_utf8_lossy(&o.out).into_owned())
}

// ---- docker ----

/// A size as docker writes it (decimal units: "15.09GB", "273.3MB",
/// "839.1kB", "0B"), in bytes.
pub fn parse_size(s: &str) -> Option<i64> {
    let s = s.trim();
    let at = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (n, unit) = s.split_at(at);
    let n: f64 = n.parse().ok()?;
    let mult = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((n * mult).round() as i64)
}

/// What docker says it could free, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DockerReclaim {
    pub images: i64,
    pub build: i64,
    pub volumes: i64,
}

/// `docker system df --format '{{json .}}'`'s lines read: each kind's
/// Reclaimable ("15.09GB (61%)").
pub fn parse_docker_df(text: &str) -> DockerReclaim {
    let mut r = DockerReclaim::default();
    for l in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else {
            continue;
        };
        let size = v["Reclaimable"]
            .as_str()
            .and_then(|s| parse_size(s.split(' ').next().unwrap_or("")))
            .unwrap_or(0);
        match v["Type"].as_str().unwrap_or("") {
            "Images" => r.images = size,
            "Build Cache" => r.build = size,
            "Local Volumes" => r.volumes = size,
            _ => {}
        }
    }
    r
}

/// What docker could free now, or None when it doesn't answer within 20
/// seconds (not running, or hung on a full disk).
pub fn docker_reclaim() -> Option<DockerReclaim> {
    output(
        &["docker", "system", "df", "--format", "{{json .}}"],
        Duration::from_secs(20),
    )
    .map(|t| parse_docker_df(&t))
}

// ---- colima ----

/// colima's VM disk: the file as macOS holds it (allocated), and the
/// filesystem docker uses in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VmDisk {
    pub file: i64,
    pub size: i64,
    pub used: i64,
    pub avail: i64,
}

impl VmDisk {
    /// What a trim gives back to macOS: the file beyond what's used.
    pub fn slack(&self) -> i64 {
        (self.file - self.used).max(0)
    }

    /// Nearly full inside: under 5% or 2G left, where docker hangs.
    pub fn full(&self) -> bool {
        self.size > 0 && (self.avail < self.size / 20 || self.avail < 2 << 30)
    }
}

/// `df -k`'s last line read: size, used and available, in bytes.
pub fn parse_df(text: &str) -> Option<(i64, i64, i64)> {
    let l = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let f: Vec<&str> = l.split_whitespace().collect();
    let k = |i: usize| f.get(i)?.parse::<i64>().ok().map(|n| n * 1024);
    Some((k(1)?, k(2)?, k(3)?))
}

/// The default colima profile's VM disk file, under home.
pub fn colima_disk_file(home: &str) -> String {
    format!(
        "{}/.colima/_lima/_disks/colima/datadisk",
        home.trim_end_matches('/')
    )
}

/// What a file takes on disk (its blocks, as du counts them), 0 if gone.
fn allocated(p: &str) -> i64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p).map_or(0, |m| m.blocks() as i64 * 512)
}

/// colima's VM disk, measured: None when there's none, or colima doesn't
/// answer within 20 seconds (stopped).
pub fn colima_disk(home: &str) -> Option<VmDisk> {
    let file = allocated(&colima_disk_file(home));
    if file == 0 {
        return None;
    }
    let text = output(
        &["colima", "ssh", "--", "df", "-k", "/var/lib/docker"],
        Duration::from_secs(20),
    )?;
    let (size, used, avail) = parse_df(&text)?;
    Some(VmDisk {
        file,
        size,
        used,
        avail,
    })
}

/// colima's VM disk files under home, each profile's, and how much each
/// takes on macOS: what the home map counts for ~/.colima, without walking
/// it.
pub fn colima_files(home: &str) -> i64 {
    let d = format!("{}/.colima/_lima/_disks", home.trim_end_matches('/'));
    std::fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .map(|e| allocated(&e.path().join("datadisk").to_string_lossy()))
                .sum()
        })
        .unwrap_or(0)
}

// ---- The home folder's map ----

/// From this size, a folder untouched for UNTOUCHED is listed.
pub const BIG: i64 = 1 << 30;
/// How long a big folder sits untouched before it's listed.
pub const UNTOUCHED: Ms = 30 * DAY;
/// How long a map is used before the home folder is walked again.
pub const MAP_FOR: Ms = 6 * HOUR;
/// At most so many big untouched folders are listed.
const LISTED: usize = 12;
/// How deep below home the walk records folders.
const DEEPEST: usize = 5;

/// Your own folders: never walked, never a rule.
const PERSONAL: [&str; 6] = [
    "Documents",
    "Desktop",
    "Downloads",
    "Movies",
    "Music",
    "Pictures",
];

/// Something big and untouched no rule names, and a guess at what it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Untouched {
    pub path: String,
    pub size: i64,
    pub newest: Ms,
    /// "logs", "a cache", "scratch", or "" when its name doesn't say.
    pub kind: String,
}

/// The home folder as last walked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeMap {
    pub at: Ms,
    /// Each folder at the top of home, and what it takes, biggest first
    /// (~/.colima as its VM disks take on macOS).
    pub tops: Vec<(String, i64)>,
    pub untouched: Vec<Untouched>,
}

/// What a folder is, by its name and its parent's: logs, a cache,
/// scratch, or "" when they don't say.
pub fn classify(path: &str) -> &'static str {
    let p = Path::new(path);
    let name = |q: Option<&Path>| {
        q.and_then(|q| q.file_name())
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    };
    let (n, parent) = (name(Some(p)), name(p.parent()));
    let is = |w: &[&str]| {
        w.iter()
            .any(|w| n == *w || n.trim_start_matches('.') == *w || n.ends_with(&format!("-{w}")))
    };
    if is(&["log", "logs"]) || n.ends_with(".log") || parent == "logs" {
        "logs"
    } else if is(&["cache", "caches", "_cacache"]) || parent == "caches" || parent == ".cache" {
        "a cache"
    } else if is(&["tmp", "temp", "scratch"]) {
        "scratch"
    } else {
        ""
    }
}

/// Whether the walk goes into a folder of home, by where it is: never what
/// rush never touches, your own folders, Library but its Caches and Logs,
/// the Trash.
fn walked(rel: &str) -> bool {
    let top = rel.split('/').next().unwrap_or("");
    if PERSONAL.contains(&top) || top == ".Trash" {
        return false;
    }
    if top == "Library" {
        return rel == "Library"
            || rel.starts_with("Library/Caches")
            || rel.starts_with("Library/Logs");
    }
    true
}

/// Whether a folder is a git checkout (its .git a folder or, a worktree's,
/// a file): its own business, and Clean up's worktrees.
fn checkout(p: &Path) -> bool {
    std::fs::symlink_metadata(p.join(".git")).is_ok()
}

/// A folder walked: its size and newest change, recorded when it's at most
/// DEEPEST below home and big.
struct Walk<'a> {
    home: &'a str,
    seen: Vec<(String, i64, Ms, usize)>,
}

impl Walk<'_> {
    fn dir(&mut self, p: &Path, depth: usize) -> (i64, Ms) {
        use std::os::unix::fs::MetadataExt;
        let ps = p.to_string_lossy();
        let rel = ps
            .strip_prefix(self.home)
            .unwrap_or(&ps)
            .trim_start_matches('/')
            .to_string();
        if depth > 0
            && (!walked(&rel)
                || checkout(p)
                || never_touch(&ps, self.home)
                    .is_some_and(|n| n == ps || ps.starts_with(&format!("{n}/"))))
        {
            return (0, 0);
        }
        let Ok(rd) = std::fs::read_dir(p) else {
            return (0, 0);
        };
        let (mut size, mut newest) = (0i64, 0);
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                let (s, n) = self.dir(&e.path(), depth + 1);
                size += s;
                newest = newest.max(n);
            } else {
                size += m.blocks() as i64 * 512;
                newest = newest.max(m.mtime() * 1000);
            }
        }
        if (1..=DEEPEST).contains(&depth) && (size >= BIG || depth == 1) {
            self.seen.push((ps.into_owned(), size, newest, depth));
        }
        (size, newest)
    }
}

/// Whether a rule names path, or something in it or holding it.
fn ruled(path: &str, rules: &[CacheRule]) -> bool {
    rules.iter().any(|r| {
        path == r.path
            || path.starts_with(&format!("{}/", r.path))
            || r.path.starts_with(&format!("{path}/"))
    })
}

/// The home map from a walk's folders: the top ones by size, and the big
/// untouched ones no rule names, outermost only, biggest first.
fn map_of(
    seen: Vec<(String, i64, Ms, usize)>,
    rules: &[CacheRule],
    home: &str,
    colima: i64,
    now: Ms,
) -> HomeMap {
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let mut tops: Vec<(String, i64)> = seen
        .iter()
        .filter(|s| s.3 == 1 && base(&s.0) != ".colima")
        .map(|s| (base(&s.0), s.1))
        .collect();
    if colima > 0 {
        tops.push((".colima".into(), colima));
    }
    tops.retain(|t| t.1 > 0);
    tops.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut cand: Vec<&(String, i64, Ms, usize)> = seen
        .iter()
        .filter(|s| {
            s.3 >= 2
                && s.1 >= BIG
                && now - s.2 >= UNTOUCHED
                && !ruled(&s.0, rules)
                && may_be_yours(&s.0, home)
        })
        .collect();
    // Outermost first: one inside another listed isn't listed again.
    cand.sort_by_key(|s| s.3);
    let mut out: Vec<Untouched> = Vec::new();
    for s in cand {
        if out.iter().any(|o| s.0.starts_with(&format!("{}/", o.path))) {
            continue;
        }
        out.push(Untouched {
            path: s.0.clone(),
            size: s.1,
            newest: s.2,
            kind: classify(&s.0).into(),
        });
    }
    out.sort_by(|a, b| b.size.cmp(&a.size).then(a.path.cmp(&b.path)));
    out.truncate(LISTED);
    HomeMap {
        at: now,
        tops,
        untouched: out,
    }
}

/// Walks home and maps it. Off the UI: it reads every folder it may.
pub fn map_home(home: &str, rules: &[CacheRule], now: Ms) -> HomeMap {
    let h = home.trim_end_matches('/');
    if h.is_empty() {
        return HomeMap::default();
    }
    let mut w = Walk {
        home: h,
        seen: Vec::new(),
    };
    w.dir(Path::new(h), 0);
    map_of(w.seen, rules, h, colima_files(h), now)
}

fn map_path() -> std::path::PathBuf {
    rush_state::cache_path("diskmap.json")
}

/// The last map, if one was kept.
pub fn load_map() -> Option<HomeMap> {
    serde_json::from_slice(&std::fs::read(map_path()).ok()?).ok()
}

/// Keeps the map for the next Clean up.
pub fn save_map(m: &HomeMap) {
    if let Ok(b) = serde_json::to_vec(m) {
        let _ = rush_state::write_bytes(&map_path(), &b);
    }
}

/// The kept map while it's younger than MAP_FOR, else home walked afresh
/// and kept. Off the UI.
pub fn home_map(rules: &[CacheRule], now: Ms) -> HomeMap {
    if let Some(mut m) = load_map()
        && now - m.at < MAP_FOR
    {
        // A rule made since names what was listed.
        m.untouched.retain(|u| !ruled(&u.path, rules));
        return m;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let m = map_home(&home, rules, now);
    save_map(&m);
    m
}

/// Where a rule you make in Clean up may be: a folder at least two below
/// home, by its text, never what rush never touches, your own folders,
/// Library but its Caches and Logs, nor rush's own folders.
pub fn may_be_yours(path: &str, home: &str) -> bool {
    let h = home.trim_end_matches('/');
    if h.is_empty() || crate::worktree::clean(path) != path || path.contains("/../") {
        return false;
    }
    let Some(rel) = path.strip_prefix(&format!("{h}/")) else {
        return false;
    };
    let own = [rush_state::dir(), rush_state::cache_path("")]
        .iter()
        .any(|d| {
            let d = d.to_string_lossy();
            let d = d.trim_end_matches('/');
            !d.is_empty()
                && (path == d
                    || path.starts_with(&format!("{d}/"))
                    || d.starts_with(&format!("{path}/")))
        });
    rel.split('/').count() >= 2 && walked(rel) && never_touch(path, h).is_none() && !own
}

/// What docker and colima could give back, for the commands' rows: by the
/// command each is.
pub fn command_sizes(home: &str) -> (Option<DockerReclaim>, Option<VmDisk>) {
    // Both at once: each may wait its 20 seconds.
    std::thread::scope(|s| {
        let d = s.spawn(docker_reclaim);
        let c = crate::sweep::colima(home)
            .then(|| colima_disk(home))
            .flatten();
        (d.join().ok().flatten(), c)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_docker_sizes() {
        for (s, want) in [
            ("15.09GB", Some(15_090_000_000)),
            ("273.3MB", Some(273_300_000)),
            ("839.1kB", Some(839_100)),
            ("0B", Some(0)),
            ("1.2TB", Some(1_200_000_000_000)),
            ("x", None),
            ("", None),
        ] {
            assert_eq!(parse_size(s), want, "{s}");
        }
    }

    #[test]
    fn reads_docker_df() {
        let t = r#"{"Active":"2","Reclaimable":"15.09GB (61%)","Size":"24.59GB","TotalCount":"40","Type":"Images"}
{"Active":"1","Reclaimable":"109.8MB (13%)","Size":"839.1MB","TotalCount":"10","Type":"Containers"}
{"Active":"9","Reclaimable":"273.3MB (13%)","Size":"2.066GB","TotalCount":"15","Type":"Local Volumes"}
{"Active":"0","Reclaimable":"1.757GB","Size":"3.148GB","TotalCount":"117","Type":"Build Cache"}
not json"#;
        assert_eq!(
            parse_docker_df(t),
            DockerReclaim {
                images: 15_090_000_000,
                build: 1_757_000_000,
                volumes: 273_300_000,
            }
        );
    }

    #[test]
    fn reads_df_and_vm_slack() {
        let t = "Filesystem     1K-blocks     Used Available Use% Mounted on\n/dev/vdb1       82000000 78643200    451584 100% /var/lib/docker\n";
        let (size, used, avail) = parse_df(t).unwrap();
        assert_eq!(
            (size, used, avail),
            (82000000 * 1024, 78643200 * 1024, 451584 * 1024)
        );
        let full = VmDisk {
            file: 80 << 30,
            size,
            used,
            avail,
        };
        assert!(full.full());
        let trimmed = VmDisk {
            file: 75 << 30,
            used: 26 << 30,
            size: 79 << 30,
            avail: 50 << 30,
        };
        assert_eq!(trimmed.slack(), 49 << 30);
        assert!(!trimmed.full());
        assert_eq!(parse_df(""), None);
    }

    #[test]
    fn guesses_what_a_folder_is() {
        for (p, want) in [
            ("/Users/me/.kanban-code/logs", "logs"),
            ("/Users/me/.langwatch/portless/logs", "logs"),
            ("/Users/me/.x/app-logs", "logs"),
            ("/Users/me/.x/logs/2026-09", "logs"),
            ("/Users/me/.x/server.log", "logs"),
            ("/Users/me/Library/Caches/h2recomp", "a cache"),
            ("/Users/me/.cache/huggingface", "a cache"),
            ("/Users/me/.npm/_cacache", "a cache"),
            ("/Users/me/.foo/tmp", "scratch"),
            ("/Users/me/.foo/blobs", ""),
            ("/Users/me/.foo/catalog", ""),
        ] {
            assert_eq!(classify(p), want, "{p}");
        }
    }

    #[test]
    fn where_your_rules_may_be() {
        let h = "/Users/me";
        for (p, want) in [
            ("/Users/me/.kanban-code/logs", true),
            ("/Users/me/Library/Caches/h2recomp", true),
            ("/Users/me/Library/Logs/x", true),
            ("/Users/me/.kanban-code", false),
            ("/Users/me", false),
            ("/Users/me/Documents/old", false),
            ("/Users/me/Downloads/big", false),
            ("/Users/me/Library/Mail/x", false),
            ("/Users/me/Library/Application Support/x", false),
            ("/Users/me/.claude/projects", false),
            ("/Users/me/.colima/_lima", false),
            ("/Users/me/.x/../Documents/y", false),
            ("/Users/other/.x/logs", false),
            ("/tmp/x/logs", false),
        ] {
            assert_eq!(may_be_yours(p, h), want, "{p}");
        }
    }

    // A walk's folders mapped: the top ones by size, colima's by its disk
    // files; the big untouched ones no rule names, outermost only.
    #[test]
    fn maps_a_walk() {
        let h = "/Users/me";
        let now = 400 * DAY;
        let old = now - 60 * DAY;
        let seen = vec![
            ("/Users/me/.kanban-code".to_string(), 9 << 30, now, 1),
            (
                "/Users/me/.kanban-code/old-logs".to_string(),
                9 << 30,
                old,
                2,
            ),
            ("/Users/me/.foo".to_string(), 5 << 30, old, 1),
            ("/Users/me/.foo/a".to_string(), 3 << 30, old, 2),
            ("/Users/me/.foo/a/b".to_string(), 2 << 30, old, 3),
            ("/Users/me/.foo/new".to_string(), 2 << 30, now - DAY, 2),
            ("/Users/me/.cache".to_string(), 4 << 30, old, 1),
            ("/Users/me/.cache/vitest".to_string(), 4 << 30, old, 2),
            ("/Users/me/.small".to_string(), 1 << 20, old, 1),
        ];
        let rules = crate::rules::rules_with(Vec::new(), h);
        let m = map_of(seen, &rules, h, 24 << 30, now);
        assert_eq!(
            m.tops,
            [
                (".colima".to_string(), 24 << 30),
                (".kanban-code".to_string(), 9 << 30),
                (".foo".to_string(), 5 << 30),
                (".cache".to_string(), 4 << 30),
                (".small".to_string(), 1 << 20),
            ]
        );
        let got: Vec<(&str, &str)> = m
            .untouched
            .iter()
            .map(|u| (u.path.as_str(), u.kind.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("/Users/me/.kanban-code/old-logs", "logs"),
                ("/Users/me/.foo/a", ""),
            ],
            "vitest has a rule; .foo/new was touched; .foo/a/b is in .foo/a"
        );
    }
}
