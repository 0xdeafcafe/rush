//! The file editor: one file edited in place, markdown and JSON coloured
//! as you type, their problems found as you go, and each with the keys
//! that suit it (lists carry on in markdown, brackets pair up in JSON).
//! Long lines wrap; nothing scrolls sideways. After Go's
//! internal/ui/docedit.go (docEditor :20, docKindFor :64, openDoc :91,
//! readDoc :113, load :162, detectIndent :201, refresh :225, toWrite
//! :243, writeDoc :257, change :341, undoStep :357, insertText :369,
//! layout :413, wrapRow :435, rowOf :460, atCol :486, key :509, newline
//! :629, nextBullet :667, indentLines :678, moveLine :735, toggleTick
//! :758, wrapSel :781, softBackspace :794, jsonPair :819, inString :847,
//! view :868, cursorAt :992); its colours and problems are style.rs.

pub mod style;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rush_core::text::{Color, Line, Mods, Span, Style};

use super::file_size;
use crate::frame::Press;
use crate::frame::editor::{cut, edit_sel, insert, line_end, line_start};
use crate::style::*;
pub use style::{Diag, Links, MD_ITEM, format_json, json_problems, md_problems};

/// The selection's ground, and a problem's (style.go:86 edSelBG,
/// edErrBG).
pub const SEL_GROUND: Color = Color::Rgb(72, 62, 52);
pub const ERR_GROUND: Color = Color::Rgb(96, 42, 38);

/// The most the editor takes in; bigger files open in $EDITOR
/// (maxDocSize).
pub const MAX_DOC_SIZE: u64 = 1 << 20;

/// What kind of file it is, for its colours and keys (docKind).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Text,
    Markdown,
    Json,
}

/// The kind a file's name says (docKindFor).
pub fn kind_for(path: &str) -> Kind {
    match Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("md" | "markdown") => Kind::Markdown,
        Some("json") => Kind::Json,
        _ => Kind::Text,
    }
}

/// The file as a read in the background found it (docRead).
#[derive(Clone, Debug, Default)]
pub struct Read {
    /// Unchanged since it was last read: nothing was read.
    pub same: bool,
    pub missing: bool,
    pub modified: Option<SystemTime>,
    /// Its size, when it's too big to edit here.
    pub big: u64,
    pub err: String,
    pub text: String,
    pub crlf: bool,
    /// The .md files beside a markdown file, for [[links]].
    pub siblings: Option<HashSet<String>>,
}

/// Reads the file as it is on disk now; check reads it only when it
/// changed since modified, and takes a file it can't find as unchanged
/// (readDoc).
pub fn read_doc(path: &str, kind: Kind, modified: Option<SystemTime>, check: bool) -> Read {
    let st = std::fs::metadata(path);
    match &st {
        Err(_) if check => {
            return Read {
                same: true,
                ..Read::default()
            };
        }
        Ok(m) if check && m.modified().ok() == modified => {
            return Read {
                same: true,
                ..Read::default()
            };
        }
        Err(_) => {
            return Read {
                missing: true,
                ..Read::default()
            };
        }
        _ => {}
    }
    let st = st.expect("there");
    let mut r = Read {
        modified: st.modified().ok(),
        ..Read::default()
    };
    if st.len() > MAX_DOC_SIZE {
        r.big = st.len();
        return r;
    }
    match std::fs::read(path) {
        Err(e) => {
            r.err = e.to_string();
            return r;
        }
        Ok(b) => r.text = String::from_utf8_lossy(&b).into_owned(),
    }
    r.crlf = r.text.contains("\r\n");
    if r.crlf {
        r.text = r.text.replace("\r\n", "\n");
    }
    if kind == Kind::Markdown {
        let mut sib = HashSet::new();
        if let Some(dir) = Path::new(path).parent()
            && let Ok(rd) = std::fs::read_dir(dir)
        {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if let Some(stem) = n.strip_suffix(".md") {
                    sib.insert(stem.to_string());
                }
            }
        }
        r.siblings = Some(sib);
    }
    r
}

/// The file's indent step: the first indented line's leading tab, or its
/// spaces (two when they're odd or none are found) (detectIndent).
pub fn detect_indent(text: &str) -> String {
    for l in text.split('\n') {
        if l.starts_with('\t') {
            return "\t".into();
        }
        if l.starts_with("    ") && !l.trim_start_matches(' ').starts_with('-') {
            let n = l.len() - l.trim_start_matches(' ').len();
            return if n == 4 { "    ".into() } else { "  ".into() };
        }
        if l.starts_with("  ") {
            return "  ".into();
        }
    }
    "  ".into()
}

