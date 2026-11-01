//! A shell step drawn by what happened: its row says how each of its tools
//! came out, the problems its output lists are a table (where, rule, what;
//! enter on a row opens the whole message), and a long quoted value in its
//! command folds to a count. New in the Rust port; works from the generic
//! model (Kind::Shell, its command and output), whichever agent ran it.

use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::Style;
use rush_core::tool::Kind;

use super::draw::presets::check_names;
use super::draw::{Drawer, bash_out};
use super::outcome::{self, Came, Diag, Sev};
use super::shell::{fields_of, segments, truncate_cells};
use super::style::*;
use super::txt::{Txt, blanks, width, wrap};
use super::udiff::{clean_output, collapse_cr, expand_tabs};
use crate::model::{Status, StepId};

/// The table's rows drawn before the rest are left to ctrl+o.
const TABLE_MOST: usize = 12;

impl Drawer<'_> {
    /// How each checking tool of a finished shell step came out; empty
    /// when it ran none, or none says.
    pub(crate) fn came(&self, id: StepId) -> Rc<Vec<Came>> {
        if let Some(c) = self.cames.borrow().get(&id) {
            return c.clone();
        }
        let st = self.s.step(id);
        let mut got = Vec::new();
        if st.kind == Kind::Shell && !matches!(st.status, Status::Running | Status::Waiting) {
            let info = self.info(id);
            let cmd = &info.call.input.command;
            if !outcome::tools(cmd).is_empty() {
                let out = bash_out(st, &info.out);
                let ls: Vec<String> = collapse_cr(&out)
                    .split('\n')
                    .map(|l| expand_tabs(&clean_output(l)))
                    .collect();
                let ds = outcome::diagnostics(&ls, true);
                let ok = st.status == Status::Ok && st.exit <= 0;
                got = outcome::outcome(cmd, &ls.join("\n"), &ds, ok);
            }
        }
        let got = Rc::new(got);
        self.cames.borrow_mut().insert(id, got.clone());
        got
    }

    /// A shell step's row by what happened: what it's for, then each
    /// tool and how it came out. None when nothing says.
    pub(crate) fn shell_head(&self, id: StepId, base: Style) -> Option<Txt> {
        let cs = self.came(id);
        if cs.is_empty() {
            return None;
        }
        let info = self.info(id);
        let x = &info.call.input;
        let desc = one_line(&x.description);
        let name = if desc.is_empty() {
            let mut ns = check_names(&x.command);
            if ns.len() > 1 {
                ns.retain(|n| *n != "fmt");
            }
            ns.join(", ")
        } else {
            desc
        };
        let mut t = Txt::new();
        if !name.is_empty() {
            t = paint(base, &name).and_raw("  ");
        }
        // One tool under a name needs no name of its own.
        let solo = cs.len() == 1 && !name.is_empty();
        for (i, c) in cs.iter().enumerate() {
            if i > 0 {
                t.push(faint(" · "));
            }
            if !solo {
                t.push(paint(base, &c.tool));
                t.push_str(" ", Style::default());
            }
            let col = if c.bad {
                C_RED
            } else if c.warn {
                C_YELLOW
            } else if c.what == "✓" {
                C_GREEN
            } else {
                C_DIM
            };
            t.push(paint(col, &c.what));
        }
        Some(t)
    }

    /// The problems a step's output lists, a row each: where (its path's
    /// shortest tail and line), its rule, its message's first sentence,
    /// errors red and warnings yellow. A row opened by hand (enter) shows
    /// all it printed for it. Returns how many rows it left out.
    pub(crate) fn diag_rows(
        &mut self,
        r: &str,
        ds: &[Diag],
        pad: &Txt,
        edge: &Txt,
        w: usize,
    ) -> usize {
        let paths: Vec<&str> = ds.iter().map(|d| d.path.as_str()).collect();
        let tails = outcome::short_tails(&paths);
        let locs: Vec<(String, String)> = tails
            .into_iter()
            .zip(ds)
            .map(|(t, d)| {
                let at = if d.line > 0 {
                    format!(":{}", d.line)
                } else {
                    String::new()
                };
                (t, at)
            })
            .collect();
        let loc_w = locs
            .iter()
            .map(|(p, a)| width(p) + width(a))
            .max()
            .unwrap_or(0)
            .min((w / 3).max(12));
        let mut rule_w = ds.iter().map(|d| width(&d.rule)).max().unwrap_or(0).min(24);
        let mut msg_w = w as isize - loc_w as isize - 3 - (rule_w as isize + 2);
        if rule_w > 0 && msg_w < 16 {
            rule_w = 0;
            msg_w = w as isize - loc_w as isize - 3;
        }
        if rule_w == 0 {
            msg_w += 2;
        }
        let msg_w = msg_w.max(8) as usize;
        let shown = if self.o.verbose {
            ds.len()
        } else {
            ds.len().min(TABLE_MOST)
        };
        for (k, d) in ds.iter().enumerate().take(shown) {
            let rr = diag_ref(r, k);
            let open = self.o.open.get(&rr).copied().unwrap_or(false);
            let tint = if d.sev == Sev::Error { C_RED } else { C_YELLOW };
            let (p, at) = &locs[k];
            let mut loc = paint(tint, p).and(faint(at));
            if loc.width() > loc_w {
                loc = loc.truncate(loc_w, &faint("…"));
            }
            let lw = loc.width();
            let mut left = pad
                .clone()
                .and(edge.clone())
                .and(loc)
                .and(blanks(loc_w - lw + 3));
            if rule_w > 0 {
                let rule = truncate_cells(&d.rule, rule_w);
                let rw = width(&rule);
                left.push(paint(C_SUB, &rule));
                left.push(blanks(rule_w - rw + 2));
            }
            left.push(paint(C_OUT, &truncate_cells(d.sentence(), msg_w)));
            // Picked, the row says what enter does (and shows ▸ as a
            // folded row does, so enter opens it).
            let right = if open {
                faint("▾").and_raw("  ")
            } else if self.o.selected == rr {
                faint("▸ ").and(dim("the full message")).and_raw("  ")
            } else {
                Txt::new()
            };
            self.add(&rr, BG_WELL, left, right);
            if open {
                let ind = d
                    .full
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.len() - l.trim_start().len())
                    .min()
                    .unwrap_or(0);
                for l in &d.full {
                    let l = l.get(ind..).unwrap_or(l.trim_start());
                    for (j, row) in wrap(&paint(C_DIM, l), w).into_iter().enumerate() {
                        let left = pad.clone().and(edge.clone()).and(row);
                        self.add("", BG_WELL, left, Txt::new());
                        if j > 0 {
                            self.wrapped();
                        }
                    }
                }
            }
        }
        ds.len() - shown
    }
}

