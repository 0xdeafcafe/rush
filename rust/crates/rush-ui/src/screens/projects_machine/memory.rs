//! What an agent reads for its project, drawn: the memory view's list of
//! files (the editor below it is the session's), and the card an agent
//! asking to write one of its memory notes gets. Go: internal/ui/
//! memory.go (memoryLines :113 its list half and :144 its review line,
//! memRows :215, memKindHeads :294, memNoteRow :304, memNothing :336,
//! fileSize :601) and memcard.go (memPeek :32, indexCap :46,
//! inMemoryDir :55, memoryWrite :64, noteWrite :71, memNoteOf :78,
//! heredoc :101, heredocWrite :107, peekMem :78, splitFront :118, memTypeColour :146,
//! memTexts :163, memoryCard :189, memImpact :293, memText :330, memBtns
//! :343, btnTone :351, cardButtons :371, memDiff :407). A note's inline
//! markdown isn't styled yet (convo.Inline): its text is drawn plain.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use regex::Regex;
use rush_core::adapter::{MemoryFile, MemoryProblem, front_matter};
use rush_core::text::{Color, Line, width};
use rush_core::time::Ms;
use rush_core::tool::{Call, Input, Kind};

use super::rows::*;
use crate::header::short_path;
use crate::style::*;

/// A file size the way the memory view says it.
pub fn file_size(n: i64) -> String {
    if n < 1 << 10 {
        format!("{n} B")
    } else if n < 1 << 20 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    }
}

/// Tokens the way efficiency.Tokens says them.
pub fn eff_tokens(n: i64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 10_000 => format!("{:.0}k", n as f64 / 1e3),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// Tokens the way the conversation says them (convo.Tokens).
fn convo_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        let s = format!("{:.1}", n as f64 / 1e6);
        return format!("{}M", s.strip_suffix(".0").unwrap_or(&s));
    }
    if n >= 1000 {
        return format!("{:.0}k", n as f64 / 1e3);
    }
    n.to_string()
}

/// A row of the memory view, and the file it stands for ("mem:" and its
/// path; "" for a heading).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemRow {
    pub line: Line,
    pub r#ref: String,
}

/// The memory view's heading: what the agent reads and what it costs,
/// then a rule, then what's untidy and how many notes are worth a look
/// (review, judged as they're read), if anything; dir is whether the
/// agent keeps a memory folder, which f tidies.
pub fn memory_head(
    harness: &str,
    files: &[MemoryFile],
    problems: &[MemoryProblem],
    review: usize,
    dir: bool,
    w: usize,
) -> Vec<Line> {
    let up: i64 = files.iter().map(|f| f.up).sum();
    let left = ln([
        plain("  "),
        spb("Memory", SUB),
        plain("   "),
        sp(
            format!("what {harness} reads for this project, and when"),
            DIM,
        ),
    ]);
    let right = ln([
        sp(format!("≈{}", eff_tokens(up)), SUB),
        sp(
            format!(" tokens every session · {} files", files.len()),
            DIM,
        ),
        plain("  "),
    ]);
    let mut out = vec![
        fit(spread(left, right, w), w),
        ln([plain("  "), sp("─".repeat(w.saturating_sub(4)), FAINT)]),
    ];
    if problems.is_empty() && review == 0 {
        return out;
    }
    let mut left = match problems.len() {
        0 => ln([plain("  "), sp("memory notes worth a look", DIM)]),
        1 => ln([plain("  "), sp("! 1 thing to tidy", YELLOW)]),
        n => ln([plain("  "), sp(format!("! {n} things to tidy"), YELLOW)]),
    };
    let mut right = Line::new();
    if review > 0 {
        right.spans.push(sp(format!("{review} to review"), YELLOW));
        right.spans.push(sp(" · ", DIM));
    }
    if dir {
        right
            .spans
            .push(sp("pick a note: r reviews · f tidies the index", DIM));
        right.spans.push(plain("  "));
    }
    let room = w as isize - left.width() as isize - right.width() as isize - 5;
    if let Some(first) = problems.first()
        && room > 12
    {
        left.spans.push(sp(
            format!(
                " · {}",
                rush_draw::truncate(&first.title, room as usize, "…")
            ),
            DIM,
        ));
    }
    out.push(fit(spread(left, right, w), w));
    out
}

