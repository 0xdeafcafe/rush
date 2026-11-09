//! What recent runs of each command held, so the gate expects what a run
//! needs from what runs like it took (Rust-only: Go rush keeps no history
//! and expects each program's rule figure; see share::estimate).
//!
//! One small file, `history`, in the gate's folder beside the tickets'
//! folder, so Go rush never sees it. A line a command in a project:
//! `<project>\t<command>\t<peak> <peak>…`, the newest peak last, the
//! command used most lately last. Written whole under `history.lock` and
//! renamed into place, so a reader never sees half of it.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::gate::{flock, mkdir_all, write_file};

/// The runs kept per command.
pub const KEEP: usize = 5;

/// The commands kept: past this, the one used least lately goes.
const COMMANDS: usize = 256;

/// The peaks recent runs of command in project held, oldest first; none
/// when there's no history of it.
pub fn peaks(root: &Path, project: &str, command: &str) -> Vec<u64> {
    let (project, command) = (clean(project), clean(command));
    let b = std::fs::read_to_string(root.join("history")).unwrap_or_default();
    b.lines()
        .filter_map(parse)
        .find(|(p, c, _)| *p == project && *c == command)
        .map(|(_, _, peaks)| peaks)
        .unwrap_or_default()
}

/// Adds peak, what a run of command in project held at most, to its
/// history.
pub fn record(root: &Path, project: &str, command: &str, peak: u64) -> io::Result<()> {
    let (project, command) = (clean(project), clean(command));
    mkdir_all(root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(root.join("history.lock"))?;
    flock(&lock, libc::LOCK_EX)?;
    let path = root.join("history");
    let b = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<(String, String, Vec<u64>)> = b
        .lines()
        .filter_map(parse)
        .map(|(p, c, k)| (p.to_string(), c.to_string(), k))
        .collect();
    let mut mine = match lines
        .iter()
        .position(|(p, c, _)| *p == project && *c == command)
    {
        Some(i) => lines.remove(i).2,
        None => Vec::new(),
    };
    mine.push(peak);
    if mine.len() > KEEP {
        mine.drain(..mine.len() - KEEP);
    }
    lines.push((project, command, mine));
    if lines.len() > COMMANDS {
        lines.drain(..lines.len() - COMMANDS);
    }
    let mut out = String::new();
    for (p, c, k) in &lines {
        let k: Vec<String> = k.iter().map(u64::to_string).collect();
        out.push_str(&format!("{p}\t{c}\t{}\n", k.join(" ")));
    }
    let tmp = root.join("history.tmp");
    write_file(&tmp, out.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &path)
}

/// The project a run in dir belongs to, for its history: the main checkout
/// of the git repository dir is in, so a worktree's runs count as the
/// repository's, else dir itself.
pub fn project_of(dir: &Path) -> String {
    for d in dir.ancestors() {
        let git = d.join(".git");
        if git.is_dir() {
            return d.to_string_lossy().into_owned();
        }
        if git.is_file() {
            // A worktree's: "gitdir: <main>/.git/worktrees/<name>".
            let b = std::fs::read_to_string(&git).unwrap_or_default();
            if let Some(to) = b.trim().strip_prefix("gitdir:") {
                let to = d.join(to.trim());
                let to = to.to_string_lossy();
                if let Some((main, _)) = to.split_once("/.git/worktrees/") {
                    return main.to_string();
                }
            }
            return d.to_string_lossy().into_owned();
        }
    }
    dir.to_string_lossy().into_owned()
}

fn parse(l: &str) -> Option<(&str, &str, Vec<u64>)> {
    let mut f = l.splitn(3, '\t');
    let (p, c, k) = (f.next()?, f.next()?, f.next()?);
    let k: Vec<u64> = k.split(' ').filter_map(|n| n.parse().ok()).collect();
    Some((p, c, k))
}

/// s with the file's separators made spaces.
fn clean(s: &str) -> String {
    s.replace(['\t', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // What's recorded is read back per command and project, the last KEEP
    // runs of each.
    #[test]
    fn records_and_reads_peaks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("gate");
        assert_eq!(peaks(&root, "/p", "cargo check"), Vec::<u64>::new());
        for n in 1..=7 {
            record(&root, "/p", "cargo check", n).unwrap();
        }
        record(&root, "/p", "cargo test", 100).unwrap();
        record(&root, "/q", "cargo check", 200).unwrap();
        record(&root, "/p\tx", "go\ntest", 300).unwrap();
        for (project, command, want) in [
            ("/p", "cargo check", vec![3, 4, 5, 6, 7]),
            ("/p", "cargo test", vec![100]),
            ("/q", "cargo check", vec![200]),
            ("/p x", "go test", vec![300]),
            ("/q", "cargo test", vec![]),
        ] {
            assert_eq!(peaks(&root, project, command), want, "{project} {command}");
        }
    }

    // A worktree's runs are its repository's.
    #[test]
    fn projects_fold_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("repo");
        let wt = main.join(".claude/worktrees/a");
        std::fs::create_dir_all(main.join(".git/worktrees/a")).unwrap();
        std::fs::create_dir_all(wt.join("crates/x")).unwrap();
        let gitdir = format!("gitdir: {}\n", main.join(".git/worktrees/a").display());
        std::fs::write(wt.join(".git"), gitdir).unwrap();
        let plain = dir.path().join("plain/sub");
        std::fs::create_dir_all(&plain).unwrap();
        let m = main.to_string_lossy().into_owned();
        for (d, want) in [
            (main.join("crates"), m.clone()),
            (wt.join("crates/x"), m.clone()),
            (wt.clone(), m),
        ] {
            assert_eq!(project_of(&d), want, "{}", d.display());
        }
        // Outside any repository it's the folder itself.
        assert_eq!(project_of(&plain), plain.to_string_lossy());
    }
}
