//! The files @ offers in a message box: the session folder's files, read
//! off the UI thread and kept per folder, ranked as a file finder ranks
//! them, and how a file tagged reaches the agent. In a git checkout
//! they're `git ls-files` and the untracked files git doesn't ignore;
//! elsewhere a walk that skips hidden folders and build output. Both stop
//! at [`MAX_FILES`], so a big repository never holds the box up. A
//! folder's list is read again when the picker opens after
//! [`STALE_MS`]; meanwhile the last one is offered.
//!
//! A file goes in the box as `@path`, relative to the session's folder,
//! `@"a b/c.rs"` when the path has a space (Claude Code's own form). On
//! sending, a harness that reads `@path` as the file
//! (`Adapter::reads_at_paths`) is sent it as typed; any other the plain
//! path, `"a b/c.rs"` quoted when it has a space.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use rush_core::adapter::Registry;
use rush_core::time::Ms;

use crate::frame::Out;
use crate::model::Model;

/// The most files a folder's list keeps.
pub const MAX_FILES: usize = 20_000;
/// The most files the picker offers for what's typed.
pub const MAX_PICKS: usize = 50;
/// How old a folder's list is before the picker opening reads it again.
pub const STALE_MS: Ms = 10_000;
/// The picker unasked this long has closed: asked again, it's opened.
const REOPEN_MS: Ms = 2_000;
/// The most entries a walk looks at, and how deep it goes.
const WALK_ENTRIES: usize = 50_000;
const WALK_DEPTH: usize = 16;
/// Folders a walk skips besides hidden ones: build output and packages.
const SKIP: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "__pycache__",
    "coverage",
];

/// The files of dir, relative to it: git's when it's a checkout, else a
/// walk's. Reads the disk, so never on the UI thread.
pub fn list_files(dir: &str) -> Vec<String> {
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return Vec::new();
    }
    git_files(dir).unwrap_or_else(|| walk(dir))
}

