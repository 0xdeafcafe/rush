//! Rush's own cleanup, by rules and no agent: how anything a cleanup
//! removes is removed, what it never touches, the commands every run ends
//! with, and what Clean up measures to decide. Rust-only: Go had none.
//!
//! - remove_tree is the one removal every cleanup path goes through. It
//!   refuses NEVER_TOUCH (and anything holding one of them), checked on the
//!   path as given and as it resolves, before anything goes; a folder it
//!   can't empty for its modes (a scratch folder of mode 300, dotslash's
//!   read-only .app bundles, Go's read-only module files) is made the
//!   owner's to write and read, as `chmod -R u+rwx` would, and removed
//!   again.
//! - NEVER_TOUCH's biggest are measured (never_touch_big) and listed for
//!   you to decide on, never ticked.
//! - end_commands: pnpm store prune once, and docker's dangling images and
//!   build cache when docker is there (with colima, the space comes back
//!   only once its VM trims its disk).
//! - survey: the rules' caches, each worktree's build caches and the facts
//!   that decide them (the newest change outside node_modules, whether a
//!   process works there), measured off the UI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rush_core::time::Ms;

use crate::rules::{CacheRule, FoundCache, How, measure_caches};

pub const HOUR: Ms = 3_600_000;
pub const DAY: Ms = 24 * HOUR;

/// Folders under the home folder rush never removes, nor anything in them,
/// whatever asks: state, sessions, VMs and models that aren't made again.
pub const NEVER_TOUCH: [&str; 8] = [
    ".colima",
    ".codex/sessions",
    ".claude",
    "Library/Application Support",
    "Library/Containers",
    "Library/Group Containers",
    "Pictures",
    ".ollama",
];

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The NEVER_TOUCH folder path is, is in, or holds, for home; None when
/// it's none of them. With no home every path is refused.
pub fn never_touch(path: &str, home: &str) -> Option<String> {
    let h = home.trim_end_matches('/');
    if h.is_empty() {
        return Some("/".into());
    }
    let p = crate::worktree::clean(path);
    NEVER_TOUCH.iter().map(|n| format!("{h}/{n}")).find(|r| {
        p == *r || p.starts_with(&format!("{r}/")) || r.starts_with(&format!("{p}/")) || p == "/"
    })
}

/// path with its folder resolved (links followed), its own name as is.
fn resolved(path: &str) -> String {
    let p = Path::new(path);
    match (p.parent().map(std::fs::canonicalize), p.file_name()) {
        (Some(Ok(d)), Some(n)) => d.join(n).to_string_lossy().into_owned(),
        _ => path.to_string(),
    }
}

/// Removes path, a file or a folder and all in it, links not followed:
/// refused when it is, is in, or holds a NEVER_TOUCH folder, as given or
/// resolved. A folder that can't be emptied (Permission denied: a folder
/// of mode 300, read-only bundles) is made the owner's to read and write
/// throughout, then removed again. Gone already is done.
pub fn remove_tree(path: &str) -> Result<(), String> {
    let home = home();
    if !path.starts_with('/') {
        return Err(format!("won't delete {path}: not an absolute path"));
    }
    // The home folder as given and resolved (/var is /private/var).
    let real_home = std::fs::canonicalize(&home)
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| home.clone());
    for p in [path.to_string(), resolved(path)] {
        for h in [&home, &real_home] {
            if let Some(r) = never_touch(&p, h) {
                return Err(format!("won't delete {path}: rush never touches {r}"));
            }
        }
    }
    let p = Path::new(path);
    let gone = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    let m = match std::fs::symlink_metadata(p) {
        Err(e) if gone(&e) => return Ok(()),
        Err(e) => return Err(e.to_string()),
        Ok(m) => m,
    };
    if !m.is_dir() {
        return std::fs::remove_file(p)
            .or_else(|e| if gone(&e) { Ok(()) } else { Err(e.to_string()) });
    }
    match std::fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if gone(&e) => Ok(()),
        Err(_) => {
            own_writable(p);
            std::fs::remove_dir_all(p)
                .or_else(|e| if gone(&e) { Ok(()) } else { Err(e.to_string()) })
        }
    }
}