/// What a save writes: text as edited, out as it goes to disk (docWrite).
#[derive(Clone, Debug)]
pub struct Write {
    pub path: String,
    pub text: String,
    pub out: String,
}

/// Writes the file, through a symlink to where it points, keeping its
/// permissions; a new file's folder is made (writeDoc). The file's time
/// after it.
pub fn write_doc(w: &Write) -> Result<Option<SystemTime>, String> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::fs::canonicalize(&w.path).unwrap_or_else(|_| PathBuf::from(&w.path));
    let mode = std::fs::metadata(&path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o644);
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{base}.rush-{}", std::process::id()));
    let r = (|| {
        std::fs::write(&tmp, &w.out)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(std::fs::metadata(&path).and_then(|m| m.modified()).ok())
}

/// A read out in the background, taken once it's in (offui.go:35
/// offRead).
#[derive(Default)]
pub struct OffRead<T>(Arc<Mutex<(bool, Option<T>)>>);

impl<T: Send + 'static> OffRead<T> {
    /// Runs f in the background unless a read is out (start); at once
    /// when now.
    pub fn start(&self, now: bool, f: impl FnOnce() -> T + Send + 'static) -> bool {
        {
            let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 {
                return false;
            }
            g.0 = true;
        }
        let me = self.0.clone();
        let run = move || {
            let v = f();
            me.lock().unwrap_or_else(|e| e.into_inner()).1 = Some(v);
        };
        if now {
            run();
        } else if std::thread::Builder::new()
            .name("rush-doc-read".into())
            .spawn(run)
            .is_err()
        {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).0 = false;
            return false;
        }
        true
    }

    /// The value read, once it's done; after it a new read can start.
    pub fn take(&self) -> Option<T> {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let v = g.1.take()?;
        g.0 = false;
        Some(v)
    }

    /// Whether a read is out and not yet taken.
    pub fn out(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).0
    }
}

/// One row of the laid out file: a span of the buffer (docRow).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    /// Buffer offsets; to is past the row's last char.
    pub from: usize,
    pub to: usize,
    /// The line's last row, which owns the position at its end.
    pub last: bool,
}

/// The file cut into rows at a width (docLayout).
#[derive(Clone, Debug, Default)]
pub struct Layout {
    ver: Option<u64>,
    w: usize,
    pub rows: Vec<Row>,
    /// The first row of each line.
    pub lines: Vec<usize>,
}

impl Layout {
    /// The row the position sits in (rowOf).
    pub fn row_of(&self, pos: usize) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let (mut lo, mut hi) = (0, self.rows.len() - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.rows[mid].from <= pos {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        // A wrapped row's end is the next row's start.
        while lo > 0 && pos < self.rows[lo].from {
            lo -= 1;
        }
        lo
    }
}

/// A char's cells in the editor: a tab four, a control one (drawn as ·)
/// (docRuneW).
pub fn rune_w(r: char) -> usize {
    if r == '\t' {
        4
    } else if (r as u32) < 0x20 || r == '\x7f' || (r as u32) < 0x300 {
        1
    } else {
        rush_core::text::width(r.encode_utf8(&mut [0; 4]))
    }
}

/// Cuts buf[from..to], line n, into rows of at most w cells, breaking
/// after a space where there is one (wrapRow).
fn wrap_row(rows: &mut Vec<Row>, buf: &[char], n: usize, mut from: usize, to: usize, w: usize) {
    loop {
        let (mut width, mut brk, mut i) = (0, None, from);
        while i < to {
            let cw = rune_w(buf[i]);
            if width + cw > w && i > from {
                break;
            }
            width += cw;
            if buf[i] == ' ' {
                brk = Some(i + 1);
            }
            i += 1;
        }
        if i >= to {
            rows.push(Row {
                line: n,
                from,
                to,
                last: true,
            });
            return;
        }
        let brk = match brk {
            Some(b) if b > from && b <= i => b,
            _ => i,
        };
        rows.push(Row {
            line: n,
            from,
            to: brk,
            last: false,
        });
        from = brk;
    }
}

/// The buffer as it was, for undo (docSnap).
#[derive(Clone, Debug, Default)]
struct Snap {
    buf: Vec<char>,
    pos: usize,
}

/// One file in the editor (docEditor).
pub struct DocEditor {
    pub path: String,
    pub kind: Kind,
    pub buf: Vec<char>,
    pub pos: usize,
    /// Where a selection started.
    pub anchor: Option<usize>,
    /// The column ↑↓ aim for, once they're used.
    goal: Option<usize>,
    /// The first row shown.
    pub top: usize,
    pub saved: String,
    pub crlf: bool,
    pub missing: bool,
    /// Why it can't be edited here; "" when it can.
    pub read_only: String,
    modified: Option<SystemTime>,
    checked: Instant,
    /// It changed on disk while you had changes of your own.
    pub stale: bool,
    pub indent: String,

    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// The last change was typing, which undo takes back with the typing
    /// before it.
    typing: bool,
    /// Bumps on every change.
    pub ver: u64,