impl Drawer<'_> {
    /// t with what the step searched for marked where it shows.
    pub(crate) fn marked(&self, t: Txt) -> Txt {
        match &self.hit {
            Some(re) => mark_hits(t, re),
            None => t,
        }
    }
}

/// t with each match of re bold yellow, the search's colour, over
/// whatever colour it had.
pub(crate) fn mark_hits(t: Txt, re: &Regex) -> Txt {
    let plain = t.plain();
    let hits: Vec<(usize, usize)> = re
        .find_iter(&plain)
        .filter(|m| !m.is_empty())
        .map(|m| (m.start(), m.end()))
        .collect();
    if hits.is_empty() {
        return t;
    }
    let mut out = Txt::new();
    let mut off = 0;
    for sp in t.0 {
        let (a, b) = (off, off + sp.text.len());
        off = b;
        let mut cur = a;
        for &(h0, h1) in &hits {
            if h1 <= cur || h0 >= b {
                continue;
            }
            let (s0, s1) = (h0.max(cur), h1.min(b));
            if s0 > cur {
                out.push(Txt::styled(&sp.text[cur - a..s0 - a], sp.style));
            }
            out.push(Txt::styled(&sp.text[s0 - a..s1 - a], C_YELLOW.bold()));
            cur = s1;
        }
        if cur < b {
            out.push(Txt::styled(&sp.text[cur - a..], sp.style));
        }
    }
    out
}

