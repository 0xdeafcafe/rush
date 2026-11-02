//! Writing Claude Code's files safely, and copying what a folder holds of
//! its past sessions. Port of internal/adapters/claude/claude/login.go:
//! 245-265 (writeFileAtomic), merge.go (MergeHistory, copyNew) and
//! carry.go (WriteCarried, uuid).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rush_core::agent::PreviewLine;
use rush_core::time::{format_rfc3339, now};
use serde_json::json;

use crate::account::Account;

/// Makes dir and the folders above it, only you able to read them.
pub(crate) fn mkdir_private(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// A new file in dir named prefix + something unused, as Go's
/// os.CreateTemp: only its owner reads it.
pub(crate) fn create_temp(dir: &Path, prefix: &str) -> io::Result<(PathBuf, File)> {
    let mut n = random_u64();
    loop {
        let p = dir.join(format!("{prefix}{n}"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
        {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n = n.wrapping_add(1),
            Err(e) => return Err(e),
        }
    }
}

/// Writes b to path through a temp file and a rename, so a reader never
/// sees half of it; a file already there keeps its mode.
pub fn write_file_atomic(path: &Path, b: &[u8], mode: u32) -> io::Result<()> {
    let mode = fs::metadata(path).map_or(mode, |m| m.permissions().mode() & 0o777);
    let dir = path.parent().unwrap_or(Path::new("."));
    let base = path.file_name().unwrap_or_default().to_string_lossy();
    let (tmp, mut f) = create_temp(dir, &format!(".{base}-"))?;
    if let Err(e) = f.write_all(b) {
        drop(f);
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    drop(f);
    if let Err(e) = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path)
}

/// What of a config folder is its past sessions: the transcripts, and
/// the file copies /rewind puts back.
const HISTORY: &[&str] = &["projects", "file-history"];

/// Copies from's past sessions into to, so they're found and resumed
/// there. A file to already has is left as it is. from is left whole:
/// nothing is taken away.
pub fn merge_history(from: &Account, to: &Account) -> io::Result<()> {
    let mut first: Option<io::Error> = None;
    for dir in HISTORY {
        let (src, dst) = (from.config_dir.join(dir), to.config_dir.join(dir));
        if let Err(e) = walk_files(&src, &mut |p| {
            if !fs::symlink_metadata(p).is_ok_and(|m| m.is_file()) {
                return Ok(());
            }
            let rel = p.strip_prefix(&src).map_err(io::Error::other)?;
            copy_new(p, &dst.join(rel))
        }) && first.is_none()
        {
            first = Some(e);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Calls each with everything under root that isn't a folder (links not
/// followed), in name order, as Go's filepath.WalkDir; a root that isn't
/// there has none.
pub(crate) fn walk_files(
    root: &Path,
    each: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let ents = match fs::read_dir(root) {
        Ok(r) => r,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut ents: Vec<_> = ents.flatten().collect();
    ents.sort_by_key(|e| e.file_name());
    for e in ents {
        let ft = e.file_type()?;
        if ft.is_dir() {
            walk_files(&e.path(), each)?;
        } else {
            each(&e.path())?;
        }
    }
    Ok(())
}

/// Go's filepath.Clean: the shortest path that names the same file, by
/// its text alone.
pub fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => match out.last() {
                Some(&l) if l != ".." => {
                    out.pop();
                }
                _ if rooted => {}
                _ => out.push(".."),
            },
            _ => out.push(part),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

/// Go's filepath.Join: the parts joined by slashes, cleaned; empty parts
/// left out.
pub fn join(parts: &[&str]) -> String {
    let s: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if s.is_empty() {
        return String::new();
    }
    clean(&s.join("/"))
}

/// Go's filepath.Dir: all but the last element, cleaned.
pub fn dir_of(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => clean(&p[..i + 1]),
        None => ".".into(),
    }
}

/// Go's filepath.Base: the last element; "." for "", "/" for slashes.
pub fn base_of(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { "." } else { "/" };
    }
    match t.rfind('/') {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// The home folder, as Go's os.UserHomeDir: $HOME.
pub fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// Copies src to dst when there's nothing at dst yet, keeping its times:
/// a past session's age is when it last changed.
pub fn copy_new(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::symlink_metadata(dst).is_ok() {
        return Ok(());
    }
    let st = fs::metadata(src)?;
    let dir = dst.parent().unwrap_or(Path::new("."));
    mkdir_private(dir)?;
    let mut input = File::open(src)?;
    let base = dst.file_name().unwrap_or_default().to_string_lossy();
    let (tmp, mut out) = create_temp(dir, &format!(".{base}-"))?;
    if let Err(e) = io::copy(&mut input, &mut out) {
        drop(out);
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    let _ = out.set_permissions(fs::Permissions::from_mode(st.permissions().mode() & 0o777));
    if let Ok(m) = st.modified() {
        let _ = out.set_times(fs::FileTimes::new().set_modified(m).set_accessed(m));
    }
    drop(out);
    fs::rename(&tmp, dst)
}

/// Writes lines, another agent's conversation, as the transcript of
/// session session_id in cwd, for Claude Code to resume as its own: your
/// messages as user lines, its answers as synthetic assistant ones, each
/// after the last.
pub fn write_carried(
    a: &Account,
    cwd: &str,
    session_id: &str,
    lines: &[PreviewLine],
) -> io::Result<()> {
    let p = a.transcript_path(cwd, session_id);
    mkdir_private(p.parent().unwrap_or(Path::new(".")))?;
    // A start that died left one nothing resumed from.
    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&p)?;
    let mut w = io::BufWriter::new(f);
    let mut parent = serde_json::Value::Null;
    let mut res = Ok(());
    for l in lines {
        let at = if l.at == 0 { now() } else { l.at };
        let id = uuid();
        let message = if l.role == "assistant" {
            json!({"id": format!("msg_{id}"), "type": "message", "role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text", "text": l.text}], "stop_reason": "end_turn", "stop_sequence": null,
                "usage": {"input_tokens": 0, "output_tokens": 0}})
        } else {
            json!({"role": "user", "content": l.text})
        };
        let line = json!({"parentUuid": parent, "isSidechain": false, "userType": "external", "cwd": cwd,
            "sessionId": session_id, "type": l.role, "uuid": id, "timestamp": format_rfc3339(at),
            "message": message});
        let mut b = serde_json::to_vec(&line).unwrap_or_default();
        b.push(b'\n');
        if let Err(e) = w.write_all(&b) {
            res = Err(e);
            break;
        }
        parent = serde_json::Value::String(id);
    }
    if let Err(e) = res.and_then(|_| w.flush()) {
        drop(w);
        let _ = fs::remove_file(&p);
        return Err(e);
    }
    Ok(())
}

/// Random bytes, from the system's own source.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    if File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_err()
    {
        // Never on a working system: something unlikely to repeat.
        let t = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        for (i, x) in b.iter_mut().enumerate() {
            *x = (t >> ((i % 16) * 8)) as u8 ^ std::process::id() as u8;
        }
    }
    b
}

fn random_u64() -> u64 {
    u64::from_le_bytes(random_bytes::<8>())
}

/// A random (v4) UUID.
pub fn uuid() -> String {
    let mut b = random_bytes::<16>();
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Hex of b.
pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write(a: &Account, rel: &str, body: &str) {
        let p = a.config_dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    // merge_test.go: an older folder's past sessions are copied into
    // ~/.claude, keeping their age; a transcript ~/.claude has already is
    // left as it is.
    #[test]
    fn merge_history_copies_past_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let from = Account::at("", dir.path().join("old"));
        let to = Account::at("", dir.path().join("home"));
        write(&from, "projects/-work/s1.jsonl", "old s1");
        write(&from, "projects/-work/s2.jsonl", "old s2");
        write(&from, "file-history/s1/abc@v1", "v1");
        write(&from, "settings.json", "{}");
        write(&to, "projects/-work/s2.jsonl", "home s2");
        let then = SystemTime::now() - Duration::from_secs(72 * 3600);
        let then = SystemTime::UNIX_EPOCH
            + Duration::from_secs(
                then.duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
        File::options()
            .write(true)
            .open(from.config_dir.join("projects/-work/s1.jsonl"))
            .unwrap()
            .set_modified(then)
            .unwrap();

        merge_history(&from, &to).unwrap();
        let read = |rel: &str| fs::read_to_string(to.config_dir.join(rel)).unwrap_or_default();
        assert_eq!(read("projects/-work/s1.jsonl"), "old s1");
        assert_eq!(read("file-history/s1/abc@v1"), "v1");
        assert_eq!(
            read("projects/-work/s2.jsonl"),
            "home s2",
            "a transcript ~/.claude had was replaced"
        );
        assert_eq!(
            read("settings.json"),
            "",
            "more than past sessions was copied"
        );
        let st = fs::metadata(to.config_dir.join("projects/-work/s1.jsonl")).unwrap();
        assert_eq!(st.modified().unwrap(), then);
        merge_history(&Account::at("", dir.path().join("missing")), &to).unwrap();
    }

    #[test]
    fn carried_lines_resume_as_a_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let a = Account::at("", dir.path());
        let lines = [
            PreviewLine {
                role: "user".into(),
                text: "hi".into(),
                at: 1_000,
                ..PreviewLine::default()
            },
            PreviewLine {
                role: "assistant".into(),
                text: "hello".into(),
                ..PreviewLine::default()
            },
        ];
        write_carried(&a, "/work", "s1", &lines).unwrap();
        let b = fs::read_to_string(a.transcript_path("/work", "s1")).unwrap();
        let got: Vec<serde_json::Value> = b
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["parentUuid"], serde_json::Value::Null);
        assert_eq!(got[1]["parentUuid"], got[0]["uuid"]);
        assert_eq!(got[0]["timestamp"], "1970-01-01T00:00:01Z");
        assert_eq!(got[0]["message"]["content"], "hi");
        assert_eq!(got[1]["message"]["model"], "<synthetic>");
        assert_eq!(got[1]["message"]["content"][0]["text"], "hello");
        let id = got[0]["uuid"].as_str().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
    }
}