    lay: Layout,
    /// The text's width and height when last drawn.
    tw: usize,
    th: usize,
    nlines: usize,
    nl_ver: Option<u64>,
    diag_ver: Option<u64>,
    diags: Vec<Diag>,
    /// The .md files beside it, for [[links]].
    pub siblings: Option<HashSet<String>>,

    /// The first read is still out: there's nothing to edit yet.
    pub loading: bool,
    /// A save is out.
    pub saving: bool,
    reads: OffRead<Read>,
    pub links: Links,
    link_ver: u64,
    /// Reads run at once on the calling thread (tests).
    at_once: bool,
}

impl DocEditor {
    fn blank(path: &str, at_once: bool) -> DocEditor {
        DocEditor {
            path: path.to_string(),
            kind: kind_for(path),
            buf: Vec::new(),
            pos: 0,
            anchor: None,
            goal: None,
            top: 0,
            saved: String::new(),
            crlf: false,
            missing: false,
            read_only: "still reading it".into(),
            modified: None,
            checked: Instant::now(),
            stale: false,
            indent: "  ".into(),
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            ver: 0,
            lay: Layout::default(),
            tw: 0,
            th: 0,
            nlines: 0,
            nl_ver: None,
            diag_ver: None,
            diags: Vec::new(),
            siblings: None,
            loading: true,
            saving: false,
            reads: OffRead::default(),
            links: Links::default(),
            link_ver: 0,
            at_once,
        }
    }

    /// Opens path; the file is read in the background, and until it's in
    /// there's nothing to edit (openDoc).
    pub fn open(path: &str) -> DocEditor {
        let mut e = DocEditor::blank(path, false);
        e.start_read(false);
        e.poll();
        e
    }

    /// Opens path, reading it and its links on this thread: for tests
    /// and one-off reads.
    pub fn open_now(path: &str) -> DocEditor {
        let mut e = DocEditor::blank(path, true);
        e.links.at_once();
        e.start_read(false);
        e.poll();
        e
    }

    fn start_read(&mut self, check: bool) {
        self.checked = Instant::now();
        let (path, kind, modified) = (self.path.clone(), self.kind, self.modified);
        self.reads
            .start(self.at_once, move || read_doc(&path, kind, modified, check));
    }

    /// Takes in a read that's done (poll).
    pub fn poll(&mut self) {
        if let Some(r) = self.reads.take() {
            self.load(r);
        }
    }

    /// Whether a read is out: the caller asks to be woken to poll.
    pub fn reading(&self) -> bool {
        self.reads.out()
    }

    /// Takes in the file as read: at once when it's opened, and after that
    /// unless you have changes of your own, which it marks stale instead
    /// (load).
    pub fn load(&mut self, r: Read) {
        let first = self.loading;
        self.loading = false;
        if r.same {
            return;
        }
        if !first && self.dirty() {
            self.stale = true;
            return;
        }
        self.read_only.clear();
        if r.missing {
            self.missing = true;
            self.buf.clear();
            self.saved.clear();
            self.modified = None;
            self.pos = 0;
            self.bump();
            return;
        }
        self.missing = false;
        self.modified = r.modified;
        if r.big > 0 {
            self.read_only = format!(
                "{} is too big to edit here · ctrl+g opens it in $EDITOR",
                file_size(r.big as i64)
            );
            return;
        }
        if !r.err.is_empty() {
            self.read_only = r.err;
            return;
        }
        self.crlf = r.crlf;
        self.buf = r.text.chars().collect();
        self.saved = r.text;
        self.pos = self.pos.min(self.buf.len());
        self.anchor = None;
        self.indent = detect_indent(&self.saved);
        if r.siblings.is_some() {
            self.siblings = r.siblings;
        }
        self.bump();
    }

    fn bump(&mut self) {
        self.ver += 1;
    }

    /// Changes not saved.
    pub fn dirty(&self) -> bool {
        self.read_only.is_empty() && self.buf.iter().collect::<String>() != self.saved
    }

    /// The text as it stands.
    pub fn text(&self) -> String {
        self.buf.iter().collect()
    }

