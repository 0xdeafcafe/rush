//! A diff a command printed (git diff, git show, diff -u), drawn as an edit
//! is, and the clean-up tool output gets before it's drawn. Port of
//! internal/convo/udiff.go and render.go:2105-2160 (cleanOutput,
//! unscreen), :4126 (collapseCR) and :4138 (expandTabs).

use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::Color;
use rush_hl::Lang;

use super::code::{HlState, highlight, lang_for};
use super::draw::Drawer;
use super::shell::unquote;
use super::style::{C_DIM, C_GREEN, C_RED, C_SUB, C_TEXT, dim, faint, paint};
use super::txt::{Txt, blanks};

/// What one line of output is in the diff it prints.
#[derive(Clone, Default)]
pub(crate) struct DLine {
    /// 'f' a file's header, 's' a header line under it, 'h' a hunk's
    /// start, ' ' '+' '-' a line kept, added or removed, '\\' git's "no
    /// newline", 0 a line that isn't the diff's.
    pub kind: u8,
    pub n: usize,
    pub pair: isize,
    pub file: isize,
    pub path: String,
    pub note: String,
    pub add: usize,
    pub del: usize,
    pub ctx: String,
}

/// The diff in a command's output, line by line.
pub(crate) struct UDiff {
    pub ls: Vec<DLine>,
    old: HlState,
    new: HlState,
    /// Every line is in a diff: it folds by rows, not as output does.
    pub whole: bool,
}

static HUNK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@@+ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@+ ?(.*)$").unwrap());

struct Parse {
    u: UDiff,
    file: isize,
    hunk: bool,
    old_n: usize,
    new_n: usize,
    old_left: isize,
    new_left: isize,
}

/// The diff in lines, those `inside` says are a diff's; None when there's
/// no file in them.
pub(crate) fn parse_diff(lines: &[&str], inside: impl Fn(usize) -> bool) -> Option<UDiff> {
    let mut p = Parse {
        u: UDiff {
            ls: vec![DLine::default(); lines.len()],
            old: HlState::default(),
            new: HlState::default(),
            whole: true,
        },
        file: -1,
        hunk: false,
        old_n: 0,
        new_n: 0,
        old_left: 0,
        new_left: 0,
    };
    let mut found = false;
    for (i, l) in lines.iter().enumerate() {
        p.u.ls[i].pair = -1;
        p.u.ls[i].file = p.file;
        if !inside(i) {
            p.u.whole = false;
            p.file = -1;
            p.hunk = false;
            p.old_left = 0;
            p.new_left = 0;
            p.u.ls[i].file = -1;
            continue;
        }
        let l = clean_output(l);
        if p.file >= 0 && p.hunk && p.hunk_line(i, &l) {
            continue;
        }
        p.hunk = false;
        let next = lines
            .get(i + 1)
            .map(|n| clean_output(n))
            .unwrap_or_default();
        found = p.header(i, &l, &next) || found;
    }
    if !found {
        return None;
    }
    p.u.pair_up();
    Some(p.u)
}

impl Parse {
    fn hunk_line(&mut self, i: usize, l: &str) -> bool {
        let c = l.as_bytes().first().copied().unwrap_or(b' ');
        if self.old_left <= 0
            && self.new_left <= 0
            && (c == b' ' || l.starts_with("--- ") || l.starts_with("+++ "))
        {
            return false;
        }
        let f = self.file as usize;
        match c {
            b' ' => {
                self.u.ls[i].kind = b' ';
                self.u.ls[i].n = self.new_n;
                self.old_n += 1;
                self.new_n += 1;
                self.old_left -= 1;
                self.new_left -= 1;
            }
            b'-' => {
                self.u.ls[i].kind = b'-';
                self.u.ls[i].n = self.old_n;
                self.old_n += 1;
                self.old_left -= 1;
                self.u.ls[f].del += 1;
            }
            b'+' => {
                self.u.ls[i].kind = b'+';
                self.u.ls[i].n = self.new_n;
                self.new_n += 1;
                self.new_left -= 1;
                self.u.ls[f].add += 1;
            }
            b'\\' => self.u.ls[i].kind = b'\\',
            _ => return false,
        }
        true
    }