/// A row for each file, under its group's name and when its files reach
/// the model. On the right, what a file costs: ≈ tokens every session in
/// full colour, ≈ tokens when it's loaded later faint, nothing for what's
/// never sent. A memory's notes go under a head for their kind, each by
/// what it says, with how often sessions opened it (its reads, counted
/// as it was read). With no files, it says nothing.
pub fn mem_rows(
    files: &[MemoryFile],
    when: &BTreeMap<String, String>,
    picked: &str,
    w: usize,
    now: Ms,
    focused: bool,
    nothing: &str,
) -> Vec<MemRow> {
    let mut out = Vec::new();
    let (mut group, mut kind) = ("", "-");
    for f in files {
        let note = f.group == "Auto memory" && f.name != "MEMORY.md" && !f.missing;
        if note && f.kind != kind {
            kind = &f.kind;
            out.push(MemRow {
                line: fit(
                    ln([
                        plain("    "),
                        sp(mem_kind_head(kind), mem_type_colour(kind)),
                        plain("  "),
                        sp(mem_kind_about(kind), FAINT),
                    ]),
                    w,
                ),
                r#ref: String::new(),
            });
        }
        if f.group != group {
            group = &f.group;
            let mut head = ln([plain("  "), spb(group, TEXT)]);
            let w_when = ln([
                sp(when.get(group).cloned().unwrap_or_default(), FAINT),
                plain("  "),
            ]);
            // The memory's folder gives way to when.
            if group == "Auto memory" {
                let dir = tildify(
                    &Path::new(&f.path)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                let room = w as isize - 1 - head.width() as isize - w_when.width() as isize - 6;
                if room > 10 {
                    head.spans.push(plain("  "));
                    head.spans
                        .push(sp(truncate_left(&dir, room as usize), FAINT));
                }
            }
            out.push(MemRow {
                line: fit(spread(head, w_when, w.saturating_sub(1)), w),
                r#ref: String::new(),
            });
        }
        let r#ref = format!("mem:{}", f.path);
        if note {
            out.push(mem_note_row(f, r#ref == picked, w, now, focused));
            continue;
        }
        let mut meta: Vec<rush_core::text::Span> = Vec::new();
        if f.up > 0 {
            meta.push(sp(format!("≈{}", eff_tokens(f.up)), SUB));
        }
        if f.later > 0 {
            meta.push(sp(format!("≈{} {}", eff_tokens(f.later), f.when), FAINT));
        }
        let ago = dur(now - f.mod_time);
        meta.push(if f.missing && !f.warn.is_empty() {
            sp("missing", DIM)
        } else if f.missing {
            sp("not written yet", DIM)
        } else if f.up == 0 && f.later == 0 {
            sp(format!("{} · {ago} ago", file_size(f.size)), DIM)
        } else {
            sp(format!("{ago} ago"), DIM)
        });
        let mut right = Line::new();
        for (i, s) in meta.into_iter().enumerate() {
            if i > 0 {
                right.spans.push(sp(" · ", DIM));
            }
            right.spans.push(s);
        }
        let mark = if f.missing && !f.warn.is_empty() {
            sp("◇ ", YELLOW)
        } else if f.missing {
            sp("◇ ", FAINT)
        } else {
            sp("◆ ", BLUE)
        };
        let mut left = if f.sub {
            ln([plain("      "), sp("↳ ", FAINT)])
        } else {
            ln([plain("    "), mark])
        };
        left.spans.push(sp(&f.name, TEXT));
        if !f.kind.is_empty() {
            left.spans.push(plain("  "));
            left.spans.push(sp(&f.kind, SUB));
        }
        let (note, tone) = if f.warn.is_empty() {
            (&f.about, DIM)
        } else {
            (&f.warn, YELLOW)
        };
        if !note.is_empty() {
            let room = w as isize - left.width() as isize - right.width() as isize - 8;
            if room > 12 {
                left.spans.push(plain("  "));
                left.spans.push(sp(
                    rush_draw::truncate(&one_line(note), room as usize, "…").into_owned(),
                    tone,
                ));
            }
        }
        right.spans.push(plain("  "));
        let mut r = spread(left, right, w.saturating_sub(1));
        if r#ref == picked {
            r = picked1(r, w, focused);
        }
        out.push(MemRow {
            line: fit(r, w),
            r#ref,
        });
    }
    if files.is_empty() {
        out.push(MemRow {
            line: fit(ln([plain("    "), sp(nothing, DIM)]), w),
            r#ref: String::new(),
        });
    }
    out
}

/// The head over a memory's notes of kind (memKindHeads).
pub fn mem_kind_head(kind: &str) -> &'static str {
    match kind {
        "user" => "About you",
        "feedback" => "How to work",
        "project" => "This project",
        "reference" => "Where things are",
        "" => "No type",
        _ => "",
    }
}

/// What a memory's notes of kind hold (memKindAbout).
pub fn mem_kind_about(kind: &str) -> &'static str {
    match kind {
        "user" => "who you are, what you prefer",
        "feedback" => "corrections and approaches to keep",
        "project" => "work going on, goals, constraints",
        "reference" => "outside the code",
        "" => "no type: in its frontmatter",
        _ => "",
    }
}

/// A memory note: what it's about (or what's untidy about it), then on
/// the right how often sessions opened it and when it changed
/// (memNoteRow).
fn mem_note_row(f: &MemoryFile, picked: bool, w: usize, now: Ms, focused: bool) -> MemRow {
    let mut meta = Vec::new();
    match f.reads {
        n if n > 1 => meta.push(sp(format!("opened {n}×"), SUB)),
        1 => meta.push(sp("opened once", SUB)),
        _ => {}
    }
    if f.mod_time != 0 {
        meta.push(sp(format!("{} ago", dur(now - f.mod_time)), DIM));
    }
    let mut right = Line::new();
    for (i, s) in meta.into_iter().enumerate() {
        if i > 0 {
            right.spans.push(sp(" · ", DIM));
        }
        right.spans.push(s);
    }
    let mut left = ln([plain("      "), sp("● ", mem_type_colour(&f.kind))]);
    let about = if f.about.is_empty() {
        &f.name
    } else {
        &f.about
    };
    let room = w as isize - left.width() as isize - right.width() as isize - 6;
    if !f.warn.is_empty() {
        let a = rush_draw::truncate(
            &one_line(about),
            16.max(room - width(&f.warn) as isize - 4) as usize,
            "…",
        )
        .into_owned();
        let aw = width(&a) as isize;
        left.spans.push(sp(a, TEXT));
        left.spans.push(sp(
            rush_draw::truncate(
                &format!("  ! {}", one_line(&f.warn)),
                4.max(room - aw) as usize,
                "…",
            )
            .into_owned(),
            YELLOW,
        ));
    } else {
        left.spans.push(sp(
            rush_draw::truncate(&one_line(about), 4.max(room) as usize, "…").into_owned(),
            TEXT,
        ));
    }
    right.spans.push(plain("  "));
    let mut r = spread(left, right, w.saturating_sub(1));
    if picked {
        r = picked1(r, w, focused);
    }
    MemRow {
        line: fit(r, w),
        r#ref: format!("mem:{}", f.path),
    }
}

/// What the memory view says with no files: still reading, or nothing.
pub fn mem_nothing(read: bool) -> &'static str {
    if read {
        "Nothing found for this project."
    } else {
        "Reading…"
    }
}

