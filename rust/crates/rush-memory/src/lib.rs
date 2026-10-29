//! rush's side of an agent's memory notes: writing one from a few words,
//! and which notes are worth a look. Go: internal/memory/memory.go
//! (draftSystem :21, Asker :31, Draft :38, parseDraft :63, Types :83,
//! ValidType :86, clip :88, Settled :97, Why :103, keptPath :118, keptMu
//! :122, readKept :124, IsKept :134, Keep :142); internal/agent/memory.go
//! (MemoryNote :73, MemoryKeeper :82); cmd/rush/memory.go (memoryUsage
//! :17, addFlags :140, readsText :189).
//!
//! The model is asked through whatever the caller hands [`draft`]: the
//! UI's cheap asker, or a test's. [`Keeper`] is the agent's side (Claude
//! Code's notes.go), which no adapter fills in yet.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rush_core::adapter::{Memory, MemoryFile, Profile};
use rush_core::time::Ms;
use serde::Deserialize;

/// What the calls here fail with, in words.
pub type Result<T> = std::result::Result<T, String>;

/// A memory note to write (agent.MemoryNote).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MemoryNote {
    /// short-kebab-case, its file's name too.
    pub name: String,
    /// Its line in the index.
    pub title: String,
    /// What recall picks it by.
    pub description: String,
    /// user, feedback, project or reference.
    #[serde(rename = "type")]
    pub kind: String,
    pub body: String,
}

/// How often sessions opened a note lately, and when last; counted,
/// whether they were counted at all (agent.MemoryFile's Reads, LastRead
/// and Counted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Opens {
    pub reads: i32,
    pub last_read: Ms,
    pub counted: bool,
}

/// An agent whose memory notes rush can write and tidy
/// (agent.MemoryKeeper).
pub trait Keeper: Send + Sync {
    /// Writes a note into the memory of p's sessions in cwd, and says
    /// where.
    fn remember(&self, p: &Profile, cwd: &str, n: &MemoryNote) -> Result<String>;
    /// Fixes what needs no judgement in the memory folder dir, and says
    /// what it did.
    fn tidy_memory(&self, dir: &str) -> Result<Vec<String>>;
    /// Puts the note at drop into keep, and deletes drop.
    fn merge_notes(&self, keep: &str, drop: &str) -> Result<()>;
    /// Moves a project's note into what every project of p's reads.
    fn make_global(&self, p: &Profile, path: &str) -> Result<()>;
    /// Reads a note: its name, description, type and text.
    fn read_note(&self, path: &str) -> Result<MemoryNote>;
    /// How often sessions opened each of mem's notes lately, by path: it
    /// reads transcripts, so it's asked for only where shown.
    fn count_opens(&self, p: &Profile, mem: &Memory) -> HashMap<String, Opens>;
}

const DRAFT_SYSTEM: &str = r#"You turn what a user wants an AI coding agent to remember into one memory note.
Answer with JSON only, no fence: {"name","title","description","type","body"}.
- name: short-kebab-case, 2-5 words.
- title: a few words, for the index.
- description: one line under 120 characters saying what the note is about, so it can be picked when relevant.
- type: "user" (who the user is, their preferences), "feedback" (how the agent should work: a correction or an approach to keep), "project" (ongoing work, goals or constraints of this project), or "reference" (where to find something outside the code).
- body: the fact in plain words. For feedback and project, follow it with a "**Why:**" line and a "**How to apply:**" line when the text gives a reason; never invent one.
Keep the user's own words and terms; don't add anything they didn't say. Convert relative dates to absolute ones using today's date."#;

/// Turns what to remember, and what was said around it if anything, into
/// a note, with the cheap model ask (system, prompt) reaches. It waits on
/// the model: never on the UI thread.
pub fn draft(
    ask: impl FnOnce(&str, &str) -> Result<String>,
    what: &str,
    around: &str,
) -> Result<MemoryNote> {
    let what = what.trim();
    if what.is_empty() && around.trim().is_empty() {
        return Err("nothing to remember".into());
    }
    let (y, m, d) = local_date(rush_core::time::now());
    let mut prompt = format!("Today is {y:04}-{m:02}-{d:02}.\n\n");
    if !around.is_empty() {
        prompt += &format!(
            "The conversation it comes from:\n<conversation>\n{}\n</conversation>\n\n",
            clip(around, 12_000)
        );
    }
    if !what.is_empty() {
        prompt += &format!("What to remember:\n{what}");
    } else {
        prompt +=
            "Remember the one thing from that conversation most worth knowing in future sessions.";
    }
    let out = ask(DRAFT_SYSTEM, &prompt)?;
    parse_draft(&out).map_err(|e| format!("the model's note didn't read: {e}"))
}

