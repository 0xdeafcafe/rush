//! Writing, tidying and counting the use of auto memory notes, the way
//! Claude Code writes them itself: one fact a file, with name, description
//! and type in its frontmatter, and a line in MEMORY.md pointing at it.
//! Port of internal/adapters/claude/claude/notes.go:1-563.
//!
//! MEMORY.md and CLAUDE.md change whole (a new file renamed over the old)
//! under a lock file, so two rushes, or rush and an agent's rush memory,
//! don't write over each other's changes; and a new note never writes over
//! one.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use rush_core::adapter::front_matter as front_matter_of;
use rush_core::time::{Ms, now, parse_rfc3339};
use serde::Deserialize;

use crate::files::{base_of, clean, dir_of, join};
use crate::jobs::ms_of;
use crate::memory::{INDEX_LINE_LEN, front_matter, plural, read_mem_index};

/// The kinds of memory note Claude Code writes.
pub const NOTE_TYPES: &[&str] = &["user", "feedback", "project", "reference"];

/// A memory note to write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    /// short-kebab-case, its file's name too
    pub name: String,
    /// Its line in MEMORY.md.
    pub title: String,
    pub description: String,
    pub kind: String,
    pub body: String,
}

fn err(s: &str) -> io::Error {
    io::Error::other(s.to_string())
}

static NOT_SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new("[^a-z0-9]+").unwrap());

/// s as a note's file name: lower case, words joined by dashes.
pub fn slug(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut s = NOT_SLUG
        .replace_all(&lower, "-")
        .trim_matches('-')
        .to_string();
    if s.len() > 60 {
        s.truncate(60); // all ASCII by now
        s = s.trim_end_matches('-').to_string();
    }
    s
}

/// Writes n into the memory folder dir as a new note, and its line into
/// MEMORY.md, and says where. A name already taken gets a number
/// (testing-2), so no note is ever written over.
pub fn write_note(dir: &str, mut n: Note) -> io::Result<String> {
    n.name = slug(&n.name);
    if n.name.is_empty() {
        n.name = slug(&n.title);
    }
    if n.name.is_empty() || n.body.trim().is_empty() {
        return Err(err("a memory note needs a name and some text"));
    }
    if n.name == "memory" {
        n.name = "memory-note".into(); // MEMORY.md, on a disk that ignores case
    }
    if !NOTE_TYPES.contains(&n.kind.as_str()) {
        n.kind = "project".into();
    }
    n.title = one_line(&n.title);
    if n.title.is_empty() {
        n.title = n.name.clone();
    }
    fs::create_dir_all(dir)?;
    locked(&join(&[dir, ".rush.lock"]), || {
        let stem = n.name.clone();
        for i in 1.. {
            let name = if i > 1 {
                format!("{stem}-{i}")
            } else {
                stem.clone()
            };
            let path = join(&[dir, &format!("{name}.md")]);
            let mut f = match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                r => r?,
            };
            n.name = name;
            if let Err(e) = f.write_all(note_text(&n).as_bytes()) {
                drop(f);
                let _ = fs::remove_file(&path);
                return Err(e);
            }
            drop(f);
            index_line(dir, &path, &n.title, &n.description)?;
            return Ok(path);
        }
        unreachable!()
    })
}