/// chmod -R u+rwx on folders and u+w on files under root, links neither
/// followed nor changed.
fn own_writable(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(m) = std::fs::symlink_metadata(&d) else {
            continue;
        };
        if m.file_type().is_symlink() {
            continue;
        }
        let mode = m.permissions().mode();
        if m.is_dir() {
            if mode & 0o700 != 0o700 {
                let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(mode | 0o700));
            }
            if let Ok(rd) = std::fs::read_dir(&d) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else if mode & 0o200 == 0 {
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(mode | 0o200));
        }
    }
}

/// Every folder a process of this user works in (its cwd), "/" left out.
/// Off the UI: it reads the process table.
pub fn process_cwds() -> Vec<String> {
    let t = rush_sys::proc::snapshot();
    let mut out: Vec<String> = t
        .procs
        .keys()
        .filter_map(|&pid| rush_sys::proc::cwd(pid))
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| p != "/")
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether any of cwds is dir or in it.
pub fn works_in(cwds: &[String], dir: &str) -> bool {
    let d = crate::worktree::clean(dir);
    cwds.iter()
        .any(|c| *c == d || c.starts_with(&d) && c.as_bytes().get(d.len()) == Some(&b'/'))
}

/// When anything under dir last changed, folders named in skip (and what's
/// in them) left out, links not followed.
pub fn newest_outside(dir: &str, skip: &[&str]) -> Ms {
    let mut newest = 0;
    let mut stack = vec![PathBuf::from(dir)];
    while let Some(q) = stack.pop() {
        let Ok(m) = std::fs::symlink_metadata(&q) else {
            continue;
        };
        if let Ok(t) = m.modified() {
            newest = newest.max(crate::temp::ms_of(t));
        }
        if m.is_dir()
            && let Ok(rd) = std::fs::read_dir(&q)
        {
            for e in rd.flatten() {
                let skipped = skip.iter().any(|s| e.file_name() == *s)
                    && e.file_type().is_ok_and(|t| t.is_dir());
                if !skipped {
                    stack.push(e.path());
                }
            }
        }
    }
    newest
}

/// How long, in Clean up's words: "3 days", "5h", "20m".
pub fn span(ms: Ms) -> String {
    let (m, h, d) = (ms / 60_000, ms / HOUR, ms / DAY);
    if d >= 1 {
        format!("{d} day{}", if d == 1 { "" } else { "s" })
    } else if h >= 1 {
        format!("{h}h")
    } else {
        format!("{}m", m.max(0))
    }
}

// ---- Commands ----

pub const PNPM_PRUNE: &[&str] = &["pnpm", "store", "prune"];
pub const DOCKER_IMAGE_PRUNE: &[&str] = &["docker", "image", "prune", "-f"];
pub const DOCKER_BUILDER_PRUNE: &[&str] = &["docker", "builder", "prune", "-f"];

/// Where a tool is: on PATH, or where package managers put them (rush
/// started from the menu bar has a short PATH).
pub fn find_tool(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let h = home();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for d in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/local/go/bin",
        "/Applications/Docker.app/Contents/Resources/bin",
    ] {
        dirs.push(d.into());
    }
    if !h.is_empty() {
        for d in ["Library/pnpm", ".local/bin", "go/bin", ".docker/bin"] {
            dirs.push(Path::new(&h).join(d));
        }
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Whether docker here runs in colima's VM, whose disk gives space back
/// to macOS only once the VM trims it.
pub fn colima(home: &str) -> bool {
    !home.is_empty() && Path::new(home).join(".colima").is_dir()
}

/// The commands every Clean up ends with, each as a ticked line of no
/// known size: pnpm store prune when pnpm is there, then docker's
/// dangling images and build cache when docker is. has says whether a
/// tool is there.
pub fn end_commands(has: &dyn Fn(&str) -> bool, home: &str) -> Vec<FoundCache> {
    let cmd = |args: &'static [&'static str], why: &str, note: &str| FoundCache {
        path: args.join(" "),
        why: why.into(),
        how: How::Run(args),
        ok: true,
        note: note.into(),
        ..FoundCache::default()
    };
    let mut out = Vec::new();
    if has("pnpm") {
        out.push(cmd(
            PNPM_PRUNE,
            "drops what pnpm's store holds that nothing links to any more",
            "once, after node_modules go: what frees their hard links",
        ));
    }
    if has("docker") {
        let trim = if colima(home) {
            "colima: the space comes back to macOS only once its VM trims its disk"
        } else {
            "never volumes or containers"
        };
        out.push(cmd(DOCKER_IMAGE_PRUNE, "docker's dangling images", trim));
        out.push(cmd(DOCKER_BUILDER_PRUNE, "docker's build cache", trim));
    }
    out
}