/// The first JSON object in out, whatever the model said around it.
fn parse_draft(out: &str) -> Result<MemoryNote> {
    let Some(i) = out.find('{') else {
        return Err("no JSON in it".into());
    };
    let mut n = serde_json::Deserializer::from_str(&out[i..])
        .into_iter::<MemoryNote>()
        .next()
        .ok_or("no JSON in it")?
        .map_err(|e| e.to_string())?;
    if !valid_type(&n.kind) {
        n.kind = "project".into();
    }
    if n.body.trim().is_empty() || format!("{}{}", n.name, n.title).trim().is_empty() {
        return Err("no name or text".into());
    }
    Ok(n)
}

/// The kinds of memory note.
pub const TYPES: [&str; 4] = ["user", "feedback", "project", "reference"];

/// Whether t is one of TYPES.
pub fn valid_type(t: &str) -> bool {
    TYPES.contains(&t)
}

/// s's last n bytes, an ellipsis first, when it's longer.
fn clip(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut at = s.len() - n;
    while !s.is_char_boundary(at) {
        at += 1;
    }
    format!("…{}", &s[at..])
}

/// How long a project or reference note goes unchanged before it's worth
/// asking whether it's still true.
pub const SETTLED: Ms = 60 * 24 * 3_600_000;

/// Why a memory note is worth a look, "" when it isn't: what's untidy
/// about it, or that it's about work or a place that may have moved on
/// and hasn't changed in a while. A note kept as it is isn't, until it
/// changes. It reads the disk: not for the UI thread.
pub fn why(f: &MemoryFile, now: Ms) -> String {
    if f.group != "Auto memory"
        || f.name == "MEMORY.md"
        || f.missing
        || is_kept(&f.path, f.mod_time)
    {
        return String::new();
    }
    if !f.warn.is_empty() {
        return f.warn.clone();
    }
    if (f.kind == "project" || f.kind == "reference")
        && f.mod_time != 0
        && now - f.mod_time > SETTLED
    {
        return format!(
            "unchanged since {} · is it still true?",
            day_month(f.mod_time)
        );
    }
    String::new()
}

/// The file of notes looked at and kept as they are: path → when they
/// were last changed then, in unix seconds.
fn kept_path() -> PathBuf {
    rush_state::dir::dir().join("memory-kept.json")
}

/// Keeps this process's reads and writes of it apart; another rush's
/// write lands whole, by a rename.
static KEPT_MU: Mutex<()> = Mutex::new(());

fn read_kept() -> BTreeMap<String, i64> {
    std::fs::read(kept_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Whether the note at path was kept as it is, and hasn't changed since.
/// It reads the disk: not for the UI thread.
pub fn is_kept(path: &str, mod_time: Ms) -> bool {
    let _g = KEPT_MU.lock().unwrap_or_else(|e| e.into_inner());
    read_kept().get(path) == Some(&mod_time.div_euclid(1000))
}

/// Marks the note at path, as it is now, as looked at and kept.
pub fn keep(path: &str) -> Result<()> {
    let at = mtime_secs(Path::new(path)).map_err(|e| e.to_string())?;
    let _g = KEPT_MU.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = read_kept(); // what another rush kept meanwhile stays
    m.insert(path.to_string(), at);
    m.retain(|p, _| std::fs::metadata(p).is_ok());
    let b = serde_json::to_vec(&m).map_err(|e| e.to_string())?;
    let to = kept_path();
    let dir = to.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".memory-kept-{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, b) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    std::fs::rename(&tmp, &to).map_err(|e| e.to_string())
}

fn mtime_secs(p: &Path) -> std::io::Result<i64> {
    let t = std::fs::metadata(p)?.modified()?;
    Ok(match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs_f64().ceil() as i64),
    })
}

/// What `rush memory help` says (memoryUsage).
pub const USAGE: &str = "rush memory — the memory notes agents keep for this project

  rush memory              the notes, what each is about, and how often sessions read it
  rush memory add <text>   remember something: a cheap model writes the note (its name,
                           description and type) and its line in MEMORY.md
      --type <t>           user, feedback, project or reference
      --name <n> --description <d>
                           with --type too, the note is written as given, no model asked
  rush memory review       the notes worth a look, and why
  rush memory tidy         add MEMORY.md lines for notes without one, take out lines
                           for notes that don't exist

It works on the project of the folder it's run in (through any worktree, the
repository's), for the session in RUSH_SESSION's account, or your first.";

/// add's --type, --name and --description (as --x v or --x=v); the rest,
/// and all after --, is the text (addFlags).
pub fn add_flags(args: &[String]) -> Result<(MemoryNote, Vec<String>)> {
    let mut n = MemoryNote::default();
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            words.extend(args[i + 1..].iter().cloned());
            break;
        }
        let (k, v) = match a.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        let dst = match k {
            "--type" => &mut n.kind,
            "--name" => &mut n.name,
            "--description" => &mut n.description,
            _ => {
                words.push(a.clone());
                i += 1;
                continue;
            }
        };
        let v = match v {
            Some(v) => v,
            None => {
                if i + 1 >= args.len() {
                    return Err(format!("{k} needs a value"));
                }
                i += 1;
                args[i].clone()
            }
        };
        *dst = v;
        i += 1;
    }
    if !n.kind.is_empty() && !valid_type(&n.kind) {
        return Err(format!("--type is one of {}", TYPES.join(", ")));
    }
    Ok((n, words))
}