    fn header(&mut self, i: usize, l: &str, next: &str) -> bool {
        if l.starts_with("diff -") {
            self.file = i as isize;
            let x = &mut self.u.ls[i];
            x.kind = b'f';
            x.file = i as isize;
            x.path = diff_path(l);
            return true;
        }
        if self.file >= 0 && header_line(l) {
            self.u.ls[i].kind = b's';
            let f = self.file as usize;
            file_note(&mut self.u.ls[f], l);
            return false;
        }
        if l.starts_with("--- ") && next.starts_with("+++ ") {
            self.file = i as isize;
            let x = &mut self.u.ls[i];
            x.kind = b'f';
            x.file = i as isize;
            x.path = side_path(next);
            if x.path.is_empty() {
                x.path = side_path(l);
                x.note = "deleted".into();
            } else if side_path(l).is_empty() {
                x.note = "new".into();
            }
            return true;
        }
        if self.file >= 0 && l.starts_with("@@") {
            if let Some(m) = HUNK.captures(l) {
                self.hunk = true;
                self.old_n = m[1].parse().unwrap_or(0);
                self.new_n = m[3].parse().unwrap_or(0);
                self.old_left = count(m.get(2).map_or("", |x| x.as_str()));
                self.new_left = count(m.get(4).map_or("", |x| x.as_str()));
                self.u.ls[i].kind = b'h';
                self.u.ls[i].ctx = m[5].to_string();
            }
            return false;
        }
        self.file = -1;
        self.u.ls[i].file = -1;
        false
    }
}

fn count(s: &str) -> isize {
    if s.is_empty() {
        1
    } else {
        s.parse().unwrap_or(0)
    }
}

fn file_note(f: &mut DLine, l: &str) {
    if l.starts_with("new file") {
        f.note = "new".into();
    } else if l.starts_with("deleted file") {
        f.note = "deleted".into();
    } else if let Some(r) = l.strip_prefix("rename from ") {
        f.note = format!("renamed from {r}");
    } else if let Some(r) = l.strip_prefix("rename to ") {
        f.path = r.to_string();
    } else if let Some(r) = l.strip_prefix("copy from ") {
        f.note = format!("copied from {r}");
    } else if l.starts_with("Binary files ") {
        f.note = "binary".into();
    } else if l.starts_with("+++ ") {
        let p = side_path(l);
        if !p.is_empty() {
            f.path = p;
        }
    }
}

fn header_line(l: &str) -> bool {
    [
        "index ",
        "new file mode",
        "deleted file mode",
        "old mode",
        "new mode",
        "similarity index",
        "dissimilarity index",
        "rename from ",
        "rename to ",
        "copy from ",
        "copy to ",
        "Binary files ",
        "--- ",
        "+++ ",
    ]
    .iter()
    .any(|p| l.starts_with(p))
}

fn diff_path(l: &str) -> String {
    if let Some(rest) = l.strip_prefix("diff --git ") {
        if let Some(i) = rest.rfind(" b/") {
            return unquote(&rest[i + 3..]).to_string();
        }
        return unquote(rest).to_string();
    }
    let l = l
        .strip_prefix("diff --cc ")
        .or(l.strip_prefix("diff "))
        .unwrap_or(l);
    for a in l.split_whitespace().rev() {
        if !a.starts_with('-') {
            return unquote(a).to_string();
        }
    }
    String::new()
}