/// What git has of dir: the files it tracks and the untracked ones it
/// doesn't ignore. None when dir is no checkout.
fn git_files(dir: &str) -> Option<Vec<String>> {
    let mut child = Command::new("git")
        .args([
            "-C",
            dir,
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let mut full = false;
    for part in BufReader::new(out).split(0) {
        let Ok(p) = part else { break };
        if p.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(&p).into_owned();
        if seen.insert(s.clone()) {
            files.push(s);
        }
        if files.len() >= MAX_FILES {
            full = true;
            let _ = child.kill();
            break;
        }
    }
    let ok = child.wait().is_ok_and(|s| s.success());
    (ok || full).then_some(files)
}

/// dir's files, breadth first, skipping hidden folders and build output.
fn walk(dir: &str) -> Vec<String> {
    let root = Path::new(dir);
    let mut out = Vec::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut looked = 0;
    while let Some((d, depth)) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            looked += 1;
            if looked > WALK_ENTRIES || out.len() >= MAX_FILES {
                return out;
            }
            let name = e.file_name();
            let name = name.to_string_lossy();
            let Ok(ft) = e.file_type() else {
                continue;
            };
            let path = e.path();
            if ft.is_dir() {
                if !name.starts_with('.') && !SKIP.contains(&&*name) && depth + 1 < WALK_DEPTH {
                    queue.push_back((path, depth + 1));
                }
                continue;
            }
            if (ft.is_file() || (ft.is_symlink() && path.is_file()))
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    out
}

/// How well path matches q, lower first: q starts its name, is in its
/// name, is in the path, its letters are in its name in order, in the
/// path in order. None when it doesn't match. q is lowercase.
fn score(path: &str, q: &str) -> Option<u8> {
    if q.is_empty() {
        return Some(0);
    }
    let p = path.to_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    if name.starts_with(q) {
        Some(0)
    } else if name.contains(q) {
        Some(1)
    } else if p.contains(q) {
        Some(2)
    } else if in_order(name, q) {
        Some(3)
    } else if in_order(&p, q) {
        Some(4)
    } else {
        None
    }
}

/// Whether q's letters are in s in order.
fn in_order(s: &str, q: &str) -> bool {
    let mut it = s.chars();
    q.chars().all(|c| it.any(|x| x == c))
}

/// The files that match q, as a file finder ranks them: a hit in the
/// file's name first, then in its path, then letters in order; shorter
/// paths first within each. cap at most.
pub fn rank(files: &[String], q: &str, cap: usize) -> Vec<String> {
    let q = q.trim_start_matches('"').to_lowercase();
    let mut hits: Vec<(u8, usize, &String)> = files
        .iter()
        .filter_map(|f| score(f, &q).map(|s| (s, f.chars().count(), f)))
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    hits.truncate(cap);
    hits.into_iter().map(|h| h.2.clone()).collect()
}

/// A folder's files as last read, and the picker's last ask of them.
#[derive(Default)]
struct Listed {
    files: Arc<Vec<String>>,
    read: bool,
    read_at: Ms,
    loading: bool,
    asked_at: Ms,
    /// The last query and its files.
    last: Option<(String, Arc<Vec<String>>)>,
}

/// Every folder's files the pickers have asked for.
#[derive(Default)]
struct Files {
    by_dir: HashMap<String, Listed>,
}

/// The files of dir that match q, best first, MAX_PICKS at most. The
/// first ask, and one as the picker opens again once the list is
/// STALE_MS old, reads dir off the UI thread; until it's read, what was
/// read before (nothing, the first time).
pub fn files_for(m: &mut Model, dir: &str, q: &str) -> Arc<Vec<String>> {
    if dir.is_empty() {
        return Arc::default();
    }
    let now = m.now;
    let l = m.kept::<Files>().by_dir.entry(dir.to_string()).or_default();
    let reopened = now.saturating_sub(l.asked_at) > REOPEN_MS;
    l.asked_at = now;
    let read = !l.loading && (!l.read || (reopened && now.saturating_sub(l.read_at) > STALE_MS));
    if read {
        l.loading = true;
        let d = dir.to_string();
        let key = dir.to_string();
        m.later(
            move || list_files(&d),
            move |m, files| {
                let now = m.now;
                let l = m.kept::<Files>().by_dir.entry(key).or_default();
                l.files = Arc::new(files);
                l.read = true;
                l.read_at = now;
                l.loading = false;
                l.last = None;
                Out::Draw
            },
        );
    }
    let l = m.kept::<Files>().by_dir.entry(dir.to_string()).or_default();
    if let Some((lq, r)) = &l.last
        && lq == q
    {
        return r.clone();
    }
    let r = Arc::new(rank(&l.files, q, MAX_PICKS));
    l.last = Some((q.to_string(), r.clone()));
    r
}

/// What a file's tag holds after the @: its path, in double quotes when
/// it has a space (Claude Code's `@"a b.rs"`).
pub fn file_token(path: &str) -> String {
    if path.chars().any(char::is_whitespace) {
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

/// text as agent kind is sent it from a session in dir: each @ tag of a
/// file there stays as it is for a harness that reads `@path`
/// (Registry::reads_at_paths), and loses its @ for any other, which is
/// sent the plain path (still quoted when it has a space). Tags of
/// anything that isn't a file (an agent's) are left alone.
pub fn send_files(reg: &Registry, kind: &str, dir: &str, text: &str) -> String {
    if dir.is_empty() || !text.contains('@') || reg.reads_at_paths(kind) {
        return text.to_string();
    }
    let is_file = |p: &str| !p.is_empty() && Path::new(dir).join(p).is_file();
    let cs: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let starts = c == '@' && (i == 0 || cs[i - 1].is_whitespace());
        if !starts {
            out.push(c);
            i += 1;
            continue;
        }
        // @"a b.rs": the path is what's quoted.
        if cs.get(i + 1) == Some(&'"')
            && let Some(close) = cs[i + 2..].iter().position(|&c| c == '"')
        {
            let p: String = cs[i + 2..i + 2 + close].iter().collect();
            if is_file(&p) {
                i += 1; // the @ goes; the quoted path stays
                continue;
            }
        }
        // @path: up to the next space, less a sentence's last mark.
        let end = cs[i + 1..]
            .iter()
            .position(|c| c.is_whitespace())
            .map_or(cs.len(), |n| i + 1 + n);
        let word: String = cs[i + 1..end].iter().collect();
        let p = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'', '"']);
        if is_file(p) {
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // A hit in the name beats one in the path, and shorter paths go
    // first.
    #[test]
    fn ranks_as_a_file_finder() {
        let files = s(&[
            "rush-draw/src/lib.rs",
            "rush-convo/src/render/draw.rs",
            "docs/drawing/a.md",
            "x/d/r/a.txt",
            "readme.md",
        ]);
        assert_eq!(
            rank(&files, "dra", 10),
            s(&[
                "rush-convo/src/render/draw.rs",
                "docs/drawing/a.md",
                "rush-draw/src/lib.rs",
                "x/d/r/a.txt",
            ])
        );
        assert_eq!(
            rank(&files, "DRAW.rs", 10),
            s(&["rush-convo/src/render/draw.rs", "rush-draw/src/lib.rs"])
        );
        assert_eq!(rank(&files, "", 2), s(&["readme.md", "x/d/r/a.txt"]));
        assert!(rank(&files, "zzz", 10).is_empty());
    }

    #[test]
    fn quotes_paths_with_spaces() {
        assert_eq!(file_token("src/a.rs"), "src/a.rs");
        assert_eq!(file_token("my docs/a b.md"), "\"my docs/a b.md\"");
    }

    // A walk skips hidden folders and build output.
    #[test]
    fn walks_a_folder_that_isnt_a_checkout() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for f in [
            "a.txt",
            "src/b.rs",
            ".git2/c",
            "node_modules/x/d.js",
            "target/e",
            ".env",
        ] {
            let f = p.join(f);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "").unwrap();
        }
        let mut got = walk(&p.to_string_lossy());
        got.sort();
        assert_eq!(got, s(&[".env", "a.txt", "src/b.rs"]));
    }

    // In a checkout: what git tracks and the untracked files it doesn't
    // ignore.
    #[test]
    fn lists_a_checkout_with_git() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(p)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        for (f, body) in [
            ("kept.rs", ""),
            ("new.rs", ""),
            ("skip.log", ""),
            (".gitignore", "*.log\n"),
        ] {
            std::fs::write(p.join(f), body).unwrap();
        }
        git(&["add", "kept.rs", ".gitignore"]);
        let mut got = list_files(&p.to_string_lossy());
        got.sort();
        assert_eq!(got, s(&[".gitignore", "kept.rs", "new.rs"]));
    }
}
