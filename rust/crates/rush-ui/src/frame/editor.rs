//! Text editing, the same keys in every box: words with ctrl or alt and
//! the arrows or backspace, the line with cmd+backspace or ctrl+u and
//! ctrl+k, its ends with cmd+← →, the whole text's with cmd+↑ ↓, new lines
//! with shift+enter, alt+enter or ctrl+j; a selection with shift; a paste
//! folded into a chip is one thing. After Go's internal/ui/editor.go
//! (edit :22, cut :70, cleanPaste :82, wordLeft :116, editSel :149,
//! editChips :194, chipKey :216, editInput :262, pathSpans :350,
//! splitPaths :439, filePath :335, imagePath :297, isDrop :315),
//! imagerefs.go (chipRe :111, chipSpans :114, outOfChip :139),
//! stash.go (undoStack :18, isUndo :64), and a box's pastes as chips:
//! pastes.go (expand :46, hoverChip :177, openChip :185) and model.go
//! (:1435-1458 a paste into either box, :1464-1469 space on a lit chip).

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::Press;
use crate::screens::session_cards::pastes::{Pastes, is_long_paste};

/// A tagged paste's id: four hex digits, random as Go's rand.IntN(0x10000).
fn paste_tag() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.finish() as u16
}

/// One wrapped row or a chip, as char offsets [from, to).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seg {
    pub from: usize,
    pub to: usize,
}

/// Applies one editing key to buf with the cursor at pos; None when it
/// isn't an editing key (edit).
pub fn edit(buf: &[char], pos: usize, k: &Press) -> Option<(Vec<char>, usize)> {
    edit_named(buf, pos, &k.s, k.text())
}

fn edit_named(buf: &[char], pos: usize, s: &str, text: Option<char>) -> Option<(Vec<char>, usize)> {
    let pos = pos.min(buf.len());
    let same = |p: usize| Some((buf.to_vec(), p));
    match s {
        "left" => return same(pos.saturating_sub(1)),
        "right" => return same((pos + 1).min(buf.len())),
        "ctrl+left" | "alt+left" | "alt+b" => return same(word_left(buf, pos)),
        "ctrl+right" | "alt+right" | "alt+f" => return same(word_right(buf, pos)),
        "home" | "ctrl+a" | "super+left" => return same(line_start(buf, pos)),
        "end" | "ctrl+e" | "super+right" => return same(line_end(buf, pos)),
        "super+up" | "ctrl+home" => return same(0),
        "super+down" | "ctrl+end" => return same(buf.len()),
        "backspace" | "ctrl+h" | "shift+backspace" => {
            if pos == 0 {
                return same(0);
            }
            return Some((cut(buf, pos - 1, pos), pos - 1));
        }
        "delete" => {
            if pos >= buf.len() {
                return same(pos);
            }
            return Some((cut(buf, pos, pos + 1), pos));
        }
        "ctrl+w" | "alt+backspace" | "ctrl+backspace" => {
            let from = word_left(buf, pos);
            return Some((cut(buf, from, pos), from));
        }
        "alt+delete" | "ctrl+delete" | "alt+d" => {
            return Some((cut(buf, pos, word_right(buf, pos)), pos));
        }
        "ctrl+u" | "super+backspace" => {
            let mut from = line_start(buf, pos);
            if from == pos && pos > 0 {
                from = line_start(buf, pos - 1); // at a line's start, join and clear the one above
            }
            return Some((cut(buf, from, pos), from));
        }
        "ctrl+k" | "super+delete" => return Some((cut(buf, pos, line_end(buf, pos)), pos)),
        "shift+enter" | "alt+enter" | "ctrl+j" => {
            return Some((insert(buf, pos, &['\n']), pos + 1));
        }
        _ => {}
    }
    let c = text?;
    Some((insert(buf, pos, &[c]), pos + 1))
}