/// Runs f holding the lock file at lock, so two rushes, or rush and an
/// agent's rush memory, don't write over each other's changes.
fn locked<T>(lock: &str, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let lf = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock)
        .map_err(|e| crate::screen::path_error("open", lock.as_ref(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(lock, fs::Permissions::from_mode(0o600));
    }
    // SAFETY: flock on a descriptor lf owns; let go when it's closed.
    if unsafe { libc::flock(lf.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let out = f();
    drop(lf);
    out
}

/// The file at path, "" when there's none; any other error is one, so a
/// file that can't be read is never written over as if empty.
fn read_text(path: &str) -> io::Result<String> {
    match fs::read(path) {
        Ok(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Replaces the file at path whole, by a rename, through a link to where
/// it points.
fn write_text(path: &str, text: &str) -> io::Result<()> {
    let path = fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    let dir = dir_of(&path);
    let base = base_of(&path).to_string();
    let mut tmp = String::new();
    let mut f = None;
    for i in 0..100u32 {
        let t = join(&[
            &dir,
            &format!(".{base}.{}{}{i}", std::process::id(), now() % 1_000_000),
        ]);
        match fs::OpenOptions::new().write(true).create_new(true).open(&t) {
            Ok(h) => {
                (tmp, f) = (t, Some(h));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let Some(mut f) = f else {
        return Err(err("no temporary file to write"));
    };
    if let Err(e) = f.write_all(text.as_bytes()) {
        drop(f);
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).map_or(0o644, |m| m.permissions().mode() & 0o777);
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode));
    }
    fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn note_text(n: &Note) -> String {
    format!(
        "---\nname: {}\ndescription: {}\nmetadata:\n  type: {}\n---\n\n{}\n",
        n.name,
        yaml_line(&n.description),
        n.kind,
        n.body.trim()
    )
}

/// s on one line, quoted, so YAML reads it as the text it is whatever it
/// starts with (? - yes 10 ...): as Go's json.Marshal writes a string,
/// HTML characters escaped too.
fn yaml_line(s: &str) -> String {
    let q = serde_json::to_string(&one_line(s)).unwrap_or_default();
    let mut out = String::with_capacity(q.len());
    for c in q.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Gives the note at path a line in its MEMORY.md, unless one points at it
/// already. The line keeps its link whole: the hook is what's cut to fit.
/// The caller holds the folder's lock.
fn index_line(dir: &str, path: &str, title: &str, hook: &str) -> io::Result<()> {
    let index = join(&[dir, "MEMORY.md"]);
    let mut text = read_text(&index)?;
    let base = base_of(path);
    if text.split('\n').any(|l| target(l) == base) {
        return Ok(());
    }
    let mut title = one_line(title).replace('[', "(").replace(']', ")");
    if title.chars().count() > 60 {
        title = title.chars().take(59).collect::<String>() + "…";
    }
    let mut line = format!("- [{title}]({base})");
    let hook = one_line(hook);
    if !hook.is_empty() {
        line += &format!(" — {hook}");
    }
    if line.chars().count() > INDEX_LINE_LEN {
        line = line.chars().take(INDEX_LINE_LEN - 1).collect::<String>() + "…";
    }
    text.truncate(text.trim_end_matches('\n').len());
    if !text.is_empty() {
        text.push('\n');
    }
    write_text(&index, &format!("{text}{line}\n"))
}

static INDEX_ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[-*]\s*\[[^\]]*\]\(([^)\s]+)\)").unwrap());

/// The note an index line is for: the file its first link, at its start,
/// points at, "" for any other line.
fn target(line: &str) -> &str {
    let Some(m) = INDEX_ENTRY.captures(line) else {
        return "";
    };
    let t = m.get(1).map_or("", |m| m.as_str());
    let rel = t.strip_prefix("./").unwrap_or(t);
    if t.contains("://") || rel.contains('/') {
        return "";
    }
    rel
}

/// Rewrites MEMORY.md in dir without the index lines for which drop says
/// so, and says how many went. Other lines, and links further along a
/// line, are left as they are.
fn drop_lines(dir: &str, drop: impl Fn(&str) -> bool) -> io::Result<usize> {
    let index = join(&[dir, "MEMORY.md"]);
    let text = read_text(&index)?;
    if text.is_empty() {
        return Ok(0);
    }
    let mut keep = Vec::new();
    let mut n = 0;
    for l in text.split('\n') {
        let t = target(l);
        if !t.is_empty() && drop(t) {
            n += 1;
            continue;
        }
        keep.push(l);
    }
    if n == 0 {
        return Ok(0);
    }
    write_text(&index, &keep.join("\n"))?;
    Ok(n)
}

/// Reads the note at path: its frontmatter, and its text below.
pub fn read_note(path: &str) -> io::Result<Note> {
    let b = fs::read(path)?;
    let fm = front_matter_of(&b[..]);
    let get = |k: &str| fm.get(k).cloned().unwrap_or_default();
    let text = String::from_utf8_lossy(&b);
    let mut n = Note {
        name: get("name"),
        description: get("description"),
        kind: get("type"),
        body: note_body(&text),
        ..Note::default()
    };
    if n.name.is_empty() {
        n.name = base_of(path).trim_end_matches(".md").to_string();
    }
    n.title = n.name.clone();
    Ok(n)
}

/// A note's text without its frontmatter.
fn note_body(s: &str) -> String {
    let Some(rest) = s.strip_prefix("---") else {
        return s.trim().to_string();
    };
    match rest.find("\n---") {
        Some(i) => rest[i + 4..].trim().to_string(),
        None => s.trim().to_string(),
    }
}

/// Tidies the memory folder dir the ways that need no judgement: a line in
/// MEMORY.md for each note without one, and none for notes that don't
/// exist. It says what it did.
pub fn fix_memory(dir: &str) -> io::Result<Vec<String>> {
    locked(&join(&[dir, ".rush.lock"]), || {
        let mut did = Vec::new();
        let gone = drop_lines(
            dir,
            |note| matches!(fs::metadata(join(&[dir, note])), Err(e) if e.kind() == io::ErrorKind::NotFound),
        )?;
        if gone > 0 {
            did.push(plural(
                gone,
                "line for a note that doesn't exist taken out",
                "lines for notes that don't exist taken out",
            ));
        }
        let linked: HashSet<String> = read_mem_index(&join(&[dir, "MEMORY.md"]))
            .links
            .into_iter()
            .map(|l| l.file)
            .collect();
        let mut names: Vec<String> = fs::read_dir(dir)?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md"))
            .collect();
        names.sort();
        let mut added = 0;
        for name in names {
            let p = clean(&join(&[dir, &name]));
            if name == "MEMORY.md" || linked.contains(&p) {
                continue;
            }
            let fm = front_matter(&p);
            let mut title = fm.get("name").cloned().unwrap_or_default();
            if title.is_empty() {
                title = name.trim_end_matches(".md").to_string();
            }
            let hook = fm.get("description").cloned().unwrap_or_default();
            index_line(dir, &p, &title, &hook)?;
            added += 1;
        }
        if added > 0 {
            did.push(plural(
                added,
                "note added to MEMORY.md",
                "notes added to MEMORY.md",
            ));
        }
        Ok(did)
    })
}

/// Puts the note at drop into the one at keep, below its text, and deletes
/// drop and its line in MEMORY.md.
pub fn merge_notes(keep: &str, drop: &str) -> io::Result<()> {
    if keep == drop {
        return Err(err("a note can't be merged into itself"));
    }
    let dir = dir_of(drop);
    locked(&join(&[&dir, ".rush.lock"]), || {
        let k = fs::read(keep)?;
        let d = read_note(drop)?;
        let k = String::from_utf8_lossy(&k);
        write_text(
            keep,
            &format!("{}\n\n{}\n", k.trim_end_matches('\n'), d.body),
        )?;
        forget(drop)
    })
}

/// Deletes a file, and when it's a memory note, its line in MEMORY.md.
pub fn forget_note(path: &str) -> io::Result<()> {
    let dir = dir_of(path);
    if base_of(&dir) != "memory" || base_of(path) == "MEMORY.md" {
        return fs::remove_file(path);
    }
    locked(&join(&[&dir, ".rush.lock"]), || forget(path))
}

/// forget_note, its folder's lock held.
fn forget(path: &str) -> io::Result<()> {
    fs::remove_file(path)?;
    let base = base_of(path);
    drop_lines(&dir_of(path), |note| note == base).map(|_| ())
}

/// Moves the note at path into the user's CLAUDE.md in cfg, which every
/// session in every project loads, under a heading of its own.
pub fn make_global(cfg: &str, path: &str) -> io::Result<()> {
    let n = read_note(path)?;
    let md = join(&[cfg, "CLAUDE.md"]);
    // One lock for every account that links to it.
    let lock = match fs::canonicalize(&md) {
        Ok(real) => format!("{}.rush.lock", real.to_string_lossy()),
        Err(_) => format!("{md}.rush.lock"),
    };
    locked(&lock, || {
        let mut text = read_text(&md)?;
        text.truncate(text.trim_end_matches('\n').len());
        if !text.is_empty() {
            text += "\n\n";
        }
        let head = if n.description.is_empty() {
            &n.name
        } else {
            &n.description
        };
        text += &format!("## {}\n\n{}\n", one_line(head), n.body);
        write_text(&md, &text)?;
        forget_note(path)
    })
}

/// How often sessions read a memory note, and when last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Use {
    pub reads: i64,
    pub last: Ms,
}

/// One transcript's reads of notes: each note's file name → when it was
/// read. Its own lock is held while it's read, not the cache's.
#[derive(Default)]
struct RecallScan {
    off: u64,
    reads: HashMap<String, Vec<Ms>>,
}

/// What each transcript was read for, by path and key, so a transcript is
/// read only past where it was read last.
static RECALL_CACHE: LazyLock<Mutex<HashMap<String, Arc<Mutex<RecallScan>>>>> =
    LazyLock::new(Default::default);

/// Counts the reads of the notes in the memory folder dir in the last
/// since ms, across the transcripts in projects (Claude Code's projects
/// folder) of the project whose folder is slug and its worktrees, their
/// subagents' too. A note is opened by reading it, or by a shell command
/// naming it, through any account's folder (accounts share projects by a
/// link), so those are what's counted.
pub fn note_uses(projects: &str, slug: &str, dir: &str, since: Ms) -> HashMap<String, Use> {
    let mut out: HashMap<String, Use> = HashMap::new();
    let cutoff = now() - since;
    let key = format!("/projects/{slug}/memory/");
    let mut live = HashSet::new();
    let mut dirs: Vec<String> = fs::read_dir(projects)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        // Not another project whose name starts the same.
        .filter(|b| b == slug || b.starts_with(&format!("{slug}--")))
        .map(|b| join(&[projects, &b]))
        .collect();
    dirs.sort();
    for d in dirs {
        let mut files = jsonl_in(&d);
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            files.extend(jsonl_in(&join(&[&e.path().to_string_lossy(), "subagents"])));
        }
        for f in files {
            let Ok(st) = fs::metadata(&f) else { continue };
            if st.modified().map_or(true, |m| ms_of(m) < cutoff) {
                continue;
            }
            let ck = format!("{f}\x00{key}");
            live.insert(ck.clone());
            for (name, at) in scan_reads(&f, &ck, st.len(), &key) {
                let p = join(&[dir, &name]);
                let mut o = out.get(&p).copied().unwrap_or_default();
                for t in at {
                    if t < cutoff {
                        continue;
                    }
                    o.reads += 1;
                    o.last = o.last.max(t);
                }
                if o.reads > 0 {
                    out.insert(p, o);
                }
            }
        }
    }
    // Transcripts gone or out of the window are let go.
    let tail = format!("\x00{key}");
    RECALL_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|k, _| !k.ends_with(&tail) || live.contains(k));
    out
}

/// The .jsonl files in dir, sorted.
fn jsonl_in(dir: &str) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".jsonl"))
        .map(|n| join(&[dir, &n]))
        .collect();
    v.sort();
    v
}