/// What a command searches for with grep, rg, ag or git grep, as a
/// pattern to mark in its output; None when it searches for nothing, or
/// for something too short to mark.
pub(crate) fn search_hit(cmd: &str) -> Option<Regex> {
    for seg in segments(cmd) {
        let parts = std::iter::once(seg.text.clone()).chain(seg.filters.iter().cloned());
        for part in parts {
            let ws = fields_of(&part);
            let Some(at) = ws.iter().position(|w| {
                matches!(
                    w.rsplit('/').next().unwrap_or(w),
                    "rg" | "grep" | "egrep" | "fgrep" | "ag" | "ack"
                )
            }) else {
                continue;
            };
            let (mut pat, mut fold, mut fixed) = (None::<String>, false, false);
            let mut i = at + 1;
            while i < ws.len() {
                let w = ws[i].as_str();
                match w {
                    "-e" | "--regexp" => {
                        pat = ws.get(i + 1).cloned();
                        i += 2;
                        continue;
                    }
                    "-i" | "--ignore-case" => fold = true,
                    "-F" | "--fixed-strings" => fixed = true,
                    "-g" | "--glob" | "-t" | "--type" | "-T" | "--type-not" | "-A" | "-B"
                    | "-C" | "-m" | "--max-count" | "-f" | "--file" | "--include" | "--exclude"
                    | "-d" | "--max-depth" => {
                        i += 2;
                        continue;
                    }
                    _ if w.starts_with('-') => {
                        // Bundled short flags: -in, -rnI.
                        if !w.starts_with("--") {
                            fold |= w.contains('i');
                            fixed |= w.contains('F');
                        }
                    }
                    _ if pat.is_none() => pat = Some(w.to_string()),
                    _ => {}
                }
                i += 1;
            }
            let p = pat?;
            let p = unquote_word(&p);
            if p.chars().count() < 2 {
                return None;
            }
            let p = if fixed {
                regex::escape(p)
            } else {
                p.to_string()
            };
            let p = if fold { format!("(?i){p}") } else { p };
            return Regex::new(&p).ok();
        }
    }
    None
}

fn unquote_word(w: &str) -> &str {
    let b = w.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &w[1..w.len() - 1]
    } else {
        w
    }
}

/// Whether most of what a command printed is numbered lines of code:
/// "12:code", "a.rs:12:code", "a.rs-13-code", "    12\tcode".
pub(crate) fn numbered_code(s: &str) -> bool {
    let (mut lines, mut numbered) = (0, 0);
    for l in s.split('\n').take(400) {
        if l.trim().is_empty() || l.trim() == "--" {
            continue;
        }
        lines += 1;
        if rush_hl::code::code_prefix(l).0 > 0 {
            numbered += 1;
        }
    }
    numbered > 0 && numbered * 5 >= lines * 3
}

/// The language of the last file a command names that has one: the file
/// it read or searched.
pub(crate) fn command_lang(cmd: &str) -> Option<&'static rush_hl::Lang> {
    fields_of(cmd)
        .iter()
        .rev()
        .find_map(|w| super::code::lang_for(unquote_word(w)))
}

/// Whether a step's stderr is only the harness's notes about it.
pub(crate) fn noise_only(s: &str) -> bool {
    s.lines().filter(|l| !l.trim().is_empty()).all(|l| {
        let l = l.trim();
        l.starts_with("Shell cwd was reset to ") || l.starts_with("Session cwd remains ")
    })
}