    /// Picks up a change made on disk (by Claude, or $EDITOR): looked at
    /// in the background at most every 700ms, read again unless you have
    /// changes of your own, which it marks stale instead (refresh).
    pub fn refresh(&mut self) {
        self.poll();
        if self.loading || self.saving || self.checked.elapsed() < Duration::from_millis(700) {
            return;
        }
        self.start_read(true);
    }

    /// What saving would write now, or why it can't (toWrite).
    pub fn to_write(&self) -> Result<Write, String> {
        if !self.read_only.is_empty() {
            return Err(self.read_only.clone());
        }
        let text = self.text();
        let out = if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text.clone()
        };
        Ok(Write {
            path: self.path.clone(),
            text,
            out,
        })
    }

    /// Takes in a save that went through (wrote).
    pub fn wrote(&mut self, w: Write, modified: Option<SystemTime>) {
        self.saved = w.text;
        self.missing = false;
        self.stale = false;
        if modified.is_some() {
            self.modified = modified;
        }
    }

    /// Saves there and then: off the UI thread only (save). The editor on
    /// screen saves through Model::later with to_write, write_doc and
    /// wrote.
    pub fn save(&mut self) -> Result<(), String> {
        let w = self.to_write()?;
        let m = write_doc(&w)?;
        self.wrote(w, m);
        Ok(())
    }

    // --- changes and undo ---