/// Runs one of end_commands' commands (or a dry run writes it down): what
/// it said last, or why it failed. Within ten minutes.
pub fn run_command(args: &[&str], why: &str) -> Result<String, String> {
    let line = args.join(" ");
    if rush_core::dryrun::would("run-cleanup-command", &line, why) {
        return Ok(String::new());
    }
    let Some((name, rest)) = args.split_first() else {
        return Err("no command".into());
    };
    let Some(tool) = find_tool(name) else {
        return Err(format!("{line}: {name} isn't installed"));
    };
    let mut cmd = std::process::Command::new(tool);
    cmd.args(rest);
    let r = rush_sys::cmd::output_within(&mut cmd, std::time::Duration::from_secs(600))
        .map_err(|e| format!("{line}: {e}"))
        .and_then(|o| {
            if o.ok() {
                Ok(String::from_utf8_lossy(&o.out)
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string())
            } else {
                Err(format!("{line}: {}", o.err_text()))
            }
        });
    crate::temp::agent_event(
        None,
        "cleanup-command",
        serde_json::json!({"command": line, "why": why, "err": crate::temp::err_text(&r)}),
    );
    r
}

// ---- What rush never touches, measured for you ----

/// What each NEVER_TOUCH folder is, in a few words.
fn never_why(n: &str) -> &'static str {
    match n {
        ".colima" => "colima's VM disks",
        ".codex/sessions" => "Codex's sessions",
        ".claude" => "Claude Code's sessions and settings",
        "Library/Application Support" => "an app's state",
        "Library/Containers" => "a sandboxed app's data",
        "Library/Group Containers" => "apps' shared data",
        "Pictures" => "your pictures",
        ".ollama" => "Ollama's models",
        _ => "",
    }
}

/// The biggest things in NEVER_TOUCH's folders, each min or more, at most
/// n, biggest first: for you to decide on, never ticked (ok false). Each
/// is a folder's child, sized as du does; newest is its own change.
pub fn never_touch_big(home: &str, min: i64, n: usize) -> Vec<FoundCache> {
    let h = home.trim_end_matches('/');
    if h.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for root in NEVER_TOUCH {
        let Ok(rd) = std::fs::read_dir(format!("{h}/{root}")) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.file_type().is_symlink() {
                continue;
            }
            let path = e.path().to_string_lossy().into_owned();
            let size = crate::temp::disk_usage(&[rush_core::adapter::TempDir {
                path: path.clone(),
                keep: false,
            }]);
            if size < min {
                continue;
            }
            out.push(FoundCache {
                path,
                why: never_why(root).into(),
                size,
                newest: m.modified().map(crate::temp::ms_of).unwrap_or(0),
                note: format!("rush never touches ~/{root}: yours to decide"),
                ..FoundCache::default()
            });
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size).then(a.path.cmp(&b.path)));
    out.truncate(n);
    out
}

// ---- Clean up's survey ----

/// A cache Clean up offers: a folder a cleanup rule names, a command
/// (How::Run), or a build cache in a worktree nobody works in (wt is that
/// worktree).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheFound {
    pub found: FoundCache,
    pub wt: String,
}

impl CacheFound {
    pub fn node_modules(&self) -> bool {
        self.found.path.ends_with("/node_modules")
    }
}

/// What decides a worktree's caches, and its class (triage): when anything
/// outside its node_modules last changed, and whether a process works in
/// it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WtFacts {
    pub newest_outside: Ms,
    pub works_here: bool,
}

/// How long a worktree sits untouched, outside node_modules, before its
/// build caches go.
pub const WT_IDLE: Ms = DAY;