/// A row picked in a session's view: on the selection's ground, its
/// first cell a bar, orange while the view has the keys (picked1).
pub fn picked1(l: Line, w: usize, focused: bool) -> Line {
    let bar = sp("▍", if focused { ORANGE } else { FAINT });
    let mut body = fit(l, w.saturating_sub(1).max(1) + 1);
    // The row's first cell gives way to the bar.
    let mut out = ln([bar]);
    let mut skip = 1;
    for mut s in body.spans.drain(..) {
        if skip > 0 {
            let n = s.text.chars().count();
            if n <= skip {
                skip -= n;
                continue;
            }
            s.text = s.text.chars().skip(skip).collect();
            skip = 0;
        }
        out.spans.push(s);
    }
    highlight(fit(out, w), w)
}

/// s's last w cells, an ellipsis before them when any were cut: Go's
/// ansi.TruncateLeft(s, width(s)-w, "…"), so the ellipsis is one more.
fn truncate_left(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for c in chars.iter().rev() {
        if width(&out) + width(c.encode_utf8(&mut [0; 4])) > w {
            break;
        }
        out.insert(0, *c);
    }
    format!("…{out}")
}

// ---- The memory card ----

/// How many of MEMORY.md's lines a new session reads: auto memory loads
/// the index up to here.
pub const INDEX_CAP: usize = 200;

/// What a memory card needs from disk: the note as it is now, whether
/// it's there, and the notes beside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemPeek {
    pub old: String,
    pub had: bool,
    /// -1 when it isn't in a memory folder.
    pub notes: i64,
    /// Every note's bytes (this one's as it is now), the index's lines and
    /// bytes, and the notes there by name, for links.
    pub bytes: i64,
    pub index_lines: usize,
    pub index_bytes: i64,
    pub names: Option<HashSet<String>>,
}

/// Whether path is a note in a Claude config dir's projects/<project>/
/// memory folder, which is where auto memory lives whichever config dir.
pub fn in_memory_dir(path: &str) -> bool {
    let p = Path::new(path);
    let name = |q: Option<&Path>| {
        q.and_then(|q| q.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let dir = p.parent();
    p.extension().is_some_and(|e| e == "md")
        && name(dir) == "memory"
        && name(dir.and_then(Path::parent).and_then(Path::parent)) == "projects"
}

/// Whether a Write or Edit of path with content writes a memory note: one
/// in a memory folder, or a file with a note's frontmatter. The index,
/// MEMORY.md, keeps the plain card.
pub fn memory_write(path: &str, content: &str) -> bool {
    if Path::new(path)
        .file_name()
        .is_some_and(|n| n == "MEMORY.md")
    {
        return false;
    }
    if in_memory_dir(path) {
        return true;
    }
    let fm = front_matter(content.as_bytes());
    ["name", "description", "type"]
        .iter()
        .all(|k| fm.get(*k).is_some_and(|v| !v.is_empty()))
}

/// The write a memory card asks about: a Write or Edit as it is, or a
/// shell heredoc into a note read as a Write (appends, for >>)
/// (noteWrite).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteWrite {
    pub call: Call,
    pub appends: bool,
    pub shell: bool,
}

/// The memory note call writes, when it writes one (memNoteOf).
pub fn mem_note_of(call: &Call) -> Option<NoteWrite> {
    let mut n = NoteWrite {
        call: call.clone(),
        ..NoteWrite::default()
    };
    if n.call.kind == Kind::Shell {
        let (path, text, appends) = heredoc_write(&n.call.input.command)?;
        if !in_memory_dir(&path) {
            return None;
        }
        n.call.kind = Kind::Write;
        n.call.input = Input {
            path,
            content: text,
            ..Input::default()
        };
        (n.appends, n.shell) = (appends, true);
    }
    if n.call.kind != Kind::Write && n.call.kind != Kind::Edit {
        return None;
    }
    memory_write(&n.call.input.path, &n.call.input.content).then_some(n)
}

/// A command that only writes a heredoc to a file: cat > f <<'EOF', cat
/// <<EOF >> f, or tee [-a] f <<EOF, and nothing after (heredoc).
static HEREDOC: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?s)^\s*(?:cat\s+(>>?)\s*("[^"]+"|'[^']+'|[^\s<>|;&]+)\s*<<-?\s*(['"]?)(\w+)['"]?"#,
        r#"|cat\s*<<-?\s*(['"]?)(\w+)['"]?\s*(>>?)\s*("[^"]+"|'[^']+'|[^\s<>|;&]+)"#,
        r#"|tee\s+(-a\s+)?("[^"]+"|'[^']+'|[^\s<>|;&-][^\s<>|;&]*)\s*<<-?\s*(['"]?)(\w+)['"]?(?:\s*>\s*/dev/null)?)[ \t]*\n"#,
    ))
    .expect("the heredoc pattern")
});

/// The file a heredoc-only command writes, what it writes, and whether
/// it appends (heredocWrite).
pub fn heredoc_write(cmd: &str) -> Option<(String, String, bool)> {
    let c = HEREDOC.captures(cmd)?;
    let g = |i: usize| c.get(i).map_or("", |m| m.as_str());
    let (op, path, word) = if !g(2).is_empty() {
        (g(1), g(2), g(4))
    } else if !g(8).is_empty() {
        (g(7), g(8), g(6))
    } else {
        (if g(9).is_empty() { ">" } else { ">>" }, g(10), g(12))
    };
    let rest = &cmd[c.get(0).expect("whole").end()..];
    let (body, after) = if rest.starts_with(&format!("{word}\n")) || rest.trim() == word {
        ("", rest.strip_prefix(word).unwrap_or(rest))
    } else {
        rest.split_once(&format!("\n{word}"))?
    };
    if !after.trim().is_empty() {
        return None; // more runs after it
    }
    let mut path = path.trim_matches(['"', '\'']).to_string();
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        for h in ["~", "$HOME", "${HOME}"] {
            if let Some(rest) = path.strip_prefix(&format!("{h}/")) {
                path = Path::new(&home).join(rest).to_string_lossy().into_owned();
            }
        }
    }
    if !Path::new(&path).is_absolute() {
        return None;
    }
    Some((path, format!("{body}\n"), op == ">>"))
}