    /// Replaces the buffer, keeping what was there for undo; typing that
    /// follows typing joins its undo step (change).
    pub fn change(&mut self, buf: Vec<char>, pos: usize, typing: bool) {
        if !self.read_only.is_empty() {
            return;
        }
        if !(typing && self.typing) {
            self.undo.push(Snap {
                buf: std::mem::take(&mut self.buf),
                pos: self.pos,
            });
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
        self.typing = typing;
        self.redo.clear();
        self.buf = buf;
        self.pos = pos;
        self.anchor = None;
        self.goal = None;
        self.bump();
    }

    fn undo_step(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(s) = from.pop() else { return };
        to.push(Snap {
            buf: std::mem::take(&mut self.buf),
            pos: self.pos,
        });
        self.buf = s.buf;
        self.pos = s.pos;
        self.anchor = None;
        self.goal = None;
        self.typing = false;
        self.bump();
    }

    /// Puts text (a paste) at the cursor, over any selection (insertText).
    pub fn insert_text(&mut self, t: &str) {
        let (mut buf, mut pos) = (self.buf.clone(), self.pos);
        if let Some((from, to)) = self.selection() {
            buf = cut(&buf, from, to);
            pos = from;
        }
        let r: Vec<char> = t.chars().collect();
        let n = r.len();
        self.change(insert(&buf, pos, &r), pos + n, false);
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.anchor
            .filter(|&a| a != self.pos)
            .map(|a| (a.min(self.pos), a.max(self.pos)))
    }

    // --- layout ---

    /// The file laid out w wide (layout).
    pub fn layout(&mut self, w: usize) -> &Layout {
        let w = w.max(4);
        if self.lay.ver == Some(self.ver) && self.lay.w == w {
            return &self.lay;
        }
        let l = &mut self.lay;
        l.ver = Some(self.ver);
        l.w = w;
        l.rows.clear();
        l.lines.clear();
        let (mut start, mut n) = (0, 0);
        for i in 0..=self.buf.len() {
            if i < self.buf.len() && self.buf[i] != '\n' {
                continue;
            }
            l.lines.push(l.rows.len());
            wrap_row(&mut l.rows, &self.buf, n, start, i, w);
            start = i + 1;
            n += 1;
        }
        &self.lay
    }

    fn col_of(&self, r: Row, pos: usize) -> usize {
        (r.from..pos.min(r.to)).map(|i| rune_w(self.buf[i])).sum()
    }

    /// The position in row r nearest column c (atCol).
    fn at_col(&self, r: Row, c: usize) -> usize {
        let mut end = r.to;
        if !r.last && end > r.from {
            end -= 1; // past a wrapped row's end is the next row
        }
        let mut w = 0;
        for i in r.from..end {
            let cw = rune_w(self.buf[i]);
            if w + cw > c {
                return i;
            }
            w += cw;
        }
        end
    }

    fn line_range(&self, pos: usize) -> (usize, usize) {
        (line_start(&self.buf, pos), line_end(&self.buf, pos))
    }

    // --- keys ---

    /// Applies one key (key). act names what the caller must do: "save",
    /// "format"; copied is text for the clipboard; used, whether the key
    /// was the editor's.
    pub fn key(&mut self, k: &Press) -> (&'static str, String, bool) {
        let s = k.s.as_str();
        let h = self.th;
        let tw = self.tw;
        self.layout(tw);
        match s {
            "up" | "shift+up" => {
                self.vert(-1, s != "up");
                return ("", String::new(), true);
            }
            "down" | "shift+down" => {
                self.vert(1, s != "down");
                return ("", String::new(), true);
            }
            "pgup" | "shift+pgup" => {
                self.vert(-(h.saturating_sub(2).max(1) as isize), s != "pgup");
                return ("", String::new(), true);
            }
            "pgdown" | "shift+pgdown" => {
                self.vert(h.saturating_sub(2).max(1) as isize, s != "pgdown");
                return ("", String::new(), true);
            }
            "ctrl+home" | "super+up" => {
                self.move_to(0, false);
                return ("", String::new(), true);
            }
            "ctrl+end" | "super+down" => {
                self.move_to(self.buf.len(), false);
                return ("", String::new(), true);
            }
            "ctrl+s" | "super+s" => return ("save", String::new(), true),
            "ctrl+z" | "super+z" | "ctrl+_" | "ctrl+/" => {
                self.undo_step(true);
                return ("", String::new(), true);
            }
            "ctrl+y" | "ctrl+shift+z" | "super+shift+z" => {
                self.undo_step(false);
                return ("", String::new(), true);
            }
            _ => {}
        }
        if !self.read_only.is_empty() {
            // Moving and copying still work; nothing changes it.
            if matches!(
                s,
                "left"
                    | "right"
                    | "home"
                    | "end"
                    | "ctrl+a"
                    | "ctrl+e"
                    | "ctrl+left"
                    | "ctrl+right"
                    | "alt+left"
                    | "alt+right"
                    | "shift+left"
                    | "shift+right"
                    | "shift+home"
                    | "shift+end"
                    | "super+a"
                    | "ctrl+c"
                    | "alt+c"
            ) && let Some(e) = edit_sel(&self.buf, self.pos, self.anchor, k)
            {
                self.pos = e.pos;
                self.anchor = e.anchor;
                return ("", e.copied, true);
            }
            return ("", String::new(), true);
        }
        match s {
            "enter" => {
                self.newline();
                return ("", String::new(), true);
            }
            "tab" => {
                self.indent_lines(false);
                return ("", String::new(), true);
            }
            "shift+tab" => {
                self.indent_lines(true);
                return ("", String::new(), true);
            }
            "alt+up" | "alt+down" => {
                self.move_line(s == "alt+up");
                return ("", String::new(), true);
            }
            "ctrl+t" => {
                if self.kind == Kind::Json {
                    return ("format", String::new(), true);
                }
                if self.kind == Kind::Markdown {
                    self.toggle_tick();
                }
                return ("", String::new(), true);
            }
            "ctrl+b" => {
                if self.kind == Kind::Markdown {
                    self.wrap_sel("**");
                }
                return ("", String::new(), true);
            }
            "backspace" | "ctrl+h" => {
                if self.selection().is_none() && self.soft_backspace() {
                    return ("", String::new(), true);
                }
            }
            _ => {}
        }
        if self.kind == Kind::Json
            && let Some(c) = k.text()
            && self.selection().is_none()
            && self.json_pair(c)
        {
            return ("", String::new(), true);
        }
        let Some(e) = edit_sel(&self.buf, self.pos, self.anchor, k) else {
            return ("", String::new(), s == "ctrl+c"); // nothing picked to copy; it never quits from here
        };
        if e.buf != self.buf {
            let typed = k.text().is_some_and(|c| c != ' ' && c != '\n');
            self.change(e.buf, e.pos, typed);
            return ("", e.copied, true);
        }
        self.pos = e.pos;
        self.anchor = e.anchor;
        self.goal = None;
        self.typing = false;
        ("", e.copied, true)
    }

    fn move_to(&mut self, pos: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.pos);
            }
        } else {
            self.anchor = None;
        }
        self.pos = pos.min(self.buf.len());
        self.typing = false;
    }

    fn vert(&mut self, d: isize, extend: bool) {
        let tw = self.tw;
        self.layout(tw);
        let r = self.lay.row_of(self.pos);
        let row = self.lay.rows[r];
        let g = match self.goal {
            Some(g) => g,
            None => self.col_of(row, self.pos),
        };
        let t = (r as isize + d).clamp(0, self.lay.rows.len() as isize - 1) as usize;
        if t == r && d < 0 {
            self.move_to(0, extend);
        } else if t == r && d > 0 {
            self.move_to(self.buf.len(), extend);
        } else {
            let p = self.at_col(self.lay.rows[t], g);
            self.move_to(p, extend);
        }
        self.goal = Some(g);
    }

    /// Breaks the line, keeping its indent; in markdown a list goes on
    /// (and an empty item ends it), and in JSON an opened bracket indents
    /// (newline).
    fn newline(&mut self) {
        let (mut buf, mut pos) = (self.buf.clone(), self.pos);
        if let Some((from, to)) = self.selection() {
            buf = cut(&buf, from, to);
            pos = from;
        }
        let ls = line_start(&buf, pos);
        let line: String = buf[ls..pos].iter().collect();
        let lead: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let mut ins = format!("\n{lead}");
        let mut after = String::new();
        match self.kind {
            Kind::Markdown => {
                if let Some(m) = MD_ITEM.captures(&line) {
                    let whole = m.get(0).expect("item").as_str();
                    if line[whole.len()..].trim().is_empty() && pos == line_end(&buf, pos) {
                        // An empty item: enter ends the list.
                        let b = cut(&buf, ls, pos);
                        self.change(b, ls, false);
                        return;
                    }
                    ins += &next_bullet(&m[2]);
                    if m.get(3).is_some() {
                        ins += "[ ] ";
                    }
                }
            }
            Kind::Json => {
                let before = line.trim_end_matches(' ');
                if before.ends_with('{') || before.ends_with('[') {
                    ins += &self.indent;
                    if pos < buf.len() && (buf[pos] == '}' || buf[pos] == ']') {
                        after = format!("\n{lead}");
                    }
                }
            }
            Kind::Text => {}
        }
        let n = ins.chars().count();
        let r: Vec<char> = format!("{ins}{after}").chars().collect();
        self.change(insert(&buf, pos, &r), pos + n, false);
    }

    /// Indents (or outdents) the lines of the selection or the cursor's
    /// line; tab with no selection off a list item types an indent
    /// (indentLines).
    fn indent_lines(&mut self, out: bool) {
        let sel = self.selection();
        let (from, to) = sel.unwrap_or((self.pos, self.pos));
        if sel.is_none() {
            let ls = line_start(&self.buf, self.pos);
            let line: String = self.buf[ls..line_end(&self.buf, self.pos)].iter().collect();
            if !out && !(self.kind == Kind::Markdown && MD_ITEM.is_match(&line)) {
                let r: Vec<char> = self.indent.chars().collect();
                let p = self.pos + r.len();
                self.change(insert(&self.buf, self.pos, &r), p, false);
                return;
            }
        }
        let mut buf = self.buf.clone();
        let (mut pos, mut anchor) = (self.pos, self.anchor);
        let step: Vec<char> = self.indent.chars().collect();
        let mut starts = Vec::new();
        let mut i = line_start(&buf, from);
        loop {
            starts.push(i);
            let j = line_end(&buf, i);
            if j >= to || j >= buf.len() {
                break;
            }
            i = j + 1;
        }
        for &i in starts.iter().rev() {
            let shift: isize = if out {
                let mut n = 0;
                while n < step.len()
                    && i + n < buf.len()
                    && (buf[i + n] == ' ' || buf[i + n] == '\t')
                {
                    n += 1;
                    if buf[i + n - 1] == '\t' {
                        break;
                    }
                }
                buf = cut(&buf, i, i + n);
                -(n as isize)
            } else {
                buf = insert(&buf, i, &step);
                step.len() as isize
            };
            let adj = |p: usize| {
                if p >= i {
                    (p as isize + shift).max(i as isize) as usize
                } else {
                    p
                }
            };
            pos = adj(pos);
            anchor = anchor.map(adj);
        }
        self.change(buf, pos, false);
        if sel.is_some() {
            self.anchor = anchor;
        }
    }

    /// Swaps the cursor's line with the one above or below (moveLine).
    fn move_line(&mut self, up: bool) {
        let (ls, le) = self.line_range(self.pos);
        let col = self.pos - ls;
        if up {
            if ls == 0 {
                return;
            }
            let ps = line_start(&self.buf, ls - 1);
            let mut b = self.buf[..ps].to_vec();
            b.extend_from_slice(&self.buf[ls..le]);
            b.push('\n');
            b.extend_from_slice(&self.buf[ps..ls - 1]);
            b.extend_from_slice(&self.buf[le..]);
            self.change(b, ps + col, false);
            return;
        }
        if le >= self.buf.len() {
            return;
        }
        let ne = line_end(&self.buf, le + 1);
        let below = ne - (le + 1);
        let mut b = self.buf[..ls].to_vec();
        b.extend_from_slice(&self.buf[le + 1..ne]);
        b.push('\n');
        b.extend_from_slice(&self.buf[ls..le]);
        b.extend_from_slice(&self.buf[ne..]);
        self.change(b, ls + below + 1 + col, false);
    }

    /// Ticks or unticks a markdown task item: "- [ ]" and "- [x]"
    /// (toggleTick).
    fn toggle_tick(&mut self) {
        let (ls, le) = self.line_range(self.pos);
        let line: String = self.buf[ls..le].iter().collect();
        let Some(m) = MD_ITEM.captures(&line) else {
            return;
        };
        let repl = match m.get(3) {
            Some(b) if b.as_str().contains(['x', 'X']) => {
                format!("{}[ ] {}", &line[..b.start()], &line[b.end()..])
            }
            Some(b) => format!("{}[x] {}", &line[..b.start()], &line[b.end()..]),
            None => {
                let e = m.get(0).expect("item").end();
                format!("{}[ ] {}", &line[..e], &line[e..])
            }
        };
        let d = repl.chars().count() as isize - line.chars().count() as isize;
        let mut b = self.buf[..ls].to_vec();
        b.extend(repl.chars());
        b.extend_from_slice(&self.buf[le..]);
        let p = (self.pos as isize + d).max(ls as isize) as usize;
        self.change(b, p, false);
    }

    /// Puts mark either side of the selection, or of an empty pair with
    /// the cursor inside (wrapSel).
    fn wrap_sel(&mut self, mark: &str) {
        let r: Vec<char> = mark.chars().collect();
        match self.selection() {
            None => {
                let both: Vec<char> = r.iter().chain(&r).copied().collect();
                let p = self.pos + r.len();
                self.change(insert(&self.buf, self.pos, &both), p, false);
            }
            Some((from, to)) => {
                let b = insert(&insert(&self.buf, to, &r), from, &r);
                self.change(b, to + 2 * r.len(), false);
            }
        }
    }

    /// Takes back an indent step in leading space, or an empty pair of
    /// JSON brackets or quotes, whole; says whether it did
    /// (softBackspace).
    fn soft_backspace(&mut self) -> bool {
        let p = self.pos;
        if p == 0 {
            return false;
        }
        if self.kind == Kind::Json && p < self.buf.len() {
            let (o, c) = (self.buf[p - 1], self.buf[p]);
            if matches!((o, c), ('{', '}') | ('[', ']') | ('"', '"')) {
                let b = cut(&self.buf, p - 1, p + 1);
                self.change(b, p - 1, false);
                return true;
            }
        }
        let ls = line_start(&self.buf, p);
        let n_in = self.indent.chars().count();
        if self.indent == "\t" || p - ls < n_in || self.buf[ls..p].iter().any(|c| *c != ' ') {
            return false;
        }
        let mut n = (p - ls) % n_in;
        if n == 0 {
            n = n_in;
        }
        let b = cut(&self.buf, p - n, p);
        self.change(b, p - n, false);
        true
    }

    /// Closes a bracket or quote as it's opened, and steps over a closing
    /// one that's already there; says whether it did either (jsonPair).
    fn json_pair(&mut self, c: char) -> bool {
        let next = self.buf.get(self.pos).copied();
        if matches!(c, '}' | ']' | '"') && next == Some(c) && (c != '"' || self.in_string()) {
            self.pos += 1;
            self.typing = false;
            return true;
        }
        let shut = match c {
            '{' => '}',
            '[' => ']',
            '"' => '"',
            _ => return false,
        };
        if c == '"' && self.in_string() {
            return false;
        }
        if let Some(n) = next
            && !" \n\t,}]:".contains(n)
        {
            return false;
        }
        let p = self.pos + 1;
        self.change(insert(&self.buf, self.pos, &[c, shut]), p, false);
        true
    }

    /// Whether the cursor is inside a JSON string on its line (inString).
    fn in_string(&self) -> bool {
        let mut inside = false;
        let mut i = line_start(&self.buf, self.pos);
        while i < self.pos {
            match self.buf[i] {
                '\\' => i += 1,
                '"' => inside = !inside,
                _ => {}
            }
            i += 1;
        }
        inside
    }

    // --- drawing ---

    /// The file in w×h cells: a line-number gutter, then its rows, the
    /// cursor shown when it has the keys (view).
    pub fn view(&mut self, w: usize, h: usize, focused: bool) -> Vec<Line> {
        if h == 0 {
            return Vec::new();
        }
        if self.nl_ver != Some(self.ver) {
            self.nlines = self.buf.iter().filter(|c| **c == '\n').count() + 1;
            self.nl_ver = Some(self.ver);
        }
        let gut = self.nlines.to_string().len() + 2;
        let tw = 4.max(w as isize - gut as isize - 1) as usize;
        self.tw = tw;
        self.th = h;
        self.layout(tw);
        let cur = self.lay.row_of(self.pos);
        if focused {
            if cur < self.top {
                self.top = cur;
            } else if cur >= self.top + h {
                self.top = cur + 1 - h;
            }
        }
        self.top = self.top.min(self.lay.rows.len().saturating_sub(h));

        let mut by_line: HashMap<usize, Diag> = HashMap::new();
        for d in self.problems().to_vec() {
            match by_line.get(&d.line) {
                Some(old) if !(old.warn && !d.warn) => {}
                _ => {
                    by_line.insert(d.line, d);
                }
            }
        }
        let rows = &self.lay.rows;
        let last = rows.len().min(self.top + h);
        let (mut first_line, mut last_line) = (0, 0);
        if self.top < rows.len() {
            first_line = rows[self.top].line;
            last_line = rows[self.top.max(last.saturating_sub(1))].line;
        }
        let rows = rows[..last].to_vec();
        let cur_line = rows.get(cur).map(|r| r.line);
        let styles = self.styles(first_line, last_line);
        let sel = self.selection();
        let mut out = Vec::with_capacity(h);
        for r in self.top..last {
            let row = rows[r];
            let mut l = Line::new();
            // The gutter: a mark on a line with a problem, and the number
            // on the line's first row.
            l.spans.push(match by_line.get(&row.line) {
                Some(d) if d.warn => sp("▍", YELLOW),
                Some(_) => sp("▍", RED),
                None => plain(" "),
            });
            if r == 0 || rows[r - 1].line != row.line {
                let n = format!("{:>w$}", row.line + 1, w = gut - 2);
                if Some(row.line) == cur_line && focused {
                    l.spans.push(sp(n, SUB));
                } else {
                    l.spans.push(sp(n, FAINT));
                }
            } else {
                l.spans.push(plain(blanks(gut - 2)));
            }
            l.spans.push(plain(" "));
            let st = styles.get(&row.line);
            let ls = line_start(&self.buf, row.from);
            let err_col = by_line.get(&row.line).filter(|d| !d.warn).map(|d| d.col);
            let mut used = 0;
            for i in row.from..row.to {
                let k = i - ls;
                let mut s = st.and_then(|st| st.get(k).copied()).unwrap_or_default();
                if Some(k) == err_col {
                    s = s.on(ERR_GROUND);
                }
                if let Some((f, t)) = sel
                    && i >= f
                    && i < t
                {
                    s = s.on(SEL_GROUND);
                }
                if focused && i == self.pos {
                    s = reversed(s);
                }
                let ch = self.buf[i];
                let text = if ch == '\t' {
                    "    ".to_string()
                } else if (ch as u32) < 0x20 || ch == '\x7f' {
                    "·".to_string()
                } else {
                    ch.to_string()
                };
                push_styled(&mut l, &text, s);
                used += rune_w(ch);
            }
            if focused && self.pos == row.to && row.last {
                l.spans.push(Span::new(
                    " ",
                    Style {
                        mods: Mods::REVERSE,
                        ..Style::default()
                    },
                ));
                used += 1;
            }
            if let Some(ec) = err_col
                && row.last
                && ec >= row.to - ls
                && used < tw
            {
                l.spans
                    .push(Span::new(" ", Style::default().on(ERR_GROUND))); // the problem is at the line's end
                used += 1;
            }
            l.spans.push(plain(blanks((tw + 1).saturating_sub(used))));
            out.push(l);
        }
        while out.len() < h {
            out.push(line([sp(" ~", FAINT)]));
        }
        out
    }

    /// Where the cursor is, as a line and column, from one (cursorAt).
    pub fn cursor_at(&self) -> (usize, usize) {
        let ls = line_start(&self.buf, self.pos);
        (
            self.buf[..ls].iter().filter(|c| **c == '\n').count() + 1,
            self.pos - ls + 1,
        )
    }
}

/// Adds text in style s to l, joining the last span when it's the same.
fn push_styled(l: &mut Line, text: &str, s: Style) {
    if let Some(last) = l.spans.last_mut()
        && last.style == s
    {
        last.text.push_str(text);
        return;
    }
    l.spans.push(Span::new(text, s));
}

/// The marker for the item after one: the same bullet, or the next
/// number (nextBullet).
pub fn next_bullet(b: &str) -> String {
    let digits: String = b.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty()
        && let Ok(n) = digits.parse::<u64>()
    {
        return format!("{}{} ", n + 1, &b[digits.len()..]);
    }
    format!("{b} ")
}
