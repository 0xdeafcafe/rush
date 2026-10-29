//! Copying a conversation for a fork, its file checkpoints with it, and
//! finding a transcript that moved. Port of
//! internal/adapters/claude/claude/forkcopy.go and findindex.go.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use crate::account::Account;
use crate::files::mkdir_private;

/// Writes the first up_to bytes of the conversation at src (all of it
/// when up_to < 0) to dst as a conversation of its own, session_id, ready
/// for claude --resume. The original is left as it is.
pub fn copy_transcript(
    src: &Path,
    dst: &Path,
    old_id: &str,
    session_id: &str,
    up_to: i64,
) -> io::Result<()> {
    let input = File::open(src)?;
    mkdir_private(dst.parent().unwrap_or(Path::new(".")))?;
    if dst.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", dst.display()),
        ));
    }
    let out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dst)?;
    let rd: Box<dyn Read> = if up_to >= 0 {
        Box::new(input.take(up_to as u64))
    } else {
        Box::new(input)
    };
    let mut r = BufReader::with_capacity(256 << 10, rd);
    let mut w = BufWriter::with_capacity(256 << 10, out);
    let from = format!(r#""sessionId":"{old_id}""#);
    let to = format!(r#""sessionId":"{session_id}""#);
    let finder = memchr::memmem::Finder::new(from.as_bytes());
    let mut line = Vec::new();
    let res: io::Result<()> = (|| {
        loop {
            line.clear();
            if r.read_until(b'\n', &mut line)? == 0 {
                return w.flush();
            }
            if old_id.is_empty() || finder.find(&line).is_none() {
                w.write_all(&line)?;
                continue;
            }
            let mut rest = &line[..];
            while let Some(i) = finder.find(rest) {
                w.write_all(&rest[..i])?;
                w.write_all(to.as_bytes())?;
                rest = &rest[i + from.len()..];
            }
            w.write_all(rest)?;
        }
    })();
    if let Err(e) = res {
        drop(w);
        let _ = fs::remove_file(dst);
        return Err(e);
    }
    Ok(())
}

impl Account {
    /// Where a session's conversation is now. Entering or leaving a
    /// worktree moves it to that folder's project, so when it isn't under
    /// the folder the session started in, it's looked for under every
    /// project; transcript_path when it's nowhere yet. It reads the disk,
    /// so never on the UI.
    pub fn find_transcript(&self, cwd: &str, session_id: &str) -> PathBuf {
        let p = self.transcript_path(cwd, session_id);
        if p.exists() {
            return p;
        }
        self.moved_transcript(session_id).unwrap_or(p)
    }

    /// The first project folder, by name, holding session_id's
    /// transcript; None when none does.
    pub fn moved_transcript(&self, session_id: &str) -> Option<PathBuf> {
        if session_id.is_empty() || session_id.contains(['/', '*', '?', '[', '\\']) {
            return None;
        }
        FIND_INDEX
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .find(&self.projects_dir(), session_id)
    }

    /// Gives conversation to the file checkpoints (the files' old
    /// contents) that conversation from kept, so a copy of it can still
    /// put files back as they were. Claude Code only brings them over
    /// itself when the transcript still names the old session, which a
    /// copy doesn't. The backups never change once written, so they're
    /// linked, not copied.
    pub fn copy_checkpoints(&self, from: &str, to: &str) -> io::Result<()> {
        let src = self.config_dir.join("file-history").join(from);
        let names = match fs::read_dir(&src) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let dst = self.config_dir.join("file-history").join(to);
        mkdir_private(&dst)?;
        for n in names.flatten() {
            if n.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let (s, d) = (n.path(), dst.join(n.file_name()));
            if fs::symlink_metadata(&d).is_ok() || fs::hard_link(&s, &d).is_ok() {
                continue;
            }
            let b = fs::read(&s)?;
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&d)?
                .write_all(&b)?;
        }
        Ok(())
    }
}

/// Which transcripts each project folder holds, listed again only when
/// the folder's time changes (a file added or removed): looking for a
/// session that moved stats each folder rather than listing every one,
/// some thousands of files long.
#[derive(Default)]
struct ProjectIndex {
    roots: HashMap<PathBuf, ProjectsRoot>,
}

struct ProjectsRoot {
    modified: SystemTime,
    /// Its folders, as of modified.
    names: Vec<std::ffi::OsString>,
    dirs: HashMap<std::ffi::OsString, ProjectDir>,
}

struct ProjectDir {
    modified: SystemTime,
    sids: HashSet<String>,
}

static FIND_INDEX: LazyLock<Mutex<ProjectIndex>> =
    LazyLock::new(|| Mutex::new(ProjectIndex::default()));