fn side_path(l: &str) -> String {
    let p = l.get(4..).unwrap_or("");
    let p = p.split('\t').next().unwrap_or("");
    let p = unquote(p.trim());
    if p == "/dev/null" {
        return String::new();
    }
    p.strip_prefix("a/")
        .or(p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_string()
}

impl UDiff {
    fn pair_up(&mut self) {
        let mut i = 0;
        while i < self.ls.len() {
            if self.ls[i].kind != b'-' {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < self.ls.len() && self.ls[j].kind == b'-' {
                j += 1;
            }
            let mut k = j;
            while k < self.ls.len() && self.ls[k].kind == b'+' {
                k += 1;
            }
            for n in 0..(j - i).min(k - j) {
                self.ls[i + n].pair = (j + n) as isize;
                self.ls[j + n].pair = (i + n) as isize;
            }
            i = k.max(i + 1);
        }
    }

    /// Whether line i draws a row of its own.
    pub fn drawn(&self, i: usize) -> bool {
        self.ls[i].kind != b's'
    }

    /// Draws line i of the diff after pad, w cells wide; false when it
    /// isn't one of the diff's (udiff.go:276).
    pub fn line(
        &mut self,
        d: &mut Drawer,
        pad: Txt,
        w: usize,
        lg: Option<&'static Lang>,
        lines: &[&str],
        i: usize,
    ) -> bool {
        let x = self.ls[i].clone();
        let mut lg = lg;
        if x.file >= 0
            && let Some(f) = lang_for(&self.ls[x.file as usize].path)
        {
            lg = Some(f);
        }
        let code = |j: usize| -> String {
            let s = clean_output(lines[j]);
            s.get(1..).map(str::to_string).unwrap_or_default()
        };
        match x.kind {
            b'f' => {
                self.old = HlState::default();
                self.new = HlState::default();
                if i > 0 {
                    d.add_rows(Color::Default, pad.clone(), Txt::new(), Txt::new(), w, 0);
                }
                d.add_rows(Color::Default, pad, Txt::new(), file_head(&x), w, 2);
            }
            b's' => {}
            b'h' => {
                self.old = HlState::default();
                self.new = HlState::default();
                let k = if i > 0 { self.ls[i - 1].kind } else { 0 };
                if i > 0 && k != b's' && k != b'f' || !x.ctx.is_empty() {
                    let mut st = HlState::default();
                    let ctx = highlight(lg, &mut st, &expand_tabs(&x.ctx), C_DIM, None);
                    d.add_rows(
                        Color::Default,
                        pad,
                        faint("    ⋯ "),
                        ctx,
                        w.saturating_sub(6),
                        1,
                    );
                }
            }
            b'+' | b'-' => {
                let pair = if x.pair >= 0 {
                    code(x.pair as usize)
                } else {
                    String::new()
                };
                let mut st = if x.kind == b'-' { self.old } else { self.new };
                d.diff_line(
                    pad,
                    w.saturating_sub(8),
                    lg,
                    &mut st,
                    x.kind as char,
                    x.n,
                    &code(i),
                    &pair,
                    x.pair >= 0,
                );
                if x.kind == b'-' {
                    self.old = st;
                } else {
                    self.new = st;
                }
            }
            b' ' => {
                let hl = highlight(lg, &mut self.new, &expand_tabs(&code(i)), C_SUB, None);
                for (j, r) in d
                    .code_rows(&hl, w.saturating_sub(8), 6)
                    .into_iter()
                    .enumerate()
                {
                    if j == 0 {
                        let t = pad
                            .clone()
                            .and(faint(&format!("{:>5} ", x.n)))
                            .and_raw("  ")
                            .and(r);
                        d.add_rows(Color::Default, t, Txt::new(), Txt::new(), usize::MAX, 0);
                    } else {
                        let t = pad.clone().and(blanks(8)).and(r);
                        d.add_rows(Color::Default, t, Txt::new(), Txt::new(), usize::MAX, 0);
                    }
                }
                self.old = self.new;
            }
            b'\\' => {
                let l = expand_tabs(&clean_output(lines[i]));
                let l = l.strip_prefix("\\ ").unwrap_or(&l).to_string();
                d.add_rows(
                    Color::Default,
                    pad,
                    blanks(8),
                    faint(&l),
                    w.saturating_sub(8),
                    1,
                );
            }
            _ => return false,
        }
        true
    }
}

/// A file's header in a diff.
fn file_head(x: &DLine) -> Txt {
    let (dir, name) = match x.path.rfind('/') {
        Some(i) => (&x.path[..=i], &x.path[i + 1..]),
        None => ("", x.path.as_str()),
    };
    let mut s = faint(dir).and(paint(C_TEXT.bold(), name));
    match x.note.as_str() {
        "new" => s = s.and_raw("  ").and(paint(C_GREEN, "new")),
        "deleted" => s = s.and_raw("  ").and(paint(C_RED, "deleted")),
        "" => {}
        n => s = s.and_raw("  ").and(dim(n)),
    }
    if x.add > 0 {
        s = s.and_raw("  ").and(paint(C_GREEN, &format!("+{}", x.add)));
    }
    if x.del > 0 {
        s = s.and_raw(" ").and(paint(C_RED, &format!("−{}", x.del)));
    }
    s
}

/// Whether lines hold a diff: a file's start and a hunk.
pub(crate) fn has_diff(lines: &[&str]) -> bool {
    let mut file = false;
    for (i, l) in lines.iter().enumerate() {
        if l.starts_with("diff -")
            || l.starts_with("--- ") && lines.get(i + 1).is_some_and(|n| n.starts_with("+++ "))
        {
            file = true;
        } else if file && l.starts_with("@@ -") {
            return true;
        }
    }
    false
}

/// Every escape sequence and control character out of a line of a tool's
/// output, tabs kept.
pub(crate) fn clean_output(s: &str) -> String {
    if !s.chars().any(|c| (c < ' ' && c != '\t') || c == '\x7f') {
        return s.to_string();
    }
    strip_escapes(s)
        .chars()
        .filter(|&c| !((c < ' ' && c != '\t') || c == '\x7f'))
        .collect()
}

/// s without ANSI escape sequences (CSI, OSC, and two-byte ones).
fn strip_escapes(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != 0x1b {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        match b.get(i) {
            Some(b'[') => {
                i += 1;
                while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
                    i += 1;
                }
                i += 1;
            }
            Some(b']') => {
                i += 1;
                while i < b.len() && b[i] != 0x07 && !(b[i] == 0x1b && b.get(i + 1) == Some(&b'\\'))
                {
                    i += 1;
                }
                i += if i < b.len() && b[i] == 0x1b { 2 } else { 1 };
            }
            Some(_) => i += 1,
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Only the last state of lines redrawn with carriage returns.
pub(crate) fn collapse_cr(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    s.split('\n')
        .map(|l| match l.trim_end_matches('\r').rfind('\r') {
            Some(j) => &l[j + 1..],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tabs as four spaces, escape codes out.
pub(crate) fn expand_tabs(s: &str) -> String {
    let s = if s.contains('\x1b') {
        strip_escapes(s)
    } else {
        s.to_string()
    };
    if s.contains('\t') {
        s.replace('\t', "    ")
    } else {
        s
    }
}

/// A screen a program drew for its own terminal's size undone: padding,
/// gaps pushing text right, runs of blank and ~ rows (render.go:2125).
pub(crate) fn unscreen(s: &str) -> String {
    static GAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" {24,}").unwrap());
    let mut out: Vec<String> = Vec::new();
    let (mut blank, mut tilde) = (0, 0);
    for l in s.split('\n') {
        let mut l = l.trim_end_matches([' ', '\t']).to_string();
        let t = l.trim();
        if !t.is_empty()
            && t != "~"
            && l.contains("                        ")
            && let Some(i) = l.find(|c: char| c != ' ')
        {
            l = format!("{}{}", &l[..i], GAP.replace_all(&l[i..], "   "));
        }
        match l.trim() {
            "" => {
                blank += 1;
                if blank > 1 {
                    continue;
                }
            }
            "~" => {
                tilde += 1;
                if tilde > 1 {
                    continue;
                }
            }
            _ => {
                blank = 0;
                tilde = 0;
            }
        }
        out.push(l);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_git_diff() {
        let lines = [
            "diff --git a/x.go b/x.go",
            "index 1..2 100644",
            "--- a/x.go",
            "+++ b/x.go",
            "@@ -1,2 +1,2 @@",
            " a",
            "-b",
            "+c",
        ];
        let u = parse_diff(&lines, |_| true).unwrap();
        assert_eq!(u.ls[0].kind, b'f');
        assert_eq!(u.ls[0].path, "x.go");
        assert_eq!((u.ls[0].add, u.ls[0].del), (1, 1));
        assert_eq!(u.ls[6].pair, 7);
        assert!(has_diff(&lines));
    }

    #[test]
    fn output_is_cleaned() {
        assert_eq!(clean_output("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(collapse_cr("10%\r50%\r100%"), "100%");
        assert_eq!(unscreen("a\n\n\n\nb"), "a\n\nb");
    }
}