/// How often sessions opened a note, as rush memory lists it (readsText).
pub fn reads_text(o: &Opens) -> String {
    match o.reads {
        _ if !o.counted || o.reads == 0 => "only its line in MEMORY.md".into(),
        1 => format!("opened once, {}", day_month(o.last_read)),
        n => format!("opened {n}×, last {}", day_month(o.last_read)),
    }
}

/// Go's "2 Jan", local.
pub fn day_month(ms: Ms) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, d) = local_date(ms);
    format!("{d} {}", MONTHS[(m as usize).saturating_sub(1) % 12])
}

/// The local year, month and day of ms.
fn local_date(ms: Ms) -> (i64, u32, u32) {
    let t = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: localtime_r writes only tm, which lives on this stack.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return (1970, 1, 1);
    }
    (
        tm.tm_year as i64 + 1900,
        tm.tm_mon as u32 + 1,
        tm.tm_mday as u32,
    )
}

#[cfg(test)]
rush_testenv::sandboxed!();

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn draft_asks_and_reads() {
        let prompt = RefCell::new(String::new());
        let n = draft(
            |_, p| {
                *prompt.borrow_mut() = p.to_string();
                Ok("Here:\n```json\n{\"name\":\"no-mocks\",\"title\":\"No mocks\",\"description\":\"tests hit a real db\",\"type\":\"feedback\",\"body\":\"Use the real db.\"}\n```".into())
            },
            "never mock the db",
            "",
        )
        .unwrap();
        assert_eq!((n.name.as_str(), n.kind.as_str()), ("no-mocks", "feedback"));
        let p = prompt.borrow();
        assert!(
            p.contains("never mock the db") && p.contains("Today is"),
            "{p}"
        );
        assert!(
            draft(|_, _| Ok("sorry".into()), "x", "").is_err(),
            "an answer that isn't a note should fail"
        );
        assert!(
            draft(|_, _| Ok(String::new()), " ", "").is_err(),
            "nothing to remember should fail"
        );
    }

    #[test]
    fn parse_draft_around() {
        let n = parse_draft(r#"Sure! {"name":"a","title":"A","description":"d","type":"opinion","body":"uses {x}"} Hope that helps {:}"#).unwrap();
        assert_eq!(n.body, "uses {x}");
        assert_eq!(n.kind, "project");
    }

    #[test]
    fn why_a_note_is_worth_a_look() {
        let dir = tempfile::tempdir().unwrap();
        let now = rush_core::time::now();
        let day = 24 * 3_600_000;
        let note = |kind: &str, age: Ms, warn: &str, name: &str| MemoryFile {
            group: "Auto memory".into(),
            name: "n".into(),
            path: dir.path().join(name).to_string_lossy().into_owned(),
            kind: kind.into(),
            mod_time: now - age,
            warn: warn.into(),
            ..Default::default()
        };
        assert_eq!(
            why(&note("feedback", 400 * day, "", "a.md"), now),
            "",
            "how to work doesn't go out of date by age"
        );
        assert!(
            why(&note("project", 90 * day, "", "b.md"), now).contains("still true"),
            "an old project note is worth a look"
        );
        assert_eq!(
            why(&note("project", 10 * day, "", "c.md"), now),
            "",
            "a recent project note isn't"
        );
        let mut f = note("user", 3_600_000, "repeats x", "n.md");
        assert_eq!(why(&f, now), "repeats x", "what's untidy is why");
        std::fs::write(&f.path, "x").unwrap();
        f.mod_time = mtime_secs(Path::new(&f.path)).unwrap() * 1000;
        keep(&f.path).unwrap();
        assert_eq!(why(&f, now), "", "a kept note isn't asked about again");
        f.mod_time += 60_000;
        assert_ne!(why(&f, now), "", "until it changes");
    }

    #[test]
    fn memory_add_flags() {
        let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let (n, words) = add_flags(&a(&[
            "--type=user",
            "likes",
            "--name",
            "tabs",
            "--",
            "--weird",
            "text",
        ]))
        .unwrap();
        assert_eq!((n.kind.as_str(), n.name.as_str()), ("user", "tabs"));
        assert_eq!(words.join(" "), "likes --weird text");
        assert!(
            add_flags(&a(&["x", "--type"])).is_err(),
            "a flag with no value is an error"
        );
        assert!(
            add_flags(&a(&["--type", "bogus", "x"])).is_err(),
            "an unknown type is an error"
        );
    }

    #[test]
    fn reads_in_words() {
        assert_eq!(reads_text(&Opens::default()), "only its line in MEMORY.md");
        let o = Opens {
            reads: 3,
            last_read: rush_core::time::now(),
            counted: true,
        };
        assert!(reads_text(&o).starts_with("opened 3×, last "));
    }
}