/// The facts of each worktree at paths, read now. Off the UI.
pub fn wt_facts(paths: &[String], cwds: &[String]) -> HashMap<String, WtFacts> {
    paths
        .iter()
        .map(|p| {
            (
                p.clone(),
                WtFacts {
                    newest_outside: newest_outside(p, &["node_modules"]),
                    works_here: works_in(cwds, p),
                },
            )
        })
        .collect()
}

/// A worktree's build cache, decided: yes once the worktree is untouched
/// outside node_modules for WT_IDLE and no process works in it.
pub fn wt_cache(path: String, wt: &str, size: i64, newest: Ms, f: WtFacts, now: Ms) -> CacheFound {
    let idle = now - f.newest_outside;
    let (ok, note) = if f.works_here {
        (false, "a process works in its worktree".to_string())
    } else if idle < WT_IDLE {
        (
            false,
            format!("its worktree was touched {} ago", span(idle)),
        )
    } else {
        (
            true,
            format!("its worktree untouched for {}, nobody in it", span(idle)),
        )
    };
    CacheFound {
        found: FoundCache {
            path,
            why: "build cache in a worktree nobody works in".into(),
            size,
            newest,
            ok,
            note,
            ..FoundCache::default()
        },
        wt: wt.to_string(),
    }
}

/// What Clean up measured: the caches (the rules', then the worktrees',
/// then the commands that end every run), each worktree's facts, and what
/// rush never touches, biggest first.
#[derive(Clone, Debug, Default)]
pub struct Survey {
    pub caches: Vec<CacheFound>,
    pub facts: HashMap<String, WtFacts>,
}

/// Measures what Clean up offers: the rules' caches, the build caches of
/// the worktrees in free (nobody's, not going whole), and the facts of
/// every worktree in wts. Off the UI: it walks each.
pub fn survey(rules: &[CacheRule], wts: &[String], free: &[String], now: Ms) -> Survey {
    let cwds = process_cwds();
    let mut all: Vec<String> = wts.to_vec();
    all.extend(free.iter().filter(|f| !wts.contains(f)).cloned());
    let facts = wt_facts(&all, &cwds);
    let mut caches: Vec<CacheFound> = measure_caches(rules, now)
        .into_iter()
        .map(|found| CacheFound {
            found,
            wt: String::new(),
        })
        .collect();
    for wt in free {
        let f = facts.get(wt).copied().unwrap_or_default();
        for p in crate::caches_in(wt) {
            let (size, newest) = crate::walk(&p);
            if size > 0 {
                caches.push(wt_cache(p, wt, size, newest, f, now));
            }
        }
    }
    let h = home();
    caches.extend(
        end_commands(&|t| find_tool(t).is_some(), &h)
            .into_iter()
            .map(|found| CacheFound {
                found,
                wt: String::new(),
            }),
    );
    Survey { caches, facts }
}