pub fn cut(buf: &[char], from: usize, to: usize) -> Vec<char> {
    if from >= to {
        return buf.to_vec();
    }
    let mut out = Vec::with_capacity(buf.len() - (to - from));
    out.extend_from_slice(&buf[..from]);
    out.extend_from_slice(&buf[to..]);
    out
}

pub fn insert(buf: &[char], pos: usize, r: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(buf.len() + r.len());
    out.extend_from_slice(&buf[..pos]);
    out.extend_from_slice(r);
    out.extend_from_slice(&buf[pos..]);
    out
}

/// Pasted text made safe to draw: tabs to spaces, CRLF and a lone CR to
/// LF, other control characters dropped (cleanPaste).
pub fn clean_paste(s: &str) -> String {
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(s.len());
    let mut col = 0;
    for r in s.chars() {
        match r {
            '\t' => {
                let n = 4 - col % 4;
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            '\n' => {
                out.push(r);
                col = 0;
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

fn is_word(r: char) -> bool {
    r.is_alphanumeric() || r == '_'
}

/// Back over any gap, then over the word before it.
pub fn word_left(buf: &[char], mut pos: usize) -> usize {
    while pos > 0 && !is_word(buf[pos - 1]) {
        pos -= 1;
    }
    while pos > 0 && is_word(buf[pos - 1]) {
        pos -= 1;
    }
    pos
}

pub fn word_right(buf: &[char], mut pos: usize) -> usize {
    while pos < buf.len() && !is_word(buf[pos]) {
        pos += 1;
    }
    while pos < buf.len() && is_word(buf[pos]) {
        pos += 1;
    }
    pos
}

pub fn line_start(buf: &[char], mut pos: usize) -> usize {
    while pos > 0 && buf[pos - 1] != '\n' {
        pos -= 1;
    }
    pos
}

pub fn line_end(buf: &[char], mut pos: usize) -> usize {
    while pos < buf.len() && buf[pos] != '\n' {
        pos += 1;
    }
    pos
}

/// What an editing key did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edited {
    pub buf: Vec<char>,
    pub pos: usize,
    pub anchor: Option<usize>,
    /// Text ctrl+c (a selection) or alt+c (everything) copied.
    pub copied: String,
}

/// edit with a selection from anchor: shift with the moves extends it,
/// typing replaces it, backspace and delete remove it (editSel).
pub fn edit_sel(buf: &[char], pos: usize, anchor: Option<usize>, k: &Press) -> Option<Edited> {
    let pos = pos.min(buf.len());
    let anchor = anchor.filter(|&a| a <= buf.len());
    let done = |buf: Vec<char>, pos, anchor, copied: String| {
        Some(Edited {
            buf,
            pos,
            anchor,
            copied,
        })
    };
    match k.s.as_str() {
        "super+a" => return done(buf.to_vec(), buf.len(), Some(0), String::new()),
        "alt+c" => return done(buf.to_vec(), pos, anchor, buf.iter().collect()),
        _ => {}
    }
    let s = k.s.as_str();
    if s.starts_with("shift+") || s.starts_with("ctrl+shift+") || s.starts_with("alt+shift+") {
        let mv = s.replace("shift+", "");
        if matches!(
            mv.as_str(),
            "left"
                | "right"
                | "home"
                | "end"
                | "ctrl+left"
                | "ctrl+right"
                | "alt+left"
                | "alt+right"
                | "super+left"
                | "super+right"
                | "super+up"
                | "super+down"
        ) {
            let a = anchor.unwrap_or(pos);
            let (_, np) = edit_named(buf, pos, &mv, None)?;
            return done(buf.to_vec(), np, Some(a), String::new());
        }
    }
    if let Some(a) = anchor.filter(|&a| a != pos) {
        let (from, to) = (a.min(pos), a.max(pos));
        match s {
            "ctrl+c" | "super+c" => {
                return done(buf.to_vec(), pos, None, buf[from..to].iter().collect());
            }
            "backspace" | "delete" | "ctrl+h" => {
                return done(cut(buf, from, to), from, None, String::new());
            }
            "esc" => return done(buf.to_vec(), pos, None, String::new()),
            _ => {
                if let Some(c) = k.text() {
                    return done(
                        insert(&cut(buf, from, to), from, &[c]),
                        from + 1,
                        None,
                        String::new(),
                    );
                }
            }
        }
    }
    let (nb, np) = edit(buf, pos, k)?;
    done(nb, np, None, String::new())
}

/// A chip in a box: a paste folded ("[Pasted text #1 +3 lines]") or an
/// image ("[Image #2]") (chipRe).
static CHIP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\[(?:[Pp]asted text )?#(\d+) (?:· )?(?:\+\d+ lines|\d+ lines?(?:: [^\]\n]*)?)\]|\[Image #\d+\]",
    )
    .unwrap()
});

/// Where buf's chips are, as char offsets (chipSpans).
pub fn chip_spans(buf: &[char]) -> Vec<Seg> {
    if !buf.contains(&'[') {
        return Vec::new();
    }
    let s: String = buf.iter().collect();
    CHIP.find_iter(&s)
        .map(|m| {
            let from = s[..m.start()].chars().count();
            Seg {
                from,
                to: from + m.as_str().chars().count(),
            }
        })
        .collect()
}

/// p moved out of a chip it's inside: to its end going forward, else its
/// start (outOfChip).
pub fn out_of_chip(spans: &[Seg], p: usize, forward: bool) -> usize {
    for c in spans {
        if p > c.from && p < c.to {
            return if forward { c.to } else { c.from };
        }
    }
    p
}

/// edit_sel for a message box, where a chip is one thing: an arrow onto
/// one selects it whole, the next goes past it; backspace or delete
/// beside one takes all of it; no move or word delete stops inside one
/// (editChips).
pub fn edit_chips(buf: &[char], pos: usize, anchor: Option<usize>, k: &Press) -> Option<Edited> {
    let spans = chip_spans(buf);
    if spans.is_empty() {
        return edit_sel(buf, pos, anchor, k);
    }
    let pos = pos.min(buf.len());
    let anchor = anchor.filter(|&a| a <= buf.len());
    if let Some(e) = chip_key(&spans, buf, pos, anchor, &k.s) {
        return Some(e);
    }
    let mut e = edit_sel(buf, pos, anchor, k)?;
    if e.buf.len() == buf.len() && e.pos != pos {
        e.pos = out_of_chip(&spans, e.pos, e.pos > pos);
    }
    Some(e)
}

fn chip_key(
    spans: &[Seg],
    buf: &[char],
    pos: usize,
    anchor: Option<usize>,
    s: &str,
) -> Option<Edited> {
    let picked = anchor.filter(|&a| a != pos);
    let ed = |buf: Vec<char>, pos, anchor| {
        Some(Edited {
            buf,
            pos,
            anchor,
            copied: String::new(),
        })
    };
    for c in spans {
        let on = picked.is_some_and(|a| a.min(pos) == c.from && a.max(pos) == c.to);
        if on && s == "right" {
            return ed(buf.to_vec(), c.to, None);
        }
        if on && s == "left" {
            return ed(buf.to_vec(), c.from, None);
        }
        if picked.is_some() {
            continue;
        }
        if s == "right" && pos == c.from {
            return ed(buf.to_vec(), c.to, Some(c.from));
        }
        if s == "left" && pos == c.to {
            return ed(buf.to_vec(), c.from, Some(c.to));
        }
        if (matches!(s, "backspace" | "ctrl+h" | "shift+backspace") && pos == c.to)
            || (s == "delete" && pos == c.from)
        {
            return ed(cut(buf, c.from, c.to), c.from, None);
        }
    }
    if picked.is_some() {
        return None;
    }
    match s {
        "ctrl+w" | "alt+backspace" | "ctrl+backspace" => {
            let from = out_of_chip(spans, word_left(buf, pos), false);
            ed(cut(buf, from, pos), from, None)
        }
        "alt+delete" | "ctrl+delete" | "alt+d" => {
            let to = out_of_chip(spans, word_right(buf, pos), true);
            ed(cut(buf, pos, to), pos, None)
        }
        _ => None,
    }
}

pub fn is_undo(s: &str) -> bool {
    matches!(s, "super+z" | "ctrl+_" | "ctrl+/")
}

pub fn is_redo(s: &str) -> bool {
    matches!(s, "shift+super+z" | "super+shift+z" | "ctrl+y")
}

const MAX_UNDO: usize = 200;

/// A box's texts to go back to (undoStack). Typing straight after typing
/// joins the step before it.
#[derive(Clone, Debug, Default)]
pub struct Undo {
    past: Vec<(Vec<char>, usize)>,
    future: Vec<(Vec<char>, usize)>,
    typing: bool,
}

impl Undo {
    pub fn save(&mut self, text: &[char], pos: usize, typing: bool) {
        if typing && self.typing {
            return;
        }
        self.typing = typing;
        self.past.push((text.to_vec(), pos));
        if self.past.len() > MAX_UNDO {
            self.past.remove(0);
        }
        self.future.clear();
    }

    pub fn undo(&mut self, text: &[char], pos: usize) -> Option<(Vec<char>, usize)> {
        self.typing = false;
        let st = self.past.pop()?;
        self.future.push((text.to_vec(), pos));
        Some((st.0.clone(), st.1.min(st.0.len())))
    }

    pub fn redo(&mut self, text: &[char], pos: usize) -> Option<(Vec<char>, usize)> {
        self.typing = false;
        let st = self.future.pop()?;
        self.past.push((text.to_vec(), pos));
        Some((st.0.clone(), st.1.min(st.0.len())))
    }
}

/// A box's text, its cursor and selection, and its undo: the Agents box,
/// a Session's message box, a sheet's field. A message box's long pastes
/// are kept beside it, shown as chips (pastes.go: Model.pastes,
/// hostConn.pastes), the chip under the pointer lit (chipHot).
#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub text: Vec<char>,
    pub pos: usize,
    pub anchor: Option<usize>,
    pub undo: Undo,
    pub pastes: Pastes,
    /// The paste chip under the pointer, to open with space or a click;
    /// empty for none.
    pub hot: Seg,
}

impl Editor {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn string(&self) -> String {
        self.text.iter().collect()
    }

    /// Empties the box, and lets its pastes go with its text: the box
    /// has been sent or cleared (pastes{}).
    pub fn clear(&mut self) {
        self.text.clear();
        self.pos = 0;
        self.anchor = None;
        self.pastes = Pastes::default();
        self.hot = Seg::default();
    }

    /// A paste into a message box, at the cursor: a long one kept and
    /// shown as its chip (model.go:1435-1458). A short one keeps its
    /// lines where lines is set (a Session's box), else is made one line
    /// (the Agents box).
    pub fn paste(&mut self, text: &str, lines: bool) {
        if is_long_paste(text) {
            let chip = self.pastes.add(text);
            self.insert_str(&chip);
        } else if lines {
            self.insert_str(text);
        } else {
            self.insert_str(&crate::style::one_line(text));
        }
    }

    /// The box's text as it goes out, each chip its paste again; tagged,
    /// each between <pasted_content> tags as Claude Code sends a paste
    /// (expand).
    pub fn sent(&self, tagged: bool) -> String {
        self.pastes.expand(&self.string(), tagged.then(paste_tag))
    }

    /// Puts the paste of the chip with the character at p in it back in
    /// the box, to read and edit in place, the cursor after it; undo folds
    /// it again. False when there's no paste kept here at p (openChip).
    pub fn open_chip_at(&mut self, p: usize) -> bool {
        self.hot = Seg::default();
        let (sp, ok) = self.pastes.chip_at(&self.text, p);
        if !ok {
            return false;
        }
        let Some((buf, pos)) = self.pastes.open(&self.text, sp) else {
            return false;
        };
        self.undo.save(&self.text, self.pos, false);
        self.text = buf;
        self.pos = pos;
        self.anchor = None;
        true
    }

    /// Lights the chip with the character at p in it, if it's a paste
    /// kept here (hoverChip); whether what's lit changed.
    pub fn hover(&mut self, p: Option<usize>) -> bool {
        let was = self.hot;
        self.hot = match p.map(|p| self.pastes.chip_at(&self.text, p)) {
            Some((sp, true)) => sp,
            _ => Seg::default(),
        };
        self.hot != was
    }

    /// A key while a chip is lit: it's unlit (lit again only once the
    /// pointer moves), and space opens it. True when space opened it
    /// (model.go:1464-1469).
    pub fn key_on_hot(&mut self, s: &str) -> bool {
        let hot = std::mem::take(&mut self.hot);
        hot != Seg::default() && s == "space" && self.open_chip_at(hot.from)
    }

    /// Puts s in the box, the cursor at its end.
    pub fn set(&mut self, s: &str) {
        self.text = s.chars().collect();
        self.pos = self.text.len();
        self.anchor = None;
    }

    /// Types s at the cursor, over the selection (a paste).
    pub fn insert_str(&mut self, s: &str) {
        let mut pos = self.pos.min(self.text.len());
        if let Some(a) = self
            .anchor
            .take()
            .filter(|&a| a != pos && a <= self.text.len())
        {
            let (from, to) = (a.min(pos), a.max(pos));
            self.text = cut(&self.text, from, to);
            pos = from;
        }
        let r: Vec<char> = s.chars().collect();
        self.undo.save(&self.text, pos, false);
        self.text = insert(&self.text, pos, &r);
        self.pos = pos + r.len();
    }

    /// Runs an editing key on the box: chips whole, undo kept (editInput).
    /// None when it isn't an editing key; Some with what was copied.
    pub fn key(&mut self, k: &Press) -> Option<String> {
        if is_undo(&k.s) || is_redo(&k.s) {
            let r = if is_undo(&k.s) {
                self.undo.undo(&self.text, self.pos)
            } else {
                self.undo.redo(&self.text, self.pos)
            };
            if let Some((t, p)) = r {
                self.text = t;
                self.pos = p;
                self.anchor = None;
            }
            return Some(String::new());
        }
        let e = edit_chips(&self.text, self.pos, self.anchor, k)?;
        if e.buf != self.text {
            let typing =
                k.text().is_some_and(|c| c != ' ') && e.anchor.is_none() && self.anchor.is_none();
            self.undo.save(&self.text, self.pos, typing);
        }
        self.text = e.buf;
        self.pos = e.pos;
        self.anchor = e.anchor;
        Some(e.copied)
    }

    /// The selection, as [from, to); None without one.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.anchor
            .filter(|&a| a != self.pos)
            .map(|a| (a.min(self.pos), a.max(self.pos)))
    }
}