/// The buttons a memory card has: one written by a shell command has no
/// always, which would allow the command, cat >> and all, not the note
/// (memBtnsFor).
pub fn mem_btns_for(shell: bool) -> &'static [[&'static str; 3]] {
    if shell { &MEM_BTNS[..3] } else { &MEM_BTNS }
}

/// What's on disk for the note at path: read off the UI.
pub fn peek_mem(path: &str) -> MemPeek {
    let mut p = MemPeek {
        notes: -1,
        ..MemPeek::default()
    };
    if let Ok(b) = std::fs::read_to_string(path) {
        p.old = b;
        p.had = true;
    }
    if !in_memory_dir(path) {
        return p;
    }
    p.notes = 0;
    let mut names = HashSet::new();
    let dir = Path::new(path).parent().unwrap_or(Path::new("."));
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    for f in files {
        let b = std::fs::read_to_string(&f).unwrap_or_default();
        let base = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if base == "MEMORY.md" {
            p.index_bytes = b.len() as i64;
            p.index_lines = b.trim_end_matches('\n').matches('\n').count() + 1;
            continue;
        }
        p.notes += 1;
        p.bytes += b.len() as i64;
        names.insert(base.trim_end_matches(".md").to_string());
        if let Some(n) = front_matter(b.as_bytes()).get("name")
            && !n.is_empty()
        {
            names.insert(n.clone());
        }
    }
    p.names = Some(names);
    p
}

/// A note's frontmatter, fences and all, and the text after.
pub fn split_front(s: &str) -> (&str, &str) {
    if !s.starts_with("---\n") && !s.starts_with("---\r\n") {
        return ("", s);
    }
    let first = s.find('\n').map_or(s.len(), |i| i + 1);
    let rest = &s[first..];
    let mut i = 0;
    while i < rest.len() {
        let j = rest[i..].find('\n');
        let line = match j {
            Some(j) => &rest[i..i + j],
            None => &rest[i..],
        };
        if line.trim() == "---" {
            let Some(j) = j else {
                return (s, "");
            };
            let end = first + i + j + 1;
            return (&s[..end], &s[end..]);
        }
        match j {
            Some(j) => i += j + 1,
            None => break,
        }
    }
    ("", s)
}

/// Each type of note in its own colour.
pub fn mem_type_colour(t: &str) -> Color {
    match t {
        "user" => BLUE,
        "feedback" => YELLOW,
        "project" => GREEN,
        "reference" => QUEUE,
        _ => SUB,
    }
}

/// The note before and after a call: the Write's text, or the Edit's on
/// top of the note as it is (just the edit itself until that's read), or
/// the note with what's appended.
pub fn mem_texts(
    input: &Input,
    edit: bool,
    appends: bool,
    peek: &MemPeek,
    known: bool,
) -> (String, String) {
    if appends && known && peek.had {
        return (peek.old.clone(), format!("{}{}", peek.old, input.content));
    }
    if appends {
        return (String::new(), input.content.clone());
    }
    if edit && known && peek.had {
        let mut after = peek.old.clone();
        for e in &input.edits {
            after = if e.all {
                after.replace(&e.old, &e.new)
            } else {
                after.replacen(&e.old, &e.new, 1)
            };
        }
        return (peek.old.clone(), after);
    }
    if edit {
        let o: Vec<&str> = input.edits.iter().map(|e| e.old.as_str()).collect();
        let n: Vec<&str> = input.edits.iter().map(|e| e.new.as_str()).collect();
        return (o.join("\n"), n.join("\n"));
    }
    if known && peek.had {
        return (peek.old.clone(), input.content.clone());
    }
    (String::new(), input.content.clone())
}

/// A button drawn on a card, where a click presses it: x and y from the
/// card's top left, three rows high.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardBtn {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub key: String,
}

/// The memory card's buttons: the key each presses, what shows for it,
/// and its name.
pub const MEM_BTNS: [[&str; 3]; 4] = [
    ["enter", "↵", "Remember"],
    ["e", "e", "Edit it first"],
    ["n", "n", "Skip"],
    ["a", "a", "Always ok"],
];

/// A button's colour, by what its key does: yes green, no red, for good
/// blue, don't-ask yellow, edit first purple.
pub fn btn_tone(key: &str) -> Color {
    match key {
        "y" | "enter" => GREEN,
        "n" => RED,
        "a" => BLUE,
        "o" => YELLOW,
        "e" => QUEUE,
        _ => ORANGE,
    }
}

/// The ground's own colour, for words on a filled button.
const INK: Color = TAB_ON_FG;

/// A card's buttons, three rows starting y rows into the card, and where
/// each is: each outlined in the colour of what it does, its key bold in
/// it. The picked one is enter's: while the card has the keys it's filled
/// solid, as is one under the pointer.
pub fn card_buttons(
    bs: &[[&str; 3]],
    focus: bool,
    pick: usize,
    hover: &str,
    y: usize,
) -> ([Line; 3], Vec<CardBtn>) {
    let mut rows: [Line; 3] = Default::default();
    let mut btns = Vec::new();
    let mut x = 3; // past the edge and its two spaces
    for (i, b) in bs.iter().enumerate() {
        let iw = width(&format!(" {} {}  ", b[1], b[2]));
        let tone = btn_tone(b[0]);
        if i > 0 {
            for r in &mut rows {
                r.spans.push(plain("  "));
            }
            x += 2;
        }
        if focus && i == pick || hover == format!("btn:{}", b[0]) {
            rows[0].spans.push(sp("▄".repeat(iw + 2), tone));
            let mut mid = fit(ln([plain(format!("  {} {}", b[1], b[2]))]), iw + 2);
            for s in &mut mid.spans {
                s.style = rush_core::text::Style {
                    fg: INK,
                    bg: tone,
                    mods: rush_core::text::Mods::BOLD,
                };
            }
            rows[1].spans.extend(mid.spans);
            rows[2].spans.push(sp("▀".repeat(iw + 2), tone));
        } else {
            let name = if i == pick {
                spb(b[2], BRIGHT)
            } else {
                sp(b[2], TEXT)
            };
            rows[0]
                .spans
                .push(sp(format!("╭{}╮", "─".repeat(iw)), tone));
            rows[1].spans.extend([
                sp("│", tone),
                plain(" "),
                spb(b[1], tone),
                plain(" "),
                name,
                plain("  "),
                sp("│", tone),
            ]);
            rows[2]
                .spans
                .push(sp(format!("╰{}╯", "─".repeat(iw)), tone));
        }
        btns.push(CardBtn {
            x,
            y,
            w: iw + 2,
            key: b[0].to_string(),
        });
        x += iw + 2;
    }
    (rows, btns)
}