/// The ref of the k-th row of a step's problems: enter opens it.
pub fn diag_ref(step: &str, k: usize) -> String {
    format!("{step}:diag:{k}")
}

/// The ref of a command line's folded value: enter shows it whole.
pub fn value_ref(step: &str, line: usize) -> String {
    format!("{step}:val:{line}")
}

/// A quoted value too long to read in a command, as a count: "19 files"
/// when its words are paths, "N words", else "N chars". None when it's
/// short enough to show.
pub(crate) fn value_count(tok: &str, room: usize) -> Option<String> {
    static FILE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(/|\.[A-Za-z0-9]{1,8}$)").expect("a valid pattern"));
    let b = tok.as_bytes();
    if b.len() < 2 || !(b[0] == b'"' || b[0] == b'\'') || b[b.len() - 1] != b[0] {
        return None;
    }
    let inner = &tok[1..tok.len() - 1];
    let words: Vec<&str> = inner.split_whitespace().collect();
    let long = width(inner) > 48 && words.len() >= 3 || width(inner) > room.max(24);
    if !long || inner.contains('\n') {
        return None;
    }
    Some(
        if words.len() >= 2 && words.iter().all(|w| FILE.is_match(w)) {
            format!("{} files", words.len())
        } else if words.len() >= 2 {
            format!("{} words", words.len())
        } else {
            format!("{} chars", inner.chars().count())
        },
    )
}

/// A command's tinted words with each quoted value's words its own, so a
/// row breaks between them, never inside a path.
pub(crate) fn split_quoted(toks: Vec<Txt>) -> Vec<Txt> {
    let mut out = Vec::with_capacity(toks.len());
    for t in toks {
        let p = t.plain();
        if t.is_blank() || !p.contains(' ') || p.contains('\n') {
            out.push(t);
            continue;
        }
        let style = t.0.first().map(|s| s.style).unwrap_or_default();
        for (i, w) in p.split(' ').enumerate() {
            if i > 0 {
                out.push(Txt::raw(" "));
            }
            if !w.is_empty() {
                out.push(Txt::styled(w, style));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_fold_to_a_count() {
        let files = format!(
            "\"{}\"",
            (0..19)
                .map(|i| format!("src/f{i}.ts"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(value_count(&files, 60).as_deref(), Some("19 files"));
        assert_eq!(value_count("\"a b\"", 60), None);
        let words = format!("'{}'", "lorem ipsum ".repeat(8).trim());
        assert_eq!(value_count(&words, 60).as_deref(), Some("16 words"));
        assert_eq!(value_count("plain", 60), None);
    }

    #[test]
    fn searches_mark_what_they_looked_for() {
        let re = search_hit("cd x && rg -n 'marks' crates/a.rs | head -20").expect("a pattern");
        let t = mark_hits(Txt::raw("p.activity.marks.then"), &re);
        let hit = t.0.iter().find(|s| s.text == "marks").expect("marked");
        assert_eq!(hit.style, C_YELLOW.bold());
        assert!(search_hit("grep -rin -e Foo src").unwrap().is_match("foo"));
        assert!(search_hit("ls -la").is_none());
        assert!(numbered_code("170:    pub x: u8,\n1083:   let y = 1;"));
        assert!(!numbered_code("hello\nworld"));
        assert_eq!(
            command_lang("sed -n 1,40p x.go").map(|l| l.name),
            Some("go")
        );
        assert!(noise_only("Shell cwd was reset to /Users/x\n"));
        assert!(!noise_only("error: boom"));
    }

    #[test]
    fn quoted_words_split_between_words() {
        let toks = vec![Txt::raw("P="), Txt::raw("\"a/b.ts c/d.ts\"")];
        let got: Vec<String> = split_quoted(toks).iter().map(Txt::plain).collect();
        assert_eq!(got, ["P=", "\"a/b.ts", " ", "c/d.ts\""]);
    }
}