/// A word of text that may be a path, and where it was (span).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSpan {
    pub from: usize,
    pub to: usize,
    pub tok: String,
}

/// Splits text like split_paths, keeping where each word was, as byte
/// offsets (pathSpans).
pub fn path_spans(text: &str) -> Vec<PathSpan> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quote, mut esc, mut start): (Option<char>, bool, Option<usize>) = (None, false, None);
    let flush = |out: &mut Vec<PathSpan>, cur: &mut String, start: &mut Option<usize>, end| {
        if let Some(s) = start.take() {
            out.push(PathSpan {
                from: s,
                to: end,
                tok: std::mem::take(cur),
            });
        }
        cur.clear();
    };
    for (i, r) in text.char_indices() {
        let blank = matches!(r, ' ' | '\t' | '\n' | '\r');
        if start.is_none() && blank && quote.is_none() && !esc {
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
        if esc {
            cur.push(r);
            esc = false;
        } else if r == '\\' && quote != Some('\'') {
            esc = true;
        } else if let Some(q) = quote {
            if r == q {
                quote = None;
            } else {
                cur.push(r);
            }
        } else if r == '\'' || r == '"' {
            quote = Some(r);
        } else if blank {
            flush(&mut out, &mut cur, &mut start, i);
        } else {
            cur.push(r);
        }
    }
    flush(&mut out, &mut cur, &mut start, text.len());
    out
}