/// What saving a note means, in a few rows: how much memory the project
/// has, what every new session reads more of, and where it's kept. Tokens
/// are estimated, a token to four bytes.
pub fn mem_impact(
    p: &MemPeek,
    update: bool,
    path: &str,
    about: &str,
    text: &str,
    w: usize,
) -> Vec<Line> {
    let tok = |n: i64| format!("~{} tokens", convo_tokens((n / 4).max(1)));
    let label = |s: &str| sp(format!("{s:<8}  "), SUB);
    let mut notes = p.notes;
    if !update && !p.had {
        notes += 1;
    }
    let index = if p.index_lines >= INDEX_CAP {
        sp(
            format!(
                "index {} lines: past {INDEX_CAP}, new sessions miss the rest",
                p.index_lines
            ),
            YELLOW,
        )
    } else {
        sp(format!("index {} of {INDEX_CAP} lines", p.index_lines), DIM)
    };
    let have = format!(
        "{notes} note{} · {}",
        if notes == 1 { "" } else { "s" },
        tok(p.bytes + text.len() as i64 - p.old.len() as i64)
    );
    let mut out = vec![ln([label("memory"), sp(format!("{have} · "), DIM), index])];
    let impact = if update {
        let d = text.len() as i64 - p.old.len() as i64;
        let change = if d == 0 {
            "the same size".to_string()
        } else {
            format!("{d:+} bytes")
        };
        format!(
            "a session reads it when it's relevant: {change} ({} in all); the index stays as it is",
            tok(text.len() as i64)
        )
    } else {
        // The index gets a line naming it: its description and a link.
        format!(
            "every new session here starts with the index, a line ({}) longer; the note ({}) is read only when it's relevant",
            tok(about.len() as i64 + 40),
            tok(text.len() as i64)
        )
    };
    for (i, l) in wrap(&impact, w.saturating_sub(10)).into_iter().enumerate() {
        out.push(if i == 0 {
            ln([label("impact"), sp(l, DIM)])
        } else {
            ln([plain(" ".repeat(10)), sp(l, DIM)])
        });
    }
    out.push(ln([
        label("file"),
        sp(short_path(&tildify(path), w.saturating_sub(10)), FAINT),
    ]));
    out
}

/// A note's text as it reads, wrapped to w.
pub fn mem_text(body: &str, w: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for l in body.trim_matches('\n').split('\n') {
        for wl in wrap(&l.replace('\t', "  "), w) {
            out.push(ln([sp(wl, TEXT)]));
        }
    }
    out
}

/// What changes between a note's old text and its new: the lines between
/// what they share at the start and at the end, the old red and the new
/// green, with a line of what's around them.
pub fn mem_diff(old: &str, cur: &str, w: usize) -> Vec<Line> {
    let a: Vec<&str> = old.trim_matches('\n').split('\n').collect();
    let b: Vec<&str> = cur.trim_matches('\n').split('\n').collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    let mut out = Vec::new();
    let mut line = |sign: &str, col: Color, l: &str| {
        for (i, wl) in wrap(&l.replace('\t', "  "), w.saturating_sub(2))
            .into_iter()
            .enumerate()
        {
            let s = if i > 0 { " " } else { sign };
            out.push(ln([sp(format!("{s} "), col), sp(wl, TEXT)]));
        }
    };
    let around = |l: &str| {
        ln([
            plain("  "),
            sp(
                rush_draw::truncate(l, w.saturating_sub(2), "…").into_owned(),
                DIM,
            ),
        ])
    };
    let mut lines = Vec::new();
    if pre > 0 {
        lines.push(around(a[pre - 1]));
    }
    for l in &a[pre..a.len() - suf] {
        line("−", RED, l);
    }
    for l in &b[pre..b.len() - suf] {
        line("+", GREEN, l);
    }
    lines.append(&mut out);
    if suf > 0 {
        lines.push(around(a[a.len() - suf]));
    }
    lines
}

/// What the card needs of the step asking: its input, whether it's an
/// Edit, what's on disk (known once read), and where the keys are.
pub struct MemCard<'a> {
    pub input: &'a Input,
    pub edit: bool,
    /// A shell heredoc appending to the note, or writing it.
    pub appends: bool,
    pub shell: bool,
    pub peek: &'a MemPeek,
    pub known: bool,
    pub focus: bool,
    pub pick: usize,
    pub hover: &'a str,
}