static NOTE_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+\.md").unwrap());
/// A shell command that writes or deletes rather than reads.
static WRITES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[\s;&|])(rm|mv|cp|tee|sed -i)\s|>\s*\S*/memory/").unwrap());

/// When each note whose path has key was read in the transcript f, by
/// file name, read from where it was read to last.
fn scan_reads(f: &str, ck: &str, size: u64, key: &str) -> HashMap<String, Vec<Ms>> {
    let s = RECALL_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(ck.to_string())
        .or_default()
        .clone();
    let mut s = s.lock().unwrap_or_else(|e| e.into_inner());
    if size < s.off {
        // Written over: from the start.
        *s = RecallScan::default();
    }
    if size > s.off {
        s.read(f, key);
    }
    s.reads.clone()
}

impl RecallScan {
    /// Reads f from off on, its lock held.
    fn read(&mut self, f: &str, key: &str) {
        let Ok(mut fh) = fs::File::open(f) else {
            return;
        };
        if fh.seek(SeekFrom::Start(self.off)).is_err() {
            return;
        }
        let mut r = BufReader::with_capacity(256 << 10, fh);
        let mut off = self.off;
        let mut line = Vec::new();
        let kb = key.as_bytes();
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.last() != Some(&b'\n') {
                break; // a line still being written is read next time
            }
            off += line.len() as u64;
            let has = |n: &[u8]| memchr::memmem::find(&line, n).is_some();
            if !has(kb) || !has(br#""Read""#) && !has(br#""Bash""#) {
                continue;
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Input {
                file_path: Option<String>,
                command: Option<String>,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct C {
                #[serde(rename = "type")]
                kind: Option<String>,
                name: Option<String>,
                input: Option<Input>,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Msg {
                content: Option<Vec<C>>,
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Rec {
                timestamp: Option<String>,
                message: Option<Msg>,
            }
            let Ok(rec) = serde_json::from_slice::<Rec>(&line) else {
                continue;
            };
            let at = rec.timestamp.as_deref().map_or(0, parse_rfc3339);
            for c in rec.message.and_then(|m| m.content).unwrap_or_default() {
                if c.kind.as_deref() != Some("tool_use") {
                    continue;
                }
                let input = c.input.unwrap_or_default();
                let mut names = Vec::new();
                match c.name.as_deref() {
                    Some("Read") => {
                        let fp = input.file_path.unwrap_or_default();
                        if let Some((_, name)) = fp.split_once(key)
                            && !name.contains('/')
                        {
                            names.push(name.to_string());
                        }
                    }
                    Some("Bash") => {
                        let cmd = input.command.unwrap_or_default();
                        if WRITES.is_match(&cmd) {
                            continue;
                        }
                        let mut rest = cmd.as_str();
                        while let Some((_, after)) = rest.split_once(key) {
                            if let Some(m) = NOTE_FILE.find(after)
                                && m.as_str() != "MEMORY.md"
                            {
                                names.push(m.as_str().to_string());
                            }
                            rest = after;
                        }
                    }
                    _ => {}
                }
                for name in names {
                    self.reads.entry(name).or_default().push(at);
                }
            }
        }
        self.off = off;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tmp() -> (tempfile::TempDir, String) {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().to_string_lossy().into_owned();
        (t, p)
    }

    fn read(p: &str) -> String {
        fs::read_to_string(p).unwrap_or_default()
    }

    // Go says which file: "open …/memory/.rush.lock: no such file or
    // directory" when there's no memory folder to tidy.
    #[test]
    fn no_memory_folder_names_its_lock() {
        let (_t, p) = tmp();
        let dir = format!("{p}/memory");
        let e = fix_memory(&dir).unwrap_err();
        assert_eq!(
            e.to_string(),
            format!("open {dir}/.rush.lock: no such file or directory")
        );
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }

    fn note(name: &str, title: &str, desc: &str, kind: &str, body: &str) -> Note {
        Note {
            name: name.into(),
            title: title.into(),
            description: desc.into(),
            kind: kind.into(),
            body: body.into(),
        }
    }

    // notes_test.go:11 (TestWriteNoteAndIndex)
    #[test]
    fn write_note_and_index() {
        let (_t, root) = tmp();
        let dir = join(&[&root, "memory"]);
        let p = write_note(
            &dir,
            note(
                "Targeted Tests",
                "Targeted tests",
                "run: only what changed",
                "feedback",
                "Run only the tests you touched.",
            ),
        )
        .unwrap();
        assert_eq!(base_of(&p), "targeted-tests.md");
        let fm = front_matter(&p);
        assert_eq!(
            (
                fm["name"].as_str(),
                fm["type"].as_str(),
                fm["description"].as_str()
            ),
            ("targeted-tests", "feedback", "run: only what changed")
        );
        // The same name again is a new note beside it, never written over.
        let p2 = write_note(&dir, note("targeted-tests", "", "x", "bogus", "again")).unwrap();
        assert_eq!(base_of(&p2), "targeted-tests-2.md");
        assert!(
            read(&p).contains("Run only the tests you touched."),
            "the first note was written over"
        );
        let ix = read(&join(&[&dir, "MEMORY.md"]));
        assert!(
            ix.contains("- [Targeted tests](targeted-tests.md) — run: only what changed")
                && ix.contains("(targeted-tests-2.md) — x"),
            "index {ix:?}"
        );
        let fm = front_matter(&p2);
        assert_eq!(
            (fm["type"].as_str(), fm["name"].as_str()),
            ("project", "targeted-tests-2"),
            "an unknown type should fall back to project"
        );
        // A long title keeps its link whole; YAML-looking text stays text.
        let p3 = write_note(
            &dir,
            note("long", &"word ".repeat(60), "? yes: 10", "user", "b"),
        )
        .unwrap();
        let ix = read(&join(&[&dir, "MEMORY.md"]));
        assert!(ix.contains("…](long.md)"), "index {ix:?}");
        assert_eq!(front_matter(&p3)["description"], "? yes: 10");
    }

    // notes_test.go:51 (TestFixMemoryKeepsOtherText)
    #[test]
    fn fix_memory_keeps_other_text() {
        let (_t, root) = tmp();
        let dir = join(&[&root, "memory"]);
        fs::create_dir_all(&dir).unwrap();
        let index = "# Notes\nSee [the guide](../../docs/guide.md) first.\n- [A](a.md) — see [guide](../../docs/guide.md) and [gone](gone.md)\n- [Gone](gone.md) — x\n";
        let ixp = join(&[&dir, "MEMORY.md"]);
        fs::write(&ixp, index).unwrap();
        fs::write(
            join(&[&dir, "a.md"]),
            "---\nname: a\ndescription: a\n---\nhi\n",
        )
        .unwrap();
        fix_memory(&dir).unwrap();
        let want = "# Notes\nSee [the guide](../../docs/guide.md) first.\n- [A](a.md) — see [guide](../../docs/guide.md) and [gone](gone.md)\n";
        assert_eq!(read(&ixp), want);
        // Forgetting a note takes its own line, not one that mentions it.
        fs::write(join(&[&dir, "gone.md"]), "x").unwrap();
        fs::write(&ixp, format!("{want}- [Gone](./gone.md) — x\n")).unwrap();
        forget_note(&join(&[&dir, "gone.md"])).unwrap();
        assert_eq!(read(&ixp), want, "after forget");
    }

    // notes_test.go:76 (TestMakeGlobalKeepsUnreadable)
    #[test]
    fn make_global_keeps_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, cfg) = tmp();
        let dir = join(&[&cfg, "projects", "x", "memory"]);
        let a = write_note(&dir, note("a", "", "first", "feedback", "A")).unwrap();
        let md = join(&[&cfg, "CLAUDE.md"]);
        fs::write(&md, "precious").unwrap();
        fs::set_permissions(&md, fs::Permissions::from_mode(0o200)).unwrap(); // writable, not readable
        assert!(
            make_global(&cfg, &a).is_err(),
            "a CLAUDE.md that can't be read must not be written over"
        );
        fs::set_permissions(&md, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read(&md), "precious");
        assert!(Path::new(&a).exists(), "the note should stay");
        // Through a link, the file it points at is what changes.
        let (_t2, other) = tmp();
        let real = join(&[&other, "CLAUDE.md"]);
        fs::write(&real, "mine").unwrap();
        fs::remove_file(&md).unwrap();
        std::os::unix::fs::symlink(&real, &md).unwrap();
        make_global(&cfg, &a).unwrap();
        assert!(
            fs::symlink_metadata(&md).unwrap().file_type().is_symlink(),
            "the link was replaced by a file"
        );
        assert!(
            read(&real).contains("mine\n\n## first\n\nA"),
            "real {:?}",
            read(&real)
        );
    }

    // notes_test.go:109 (TestFixMemory)
    #[test]
    fn fix_memory_lines() {
        let (_t, dir) = tmp();
        let ixp = join(&[&dir, "MEMORY.md"]);
        fs::write(&ixp, "- [Gone](gone.md) — x\n- [Here](here.md) — y\n").unwrap();
        fs::write(
            join(&[&dir, "here.md"]),
            "---\nname: here\ndescription: y\n---\nhi\n",
        )
        .unwrap();
        fs::write(
            join(&[&dir, "orphan.md"]),
            "---\nname: orphan\ndescription: lost\n---\nhi\n",
        )
        .unwrap();
        let did = fix_memory(&dir).unwrap();
        assert_eq!(did.len(), 2, "{did:?}");
        let ix = read(&ixp);
        assert!(
            !ix.contains("gone.md") && ix.contains("(orphan.md) — lost") && ix.contains("here.md"),
            "index {ix:?}"
        );
        assert!(
            fix_memory(&dir).unwrap().is_empty(),
            "second fix did something"
        );
    }

    // notes_test.go:127 (TestMergeAndGlobal)
    #[test]
    fn merge_and_global() {
        let (_t, cfg) = tmp();
        let dir = join(&[&cfg, "projects", "x", "memory"]);
        let a = write_note(&dir, note("a", "", "first", "feedback", "A text")).unwrap();
        let b = write_note(&dir, note("b", "", "second", "feedback", "B text")).unwrap();
        merge_notes(&a, &b).unwrap();
        assert!(
            read(&a).contains("A text\n\nB text"),
            "merged {:?}",
            read(&a)
        );
        assert!(!Path::new(&b).exists(), "b should be gone");
        make_global(&cfg, &a).unwrap();
        assert!(read(&join(&[&cfg, "CLAUDE.md"])).contains("## first\n\nA text"));
        assert_eq!(read(&join(&[&dir, "MEMORY.md"])).trim(), "", "index left");
    }

    // notes_test.go:155 (TestNoteUses)
    #[test]
    fn note_uses_counts_reads() {
        let (_t, projects) = tmp();
        let slug = "-repo";
        let dir = join(&[&projects, slug, "memory"]);
        let read_line = |p: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","message":{{"content":[{{"type":"tool_use","name":"Read","input":{{"file_path":"{p}"}}}}]}}}}"#
            ) + "\n"
        };
        let wt = join(&[&projects, &format!("{slug}--claude-worktrees-1")]);
        let other = join(&[&projects, &format!("{slug}-other")]);
        for d in [&wt, &other, &dir] {
            fs::create_dir_all(d).unwrap();
        }
        // Read through another account's folder, from a worktree's session.
        let a = read_line("/acct2/projects/-repo/memory/a.md");
        fs::write(join(&[&wt, "s.jsonl"]), format!("{a}{a}")).unwrap();
        fs::write(
            join(&[&other, "s.jsonl"]),
            read_line("/x/projects/-repo/memory/a.md"),
        )
        .unwrap();
        let f = join(&[&projects, slug, "s.jsonl"]);
        let bash = |cmd: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-10-02T10:00:00Z","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"{cmd}"}}}}]}}}}"#
            ) + "\n"
        };
        fs::write(
            &f,
            read_line("/x/projects/-repo/memory/b.md")
                + &bash("cat ~/x/projects/-repo/memory/c.md; ls")
                + &bash("rm /x/projects/-repo/memory/c.md"),
        )
        .unwrap();
        const DAY: Ms = 24 * 3_600_000;
        let u = note_uses(&projects, slug, &dir, 365 * DAY);
        let reads = |u: &HashMap<String, Use>, n: &str| {
            u.get(&join(&[&dir, n])).copied().unwrap_or_default()
        };
        assert_eq!(
            (reads(&u, "a.md").reads, reads(&u, "b.md").reads),
            (2, 1),
            "uses"
        );
        let c = reads(&u, "c.md");
        assert_eq!(
            (c.reads, c.last),
            (1, parse_rfc3339("2026-10-02T10:00:00Z")),
            "a cat reads it, an rm doesn't"
        );
        // Reads before the window aren't counted, whatever the file's age.
        assert_eq!(
            reads(&note_uses(&projects, slug, &dir, DAY), "a.md").reads,
            0,
            "old reads counted"
        );
        // More of the transcript is read from where it stopped.
        let mut fh = fs::OpenOptions::new().append(true).open(&f).unwrap();
        fh.write_all(read_line("/x/projects/-repo/memory/b.md").as_bytes())
            .unwrap();
        drop(fh);
        assert_eq!(
            reads(&note_uses(&projects, slug, &dir, 365 * DAY), "b.md").reads,
            2
        );
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Targeted Tests!"), "targeted-tests");
        assert_eq!(slug(&"ab ".repeat(40)).len(), 59);
        // As Go's json.Marshal writes it, and read back the same.
        assert_eq!(yaml_line("a  <b> & c"), r#""a \u003cb\u003e \u0026 c""#);
        let fm =
            front_matter_of(format!("---\ndescription: {}\n---\n", yaml_line("<b>")).as_bytes());
        assert_eq!(fm["description"], "<b>");
    }
}