/// Removes what Clean up offered, as it was decided: a command run, a
/// rule's folder through remove_rule_cache, a worktree's build cache
/// through remove_cache (refused when it changed since it was measured, or
/// a process works in its worktree now). It says how much it freed, by
/// what was measured (a command's 0).
pub fn remove_found(
    c: &CacheFound,
    rules: &[CacheRule],
    cwds: &[String],
    why: &str,
) -> Result<i64, String> {
    if let How::Run(args) = c.found.how {
        return run_command(args, why).map(|_| 0);
    }
    if c.wt.is_empty() {
        return crate::remove_rule_cache(&c.found.path, rules, why);
    }
    if works_in(cwds, &c.wt) {
        return Err(format!(
            "{}: a process works in its worktree now; it stays",
            c.found.path
        ));
    }
    crate::remove_cache(
        &c.found.path,
        std::slice::from_ref(&c.wt),
        c.found.newest + 1,
        why,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, UNIX_EPOCH};

    const H: &str = "/Users/me";
    const NOW: Ms = 1_793_190_896_789;

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn mode(p: &Path, m: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    }

    fn touch(p: &Path, at: Ms) {
        let t = UNIX_EPOCH + Duration::from_millis(at as u64);
        std::fs::File::open(p)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(t).set_accessed(t))
            .unwrap();
    }

    #[test]
    fn what_rush_never_touches() {
        for (p, want) in [
            ("/Users/me/.claude", Some("/Users/me/.claude")),
            ("/Users/me/.claude/projects/x", Some("/Users/me/.claude")),
            (
                "/Users/me/.codex/sessions/2026",
                Some("/Users/me/.codex/sessions"),
            ),
            ("/Users/me/.codex/cache", None),
            (
                "/Users/me/.colima/_lima/colima/diffdisk",
                Some("/Users/me/.colima"),
            ),
            ("/Users/me/.ollama/models", Some("/Users/me/.ollama")),
            (
                "/Users/me/Pictures/Photos Library.photoslibrary",
                Some("/Users/me/Pictures"),
            ),
            (
                "/Users/me/Library/Application Support/Code",
                Some("/Users/me/Library/Application Support"),
            ),
            (
                "/Users/me/Library/Containers/x",
                Some("/Users/me/Library/Containers"),
            ),
            (
                "/Users/me/Library/Group Containers/x",
                Some("/Users/me/Library/Group Containers"),
            ),
            (
                "/Users/me/Library/Caches/x/../../Containers/y",
                Some("/Users/me/Library/Containers"),
            ),
            // What holds one of them.
            (
                "/Users/me/Library",
                Some("/Users/me/Library/Application Support"),
            ),
            ("/Users/me", Some("/Users/me/.colima")),
            ("/", Some("/Users/me/.colima")),
            ("/Users/me/Library/Caches/go-build", None),
            ("/Users/me/src/r/.claude/worktrees/a", None),
            ("/Users/me/.claudette", None),
        ] {
            assert_eq!(never_touch(p, H).as_deref(), want, "{p}");
        }
        assert!(never_touch("/tmp/x", "").is_some(), "no home: nothing");
    }

    // A scratch folder of mode 300 (no reading it) and a read-only bundle
    // both go: made the owner's to write first. Links aren't followed.
    #[test]
    fn removes_what_permissions_hold_back() {
        let d = tempfile::tempdir().unwrap();
        let keep = d.path().join("keep");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("f"), "x").unwrap();
        mode(&keep.join("f"), 0o444);
        let scratch = d.path().join("s");
        let block = scratch.join("nlpgo-codeblock-1");
        std::fs::create_dir_all(block.join("inner")).unwrap();
        std::fs::write(block.join("inner").join("f"), "x").unwrap();
        std::os::unix::fs::symlink(&keep, block.join("link")).unwrap();
        mode(&block.join("inner"), 0o300);
        mode(&block, 0o300);
        let app = d.path().join("dotslash").join("Tool.app").join("Contents");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("bin"), "x").unwrap();
        mode(&app.join("bin"), 0o444);
        mode(&app, 0o555);
        mode(app.parent().unwrap(), 0o555);
        for p in [&scratch, &d.path().join("dotslash")] {
            remove_tree(&s(p)).unwrap();
            assert!(!p.exists(), "{p:?} still there");
        }
        assert!(keep.join("f").exists(), "followed a link");
        assert_eq!(
            std::fs::metadata(keep.join("f"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444,
            "changed what a link points at"
        );
        remove_tree(&s(&d.path().join("gone"))).unwrap();
        assert!(remove_tree("relative/x").is_err());
    }

    // The deny list is checked before anything goes, as given and through
    // a link.
    #[test]
    fn never_removes_what_it_never_touches() {
        let home = home();
        let d = tempfile::tempdir().unwrap();
        for n in NEVER_TOUCH {
            let p = format!("{home}/{n}/x");
            std::fs::create_dir_all(&p).unwrap();
            assert!(remove_tree(&p).is_err(), "{p}");
            assert!(remove_tree(&format!("{home}/{n}")).is_err(), "{n}");
            assert!(Path::new(&p).exists(), "{p} removed");
        }
        assert!(remove_tree(&format!("{home}/Library")).is_err());
        let link = d.path().join("l");
        std::os::unix::fs::symlink(format!("{home}/.claude"), &link).unwrap();
        assert!(remove_tree(&s(&link.join("x"))).is_err(), "through a link");
        assert!(Path::new(&format!("{home}/.claude/x")).exists());
        remove_tree(&s(&link)).unwrap();
        assert!(!link.exists() && Path::new(&format!("{home}/.claude")).exists());
    }

    #[test]
    fn who_works_where() {
        let cwds = vec!["/w/a/apps/web".to_string(), "/w/bb".to_string()];
        for (dir, want) in [
            ("/w/a", true),
            ("/w/a/", true),
            ("/w/b", false),
            ("/w/bb", true),
            ("/w/a/apps/webx", false),
        ] {
            assert_eq!(works_in(&cwds, dir), want, "{dir}");
        }
    }

    #[test]
    fn newest_change_outside_node_modules() {
        let d = tempfile::tempdir().unwrap();
        let wt = d.path().join("wt");
        for p in ["src", "node_modules/x", "apps/web/node_modules/y"] {
            std::fs::create_dir_all(wt.join(p)).unwrap();
        }
        let old = NOW - 3 * DAY;
        for p in [
            "src",
            "apps/web",
            "apps",
            "node_modules/x",
            "node_modules",
            "apps/web/node_modules/y",
            "apps/web/node_modules",
            "",
        ] {
            touch(&wt.join(p), old);
        }
        touch(&wt.join("node_modules/x"), NOW);
        touch(&wt.join("apps/web/node_modules/y"), NOW);
        assert_eq!(newest_outside(&s(&wt), &["node_modules"]), old);
        assert_eq!(newest_outside(&s(&wt), &[]), NOW);
    }

    #[test]
    fn decides_a_worktrees_caches() {
        let f = |ago: Ms, works_here| WtFacts {
            newest_outside: NOW - ago,
            works_here,
        };
        for (facts, ok, note) in [
            (
                f(3 * DAY, false),
                true,
                "its worktree untouched for 3 days, nobody in it",
            ),
            (f(2 * HOUR, false), false, "its worktree was touched 2h ago"),
            (f(3 * DAY, true), false, "a process works in its worktree"),
        ] {
            let c = wt_cache("/w/a/node_modules".into(), "/w/a", 9, 0, facts, NOW);
            assert_eq!((c.found.ok, c.found.note.as_str()), (ok, note));
            assert_eq!(c.wt, "/w/a");
        }
    }

    #[test]
    fn every_run_ends_with_its_commands() {
        let names = |v: Vec<FoundCache>| v.into_iter().map(|c| c.path).collect::<Vec<_>>();
        let d = tempfile::tempdir().unwrap();
        let home = s(d.path());
        let none = |_: &str| false;
        let pnpm = |t: &str| t == "pnpm";
        let all = |_: &str| true;
        assert!(end_commands(&none, &home).is_empty());
        assert_eq!(names(end_commands(&pnpm, &home)), ["pnpm store prune"]);
        assert_eq!(
            names(end_commands(&all, &home)),
            [
                "pnpm store prune",
                "docker image prune -f",
                "docker builder prune -f"
            ]
        );
        assert!(end_commands(&all, &home).iter().all(|c| c.ok));
        assert!(!colima(&home));
        std::fs::create_dir(d.path().join(".colima")).unwrap();
        assert!(colima(&home));
        assert!(end_commands(&all, &home)[1].note.contains("trims its disk"));
    }

    #[test]
    fn lists_the_biggest_it_never_touches() {
        let home = home();
        let models = format!("{home}/.ollama/models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(format!("{models}/blob"), vec![1u8; 256 << 10]).unwrap();
        std::fs::create_dir_all(format!("{home}/Pictures/small")).unwrap();
        let got = never_touch_big(&home, 128 << 10, 50);
        let m = got
            .iter()
            .find(|f| f.path == models)
            .expect("Ollama's models listed");
        assert_eq!(m.why, "Ollama's models");
        assert!(got.iter().all(|f| !f.ok && f.size >= 128 << 10), "{got:?}");
        assert!(
            got.windows(2).all(|w| w[0].size >= w[1].size),
            "biggest first"
        );
        assert!(
            !got.iter().any(|f| f.path.ends_with("/Pictures/small")),
            "a small one"
        );
        assert!(never_touch_big("", 0, 5).is_empty());
    }
}