/// The card for a memory note a step asks to write, w wide and in at most
/// max_h rows, and the buttons on it.
pub fn memory_card(c: &MemCard, w: usize, max_h: usize) -> (Vec<Line>, Vec<CardBtn>) {
    static LINK: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\[\[[^\]\n]+\]\]").expect("a link"));
    let path = &c.input.path;
    let (old_text, new_text) = mem_texts(c.input, c.edit, c.appends, c.peek, c.known);
    let update = c.edit || c.appends || c.known && c.peek.had;
    let fm = front_matter(new_text.as_bytes());
    let body = split_front(&new_text).1;
    let old_body = split_front(&old_text).1;
    let get = |k: &str| fm.get(k).cloned().unwrap_or_default();

    let edge = if c.focus { ORANGE } else { YELLOW };
    let e = |s: &str| sp(s, edge);
    let mut out: Vec<Line> = Vec::new();
    let row = |out: &mut Vec<Line>, l: Line| {
        let mut r = ln([e("│")]);
        join(&mut r, fit(cat(ln([plain("  ")]), l), w.saturating_sub(2)));
        r.spans.push(e("│"));
        out.push(on_ground(r, w, CHROME_BG));
    };
    let tw = w.saturating_sub(6).max(20);

    // The top edge: what it asks, and the note's type and scope.
    let title = if update {
        "Update this memory?"
    } else {
        "Remember this?"
    };
    let ty = get("type");
    let left = ln([
        e("╭─ "),
        sp("◆", mem_type_colour(&ty)),
        plain(" "),
        spb(title, TEXT),
        plain(" "),
    ]);
    let mut right = Line::new();
    let mut meta = Vec::new();
    if !ty.is_empty() {
        meta.push(sp(ty.clone(), mem_type_colour(&ty)));
    }
    if in_memory_dir(path) {
        meta.push(sp("this project", SUB));
    }
    if !meta.is_empty() {
        right.spans.push(plain(" "));
        for (i, s) in meta.into_iter().enumerate() {
            if i > 0 {
                right.spans.push(sp(" · ", DIM));
            }
            right.spans.push(s);
        }
        right.spans.push(plain(" "));
    }
    let fill = (w as isize - left.width() as isize - right.width() as isize - 2).max(1) as usize;
    let mut top = left;
    top.spans.push(e(&"─".repeat(fill)));
    join(&mut top, right);
    top.spans.push(e("─╮"));
    out.push(on_ground(top, w, CHROME_BG));
    row(&mut out, Line::new());

    // What it says: its description in bold, then its text, or what
    // changes in it.
    let base = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().trim_end_matches(".md").to_string())
        .unwrap_or_default();
    let about = [get("description"), get("name"), base]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    for l in wrap(&one_line(&about), tw) {
        row(&mut out, ln([spb(l, TEXT)]));
    }
    row(&mut out, Line::new());
    let mut text = if update {
        mem_diff(old_body, body, tw)
    } else {
        mem_text(body, tw)
    };
    // It keeps to max_h: the text gives way, the rest (15 rows) doesn't.
    let room = max_h.saturating_sub(15).max(3);
    if max_h > 0 && text.len() > room {
        let more = text.len() - room + 1;
        text.truncate(room - 1);
        text.push(ln([sp(format!("… {more} more lines"), DIM)]));
    }
    for l in text {
        row(&mut out, l);
    }
    row(&mut out, Line::new());

    // The foot: how much memory there is, what this adds to every
    // session, where it goes, the notes it links to, and whether it's new.
    if in_memory_dir(path) && c.known && c.peek.notes >= 0 {
        for l in mem_impact(c.peek, update, path, &get("description"), &new_text, tw) {
            row(&mut out, l);
        }
        row(&mut out, Line::new());
    }
    let mut links = Line::new();
    for (i, m) in LINK.find_iter(body).take(6).enumerate() {
        if i == 0 {
            links.spans.extend([sp("links", SUB), plain("  ")]);
        } else {
            links.spans.push(plain(" "));
        }
        let l = m.as_str();
        let known = c
            .peek
            .names
            .as_ref()
            .is_none_or(|n| n.contains(l.trim_matches(['[', ']'])));
        links.spans.push(if known {
            sp(l, BLUE)
        } else {
            sp(format!("{l} (none yet)"), FAINT)
        });
    }
    let status = if update { "update" } else { "new" };
    row(
        &mut out,
        spread(
            links,
            ln([sp(status, DIM), plain("  ")]),
            w.saturating_sub(4),
        ),
    );
    row(&mut out, Line::new());

    let (rows, btns) = card_buttons(mem_btns_for(c.shell), c.focus, c.pick, c.hover, out.len());
    for r in rows {
        row(&mut out, r);
    }

    // The bottom edge says how to get to it, or back from it.
    let hint = if c.focus {
        " ←→ pick · enter · esc back to typing "
    } else {
        " ↑ to answer "
    };
    let fill = w.saturating_sub(3 + width(hint)).max(1);
    out.push(on_ground(
        ln([e(&format!("╰{}", "─".repeat(fill))), sp(hint, DIM), e("─╯")]),
        w,
        CHROME_BG,
    ));
    (out, btns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::tool::Replace;

    fn text(l: &[Line]) -> Vec<String> {
        l.iter()
            .map(|l| text_of(l).trim_end().to_string())
            .collect()
    }

    #[test]
    fn sizes_and_tokens() {
        for (n, want) in [(12, "12 B"), (2048, "2.0 KB"), (3 << 20, "3.0 MB")] {
            assert_eq!(file_size(n), want);
        }
        for (n, want) in [
            (999, "999"),
            (1500, "1.5k"),
            (12_345, "12k"),
            (2_500_000, "2.5M"),
        ] {
            assert_eq!(eff_tokens(n), want);
        }
        for (n, want) in [
            (999, "999"),
            (1500, "2k"),
            (1_000_000, "1M"),
            (2_500_000, "2.5M"),
        ] {
            assert_eq!(convo_tokens(n), want);
        }
    }

    #[test]
    fn memory_dirs_and_notes() {
        assert!(in_memory_dir("/u/.claude/projects/-src-x/memory/a.md"));
        assert!(in_memory_dir("/u/.claude-work/projects/p/memory/b.md"));
        assert!(!in_memory_dir("/u/.claude/projects/p/memory/b.txt"));
        assert!(!in_memory_dir("/u/.claude/projects/p/notes/b.md"));
        assert!(!memory_write("/u/.claude/projects/p/memory/MEMORY.md", ""));
        assert!(memory_write("/u/.claude/projects/p/memory/a.md", ""));
        assert!(memory_write(
            "/src/x/note.md",
            "---\nname: a\ndescription: b\ntype: user\n---\nhi"
        ));
        assert!(!memory_write("/src/x/note.md", "---\nname: a\n---\nhi"));
    }

    #[test]
    fn front_and_body() {
        let cases = [
            ("no front", ("", "no front")),
            (
                "---\nname: a\n---\nbody\n",
                ("---\nname: a\n---\n", "body\n"),
            ),
            (
                "---\r\nname: a\r\n---\r\nb",
                ("---\r\nname: a\r\n---\r\n", "b"),
            ),
            ("---\nname: a\n---", ("---\nname: a\n---", "")),
            (
                "---\nname: a\nnever closed",
                ("", "---\nname: a\nnever closed"),
            ),
        ];
        for (s, want) in cases {
            assert_eq!(split_front(s), want, "{s:?}");
        }
    }

    #[test]
    fn texts_before_and_after() {
        let peek = MemPeek {
            old: "a b a".into(),
            had: true,
            ..MemPeek::default()
        };
        let mut i = Input {
            edits: vec![Replace {
                old: "a".into(),
                new: "c".into(),
                ..Replace::default()
            }],
            ..Input::default()
        };
        assert_eq!(
            mem_texts(&i, true, false, &peek, true),
            ("a b a".into(), "c b a".into())
        );
        i.edits[0].all = true;
        assert_eq!(
            mem_texts(&i, true, false, &peek, true),
            ("a b a".into(), "c b c".into())
        );
        assert_eq!(
            mem_texts(&i, true, false, &peek, false),
            ("a".into(), "c".into())
        );
        i.content = "new".into();
        assert_eq!(
            mem_texts(&i, false, false, &peek, true),
            ("a b a".into(), "new".into())
        );
        assert_eq!(
            mem_texts(&i, false, false, &peek, false),
            (String::new(), "new".into())
        );
    }

    #[test]
    fn diffs_show_what_changes_and_around_it() {
        let got = text(&mem_diff("one\ntwo\nthree", "one\n2\nthree", 40));
        assert_eq!(got, ["  one", "− two", "+ 2", "  three"]);
    }

    #[test]
    fn buttons_and_where_they_are() {
        let (rows, btns) = card_buttons(&MEM_BTNS, false, 0, "", 7);
        let t: Vec<String> = rows.iter().map(text_of).collect();
        assert_eq!(
            t[1],
            "│ ↵ Remember  │  │ e Edit it first  │  │ n Skip  │  │ a Always ok  │"
        );
        assert_eq!(
            btns[0],
            CardBtn {
                x: 3,
                y: 7,
                w: 15,
                key: "enter".into()
            }
        );
        assert_eq!(btns[1].x, 3 + 15 + 2);
        let (rows, _) = card_buttons(&MEM_BTNS, true, 0, "", 0);
        assert!(text_of(&rows[0]).starts_with("▄▄▄▄"));
        assert_eq!(rows[1].spans[0].style.bg, GREEN);
    }

    #[test]
    fn a_new_note_card() {
        let i = Input {
            path: "/u/.claude/projects/p/memory/tabs.md".into(),
            content: "---\nname: tabs\ndescription: Alex wants tabs\ntype: feedback\n---\nUse tabs, see [[style]] and [[gone]].\n".into(),
            ..Input::default()
        };
        let peek = MemPeek {
            notes: 2,
            bytes: 400,
            index_lines: 3,
            names: Some(["style".to_string()].into()),
            ..MemPeek::default()
        };
        let c = MemCard {
            input: &i,
            edit: false,
            appends: false,
            shell: false,
            peek: &peek,
            known: true,
            focus: false,
            pick: 0,
            hover: "",
        };
        let (lines, btns) = memory_card(&c, 90, 40);
        let t = text(&lines);
        assert!(t[0].starts_with("╭─ ◆ Remember this? ─"), "{}", t[0]);
        assert!(t[0].ends_with("feedback · this project ─╮"), "{}", t[0]);
        assert!(t.iter().any(|l| l.contains("│  Alex wants tabs")));
        assert!(
            t.iter()
                .any(|l| l.contains("Use tabs, see [[style]] and [[gone]]."))
        );
        assert!(t.iter().any(|l| l.contains("memory    3 notes · ~")));
        assert!(
            t.iter()
                .any(|l| l.contains("links  [[style]] [[gone]] (none yet)"))
        );
        assert!(t.iter().any(|l| l.contains("new  │")));
        assert!(t.last().unwrap().contains(" ↑ to answer ─╯"));
        assert_eq!(btns.len(), 4);
        assert!(lines.iter().all(|l| l.width() == 90));
    }

    #[test]
    fn heredoc_writes() {
        let home = std::env::var("HOME").unwrap();
        let ok = |p: &str, t: &str, a: bool| Some((p.to_string(), t.to_string(), a));
        for (cmd, want) in [
            (
                "cat >> ~/m/a.md <<'EOF'\nhi\nthere\nEOF",
                ok(&format!("{home}/m/a.md"), "hi\nthere\n", true),
            ),
            (
                "cat > \"/x/a.md\" << EOF\nhi\nEOF\n",
                ok("/x/a.md", "hi\n", false),
            ),
            (
                "cat <<'NOTE' > $HOME/a.md\nhi\nNOTE",
                ok(&format!("{home}/a.md"), "hi\n", false),
            ),
            (
                "tee -a /x/a.md <<'EOF' >/dev/null\nhi\nEOF",
                ok("/x/a.md", "hi\n", true),
            ),
            ("cat >> /x/a.md <<'EOF'\nhi\nEOF\nrm -rf /", None), // more after it
            ("cat >> a.md <<'EOF'\nhi\nEOF", None),              // relative
            ("echo hi >> /x/a.md", None),
        ] {
            assert_eq!(heredoc_write(cmd), want, "{cmd:?}");
        }
    }

    /// A note written by a shell heredoc gets the memory card too: cat >>
    /// appends to the note as it is, and there's no always, which would
    /// allow the command rather than the note (TestHeredocMemoryCard).
    #[test]
    fn a_heredoc_note_card() {
        let path = "/u/.claude/projects/p/memory/real-db.md";
        let note = "---\nname: real-db\ndescription: Run the real db in integration tests, never mocks\ntype: feedback\n---\nUse the real db.\n";
        let call = Call {
            kind: Kind::Shell,
            input: Input {
                command: format!(
                    "cat >> '{path}' <<'EOF'\n\nAlso the seed data: fixtures drift.\nEOF"
                ),
                ..Input::default()
            },
            ..Call::default()
        };
        let n = mem_note_of(&call).expect("a note");
        assert!(n.shell && n.appends && n.call.kind == Kind::Write);
        assert_eq!(n.call.input.path, path);
        let peek = MemPeek {
            old: note.into(),
            had: true,
            ..MemPeek::default()
        };
        let c = MemCard {
            input: &n.call.input,
            edit: false,
            appends: n.appends,
            shell: n.shell,
            peek: &peek,
            known: true,
            focus: false,
            pick: 0,
            hover: "",
        };
        let (lines, btns) = memory_card(&c, 78, 40);
        let out = text(&lines).join("\n");
        for w in [
            "Update this memory?",
            "Run the real db in integration tests, never mocks",
            "+ Also the seed data: fixtures drift.",
            "n Skip",
        ] {
            assert!(out.contains(w), "card missing {w:?}:\n{out}");
        }
        assert!(!out.contains("Always ok") && !out.contains("<<"), "{out}");
        assert_eq!(btns.len(), 3);
        // Not a note: a heredoc elsewhere, or MEMORY.md.
        let mut other = call.clone();
        other.input.command = "cat > /tmp/x.md <<'EOF'\nhi\nEOF".into();
        assert!(mem_note_of(&other).is_none());
    }

    #[test]
    fn the_memory_list() {
        let f = |group: &str, name: &str, up: i64, missing: bool| MemoryFile {
            group: group.into(),
            path: format!("/p/{name}"),
            name: name.into(),
            up,
            missing,
            size: 2048,
            mod_time: 0,
            ..MemoryFile::default()
        };
        let files = vec![
            f("Instructions", "CLAUDE.md", 1500, false),
            f("Instructions", "AGENTS.md", 0, true),
            f("Settings", "settings.json", 0, false),
        ];
        let when: BTreeMap<String, String> =
            [("Instructions".to_string(), "every session".to_string())].into();
        let rows = mem_rows(&files, &when, "mem:/p/CLAUDE.md", 80, 120_000, true, "x");
        let t: Vec<String> = rows
            .iter()
            .map(|r| text_of(&r.line).trim_end().to_string())
            .collect();
        assert_eq!(
            t[0].split_whitespace().collect::<Vec<_>>(),
            ["Instructions", "every", "session"]
        );
        assert!(
            t[1].starts_with("▍   ◆ CLAUDE.md") && t[1].ends_with("≈1.5k · 2m ago"),
            "{}",
            t[1]
        );
        assert!(t[2].starts_with("    ◇ AGENTS.md") && t[2].ends_with("not written yet"));
        assert!(t[4].ends_with("2.0 KB · 2m ago"), "{}", t[4]);
        assert_eq!(rows[1].r#ref, "mem:/p/CLAUDE.md");
        assert_eq!(rows[1].line.bg, SEL_BG);
        let none = mem_rows(&[], &when, "", 80, 0, true, mem_nothing(false));
        assert_eq!(text_of(&none[0].line).trim_end(), "    Reading…");
        let head = memory_head("Claude Code", &files, &[], 0, false, 120);
        assert!(
            text_of(&head[0])
                .starts_with("  Memory   what Claude Code reads for this project, and when")
        );
        assert!(
            text_of(&head[0])
                .trim_end()
                .ends_with("≈1.5k tokens every session · 3 files")
        );
    }

    /// A memory's notes go under their kind, each by what it says and how
    /// often it was opened; the head says how many are worth a look
    /// (TestMemoryView's view half).
    #[test]
    fn notes_by_kind() {
        let note = |name: &str, kind: &str, about: &str, warn: &str| MemoryFile {
            group: "Auto memory".into(),
            path: format!("/m/{name}.md"),
            name: name.into(),
            kind: kind.into(),
            about: about.into(),
            warn: warn.into(),
            mod_time: 60_000,
            ..MemoryFile::default()
        };
        let mut index = note("MEMORY", "", "", "");
        index.name = "MEMORY.md".into();
        let mut opened = note("work-on-main", "feedback", "no worktrees or branches", "");
        opened.reads = 3;
        let files = vec![
            index,
            opened,
            note(
                "stray",
                "",
                "never indexed",
                "not in MEMORY.md · found only if recall picks it",
            ),
        ];
        let rows = mem_rows(&files, &BTreeMap::new(), "", 100, 120_000, true, "x");
        let t: String = rows
            .iter()
            .map(|r| text_of(&r.line).trim_end().to_string() + "\n")
            .collect();
        for want in [
            "How to work  corrections and approaches to keep",
            "● no worktrees or branches",
            "opened 3× · 1m ago",
            "No type",
            "never indexed  ! not in MEMORY.md",
        ] {
            assert!(t.contains(want), "rows lack {want:?}:\n{t}");
        }
        assert_eq!(rows.last().unwrap().r#ref, "mem:/m/stray.md");
        let head = memory_head("Claude Code", &files, &[], 1, true, 120);
        let l = text_of(&head[2]);
        assert!(l.starts_with("  memory notes worth a look"), "{l}");
        assert!(
            l.contains("1 to review · pick a note: r reviews · f tidies the index"),
            "{l}"
        );
        assert_eq!(
            memory_head("Claude Code", &files, &[], 0, true, 120).len(),
            2
        );
    }

    /// The memory folder gives way from its left, keeping as many cells
    /// as there's room for after the ellipsis, as Go's TruncateLeft.
    #[test]
    fn folder_cut_from_the_left() {
        assert_eq!(truncate_left("/a/bcdef", 4), "…cdef");
        assert_eq!(truncate_left("/a/b", 4), "/a/b");
    }
}