impl ProjectIndex {
    fn find(&mut self, root: &Path, sid: &str) -> Option<PathBuf> {
        let modified = fs::metadata(root).ok()?.modified().ok()?;
        let fresh = self.roots.get(root).is_some_and(|r| r.modified == modified);
        if !fresh {
            let mut ents: Vec<_> = fs::read_dir(root)
                .ok()?
                .flatten()
                .map(|e| e.file_name())
                .collect();
            ents.sort();
            let mut old = self.roots.remove(root);
            let mut r = ProjectsRoot {
                modified,
                names: Vec::with_capacity(ents.len()),
                dirs: HashMap::new(),
            };
            for n in ents {
                if let Some(d) = old.as_mut().and_then(|o| o.dirs.remove(&n)) {
                    r.dirs.insert(n.clone(), d);
                }
                r.names.push(n);
            }
            self.roots.insert(root.to_path_buf(), r);
        }
        let r = self.roots.get_mut(root)?;
        for n in &r.names {
            let dir = root.join(n);
            let Ok(st) = fs::metadata(&dir) else {
                continue;
            };
            if !st.is_dir() {
                continue;
            }
            let m = st.modified().ok();
            let d = r.dirs.entry(n.clone()).or_insert_with(|| ProjectDir {
                modified: SystemTime::UNIX_EPOCH,
                sids: HashSet::new(),
            });
            if m != Some(d.modified) {
                d.modified = m.unwrap_or(SystemTime::UNIX_EPOCH);
                d.sids = list_sessions(&dir);
            }
            if d.sids.contains(sid) {
                return Some(dir.join(format!("{sid}.jsonl")));
            }
        }
        None
    }
}

/// The session ids whose transcripts are in dir.
fn list_sessions(dir: &Path) -> HashSet<String> {
    let Ok(rd) = fs::read_dir(dir) else {
        return HashSet::new();
    };
    rd.flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".jsonl"))
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // forkcopy_test.go:12 (TestCopyCheckpoints): a rewound copy gets the
    // file checkpoints of the conversation it was cut from, under its own
    // id; the originals stay where they were.
    #[test]
    fn copy_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let a = Account::at("", dir.path());
        let src = a.config_dir.join("file-history").join("old");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("abc@v1"), "hello\n").unwrap();
        a.copy_checkpoints("old", "new").unwrap();
        let got = fs::read_to_string(a.config_dir.join("file-history/new/abc@v1")).unwrap();
        assert_eq!(got, "hello\n");
        assert!(
            src.join("abc@v1").exists(),
            "the original's checkpoint is kept"
        );
        a.copy_checkpoints("old", "new").unwrap();
        a.copy_checkpoints("none", "other").unwrap();
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    // forkcopy_test.go:40 (TestFindTranscriptAfterAMove)
    #[test]
    fn find_transcript_after_a_move() {
        let dir = tempfile::tempdir().unwrap();
        let a = Account::at("", dir.path());
        assert_eq!(
            a.find_transcript("/repo", "sid"),
            a.transcript_path("/repo", "sid")
        );
        let moved = a.transcript_path("/repo/.claude/worktrees/x", "sid");
        touch(&moved);
        assert_eq!(a.find_transcript("/repo", "sid"), moved);
    }

    // forkcopy_test.go:60 (TestFindTranscriptSeesChanges): the search keeps
    // each project's listing until the folder changes.
    #[test]
    fn find_transcript_sees_changes() {
        let dir = tempfile::tempdir().unwrap();
        let a = Account::at("", dir.path());
        touch(&a.transcript_path("/other", "x"));
        let home = a.transcript_path("/repo", "sid");
        assert_eq!(a.find_transcript("/repo", "sid"), home);
        let listed = a.transcript_path("/other", "sid");
        touch(&listed);
        bump(&listed);
        assert_eq!(a.find_transcript("/repo", "sid"), listed);
        fs::remove_file(&listed).unwrap();
        bump(listed.parent().unwrap());
        assert_eq!(a.find_transcript("/repo", "sid"), home);
        let fresh = a.transcript_path("/new", "sid");
        touch(&fresh);
        bump(&a.projects_dir());
        assert_eq!(a.find_transcript("/repo", "sid"), fresh);
    }

    /// Moves a folder's time on a second, as a file added in a later
    /// second would: the index goes by it, and a test doesn't wait.
    fn bump(p: &Path) {
        let dir = if p.is_dir() { p } else { p.parent().unwrap() };
        let m = fs::metadata(dir).unwrap().modified().unwrap();
        File::open(dir)
            .unwrap()
            .set_modified(m + std::time::Duration::from_secs(1))
            .unwrap();
    }

    #[test]
    fn copy_transcript_renames_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jsonl");
        let line1 = "{\"sessionId\":\"old\",\"x\":1}\n";
        fs::write(&src, format!("{line1}{{\"sessionId\":\"old\"}}\n")).unwrap();
        let dst = dir.path().join("p").join("b.jsonl");
        copy_transcript(&src, &dst, "old", "new", line1.len() as i64).unwrap();
        assert_eq!(
            fs::read_to_string(&dst).unwrap(),
            "{\"sessionId\":\"new\",\"x\":1}\n"
        );
        assert!(
            copy_transcript(&src, &dst, "old", "new", -1).is_err(),
            "it never writes over one"
        );
        let all = dir.path().join("c.jsonl");
        copy_transcript(&src, &all, "old", "new", -1).unwrap();
        assert_eq!(
            fs::read_to_string(&all).unwrap().matches("\"new\"").count(),
            2
        );
    }
}