/// Splits on unescaped, unquoted whitespace, undoing the escaping
/// terminals add when a file is dropped on them (splitPaths). Only plain
/// whitespace: macOS puts a narrow no-break space in screenshot names.
pub fn split_paths(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quote, mut esc) = (None, false);
    for r in s.trim().chars() {
        if esc {
            cur.push(r);
            esc = false;
        } else if r == '\\' && quote != Some('\'') {
            esc = true;
        } else if let Some(q) = quote {
            if r == q {
                quote = None;
            } else {
                cur.push(r);
            }
        } else if r == '\'' || r == '"' {
            quote = Some(r);
        } else if matches!(r, ' ' | '\t' | '\n' | '\r') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(r);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// tok as a path on disk: a file URL unescaped, ~ expanded (filePath).
pub fn file_path(tok: &str) -> String {
    let mut tok = tok.to_string();
    if let Some(rest) = tok.strip_prefix("file://") {
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        if let Some(p) = unescape_url(rest) {
            tok = p;
        }
    }
    if let Some(rest) = tok.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        tok = Path::new(&home).join(rest).to_string_lossy().into_owned();
    }
    tok
}

// url.PathUnescape: %XX decoded; None on a bad escape.
fn unescape_url(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Whether p is named as an image (isImageFile).
pub fn is_image_file(p: &str) -> bool {
    let ext = Path::new(p)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
}

/// What's at a path, as a lookup says (pathLookup): there, and a folder.
pub type Look<'a> = &'a dyn Fn(&str) -> Option<bool>;

/// tok as an image file that exists, as look says (imagePath).
pub fn image_path(tok: &str, look: Look) -> Option<String> {
    let p = file_path(tok);
    if !is_image_file(&p) {
        return None;
    }
    match look(&p) {
        Some(false) => Some(p),
        _ => None,
    }
}

/// Whether a paste is files dropped onto the terminal: nothing but
/// absolute paths that exist (isDrop).
pub fn is_drop(text: &str, look: Look) -> bool {
    let spans = path_spans(text);
    !spans.is_empty()
        && spans.iter().all(|s| {
            let p = file_path(&s.tok);
            Path::new(&p).is_absolute() && look(&p).is_some()
        })
}

/// A paste read as image files dropped onto the terminal; None unless
/// every piece is one that exists (imagePaths).
pub fn image_paths(paste: &str, look: Look) -> Option<Vec<String>> {
    split_paths(paste)
        .iter()
        .map(|t| image_path(t, look))
        .collect()
}

/// The image files named anywhere in text taken out of it: the rest of
/// the text, and the images (extractImages).
pub fn extract_images(text: &str, look: Look) -> (String, Vec<String>) {
    let mut imgs = Vec::new();
    let mut out = String::new();
    let mut last = 0;
    for sp in path_spans(text) {
        let Some(p) = image_path(&sp.tok, look) else {
            continue;
        };
        imgs.push(p);
        out.push_str(&text[last..sp.from]);
        last = sp.to;
        if text[last..].starts_with(' ') {
            last += 1; // and the space after it
        }
    }
    if imgs.is_empty() {
        return (text.to_string(), imgs);
    }
    out.push_str(&text[last..]);
    (out.trim().to_string(), imgs)
}

/// What's at p on disk: None when nothing, Some(is a folder). It reads
/// the disk: never on the UI thread for a path typed live.
pub fn look_disk(p: &str) -> Option<bool> {
    std::fs::metadata(p).ok().map(|m| m.is_dir())
}
