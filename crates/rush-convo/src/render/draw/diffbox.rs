//! A change to a file in a box: its path and how many lines it added and
//! took away on the top edge (a new, deleted or renamed file says so),
//! then its lines with their old and new numbers, highlighted, removed
//! and added lines on a soft red and green ground and the words that
//! changed on a stronger one. In one column, or side by side on a pane
//! SIDE_BY_SIDE wide or wider, the old file's lines on the left and the
//! new's on the right. Long unchanged stretches fold into a "▸ N lines"
//! row, and a long change shows its first rows and "▸ show all".
//!
//! The Code view draws a box for each file a turn changed; Normal one
//! under each edit step. What's drawn is worked out once a change
//! (render/diffbox.rs); this only lays it out.

use std::rc::Rc;

use rush_core::text::Color;

use super::super::diffbox::{Part, Row, Tag, part_of, rows};
use super::Drawer;
use crate::model::StepId;
use crate::render::style::*;
use crate::render::txt::{Txt, blanks};
use rush_core::tool::Kind;

/// The pane this wide or wider shows a change side by side: each side
/// then has room for about 75 columns of code.
pub const SIDE_BY_SIDE: usize = 160;

/// A box to draw: the edits (to one file) in it, and its refs.
pub(crate) struct BoxSpec<'s> {
    pub ids: &'s [StepId],
    /// Where it starts: the spine and indent - 1 cells.
    pub indent: usize,
    /// The top edge's ref: the file's row in Code, none in Normal.
    pub head_ref: &'s str,
    /// What each fold's ref starts with.
    pub fold_ref: &'s str,
    /// The "▸ show all" row's ref.
    pub all_ref: &'s str,
    /// An edit with no patch shows what it asked to replace.
    pub bare: bool,
    /// What happened to the file besides: deleted, moved, notebook.
    pub note: &'s str,
    /// Its added and taken away lines, when the caller counted them.
    pub counts: Option<(usize, usize)>,
    /// Rows of lines shown until "▸ show all" opens it.
    pub cap: usize,
    /// Shown whole: opened another way (ctrl+o, a file opened in Code).
    pub whole: bool,
}

/// "+a −d" in green and red; "" for neither.
fn add_del(add: usize, del: usize) -> Txt {
    let mut l = Txt::new();
    if add > 0 || del == 0 {
        l.push(paint(C_GREEN, &format!("+{add}")));
    }
    if del > 0 {
        if !l.is_empty() {
            l.push_str(" ", PLAIN);
        }
        l.push(paint(C_RED, &format!("−{del}")));
    }
    l
}

/// A cell x wide between the box's edges, a space either side, on bg.
fn cell(t: Txt, x: usize, bg: Color) -> Txt {
    let t = if t.width() > x { t.take(x) } else { t };
    let fill = x - t.width();
    Txt::raw(" ").and(t).and(blanks(fill)).and_raw(" ").on(bg)
}

/// A line number n wide, blank for 0.
fn num(n: usize, w: usize) -> Txt {
    if w == 0 {
        Txt::new()
    } else if n == 0 {
        blanks(w)
    } else {
        faint(&format!("{n:>w$}"))
    }
}

fn ground(t: Tag) -> Color {
    match t {
        Tag::Same => Color::Default,
        Tag::Del => BG_DEL,
        Tag::Add => BG_ADD,
    }
}

fn sign(t: Tag) -> Txt {
    match t {
        Tag::Same => Txt::raw(" "),
        Tag::Del => minus_sign(),
        Tag::Add => plus_sign(),
    }
}

impl Drawer<'_> {
    /// Draws the box b asks for; false when its edits have no change to
    /// show and nothing happened to the file besides.
    pub(crate) fn diff_box(&mut self, b: &BoxSpec) -> bool {
        let Some(&first) = b.ids.first() else {
            return false;
        };
        let mut parts: Vec<Rc<Part>> = Vec::new();
        // A deleted file's lines aren't known.
        for &id in b.ids.iter().filter(|_| b.note != "deleted") {
            let info = self.info(id);
            if let Some(p) = part_of(&info.call, &info.out, b.bare) {
                parts.push(p);
            }
        }
        if parts.is_empty() && b.note.is_empty() {
            return false;
        }
        let info = self.info(first);
        let (path, to) = (info.call.input.path.clone(), info.call.input.to.clone());
        // A new file: as its tool said, or the whole of it written where
        // there was no change to show (Codex's added file).
        let x = &info.call.input;
        let created = info.out.created
            || (self.s.step(first).kind == Kind::Write
                && info.out.patches.is_empty()
                && x.edits.is_empty()
                && !x.content.is_empty());
        let (add, del) = b.counts.unwrap_or_else(|| {
            parts
                .iter()
                .fold((0, 0), |(a, d), p| (a + p.add, d + p.del))
        });
        let pad = self.sp(b.indent.saturating_sub(1));
        // Side by side when rows may run that wide: not while a column
        // stands beside the conversation (rows kept to CAP_ROW then).
        let side = self.cw >= SIDE_BY_SIDE;
        let w = self.cw.saturating_sub(pad.width() + 1).max(24);

        // The top edge: the path, what happened to the file, the counts.
        let moved = b.note == "moved" && !to.is_empty();
        let shown = if moved { &to } else { &path };
        let mut name = self.file_link(shown, text(&self.p.rel(shown)));
        let mut note = match b.note {
            "deleted" => paint(C_RED, "deleted"),
            "moved" if moved => dim("renamed from ").and(sub(&self.p.rel(&path))),
            "moved" => dim("moved"),
            "notebook" => dim("notebook"),
            _ if created => paint(C_GREEN, "new file"),
            _ => Txt::new(),
        };
        let counts = if add + del == 0 && matches!(b.note, "deleted" | "moved" | "notebook") {
            Txt::new()
        } else {
            add_del(add, del)
        };
        let right = if counts.is_empty() {
            faint("─╮")
        } else {
            Txt::raw(" ").and(counts).and(faint(" ─╮"))
        };
        let left = faint("╭─ ");
        let avail = w.saturating_sub(left.width() + right.width() + 2);
        if !note.is_empty() {
            note = Txt::raw("  ").and(note);
        }
        if name.width() + note.width() > avail {
            note = note.truncate(avail.saturating_sub(name.width().min(12)), &faint("…"));
        }
        if name.width() + note.width() > avail {
            let keep = avail.saturating_sub(note.width()).max(2);
            name = faint("…").and(name.skip((name.width() + 1).saturating_sub(keep)));
        }
        let fill = w.saturating_sub(left.width() + name.width() + note.width() + 1 + right.width());
        let top = pad
            .clone()
            .and(left)
            .and(name)
            .and(note)
            .and_raw(" ")
            .and(faint(&"─".repeat(fill.max(1))))
            .and(right);
        self.add_wide(b.head_ref, top);

        // The lines.
        let nw = parts
            .iter()
            .flat_map(|p| p.hunks.iter().flat_map(|h| h.lines.iter()))
            .map(|l| l.old.max(l.new))
            .max()
            .map_or(0, |n| if n == 0 { 0 } else { n.to_string().len() });
        let fold = |k: usize| format!("{}:fold:{k}", b.fold_ref);
        let o = self.o;
        let laid = rows(&parts, side, |k| {
            o.open.get(&fold(k)).copied().unwrap_or(false)
        });
        let total = laid.iter().filter(|r| r.is_line()).count();
        let opened = o.open.get(b.all_ref).copied().unwrap_or(false);
        let whole = b.whole || opened;
        // One column: the line between the edges; side by side, two.
        let inner = w - 4;
        let lw = (w - 7) / 2;
        let rw = w - 7 - lw;
        let edge = || faint("│");
        let mut shown_lines = 0;
        if laid.is_empty() {
            let say = match b.note {
                "deleted" => "the file was deleted",
                "moved" => "moved, its lines as they were",
                _ if created => "an empty file",
                _ => "no lines changed",
            };
            let l = pad
                .clone()
                .and(edge())
                .and(cell(dim(say), inner, Color::Default))
                .and(edge());
            self.add_wide("", l);
        }
        for row in &laid {
            if row.is_line() {
                if !whole && shown_lines >= b.cap {
                    break;
                }
                shown_lines += 1;
            }
            match *row {
                Row::One((pi, hi, x)) => {
                    let ln = &parts[pi].hunks[hi].lines[x];
                    let lead = if nw > 0 {
                        num(ln.old, nw)
                            .and_raw("  ")
                            .and(num(ln.new, nw))
                            .and_raw(" ")
                    } else {
                        Txt::new()
                    }
                    .and(sign(ln.tag))
                    .and_raw(" ");
                    let lead_w = lead.width();
                    let code_w = inner.saturating_sub(lead_w).max(4);
                    let bg = ground(ln.tag);
                    for (i, r) in self.code_rows(&ln.hl, code_w, 6).into_iter().enumerate() {
                        let body = if i == 0 {
                            lead.clone().and(r)
                        } else {
                            blanks(lead_w).and(r)
                        };
                        let l = pad
                            .clone()
                            .and(edge())
                            .and(cell(body, inner, bg))
                            .and(edge());
                        self.add_wide("", l);
                        if i > 0 {
                            self.wrapped();
                        }
                    }
                }
                Row::Two(a, z) => {
                    let half = |at: Option<(usize, usize, usize)>, x: usize, left: bool| {
                        let Some((pi, hi, k)) = at else {
                            return cell(Txt::new(), x, Color::Default);
                        };
                        let ln = &parts[pi].hunks[hi].lines[k];
                        let n = if left { ln.old } else { ln.new };
                        let lead = if nw > 0 {
                            num(n, nw).and_raw(" ")
                        } else {
                            Txt::new()
                        }
                        .and(sign(ln.tag))
                        .and_raw(" ");
                        let code_w = x.saturating_sub(lead.width()).max(2);
                        let code = ln.hl.truncate(code_w, &faint("…"));
                        cell(lead.and(code), x, ground(ln.tag))
                    };
                    let l = pad
                        .clone()
                        .and(edge())
                        .and(half(a, lw, true))
                        .and(edge())
                        .and(half(z, rw, false))
                        .and(edge());
                    self.add_wide("", l);
                }
                Row::Fold { n, k, open } => {
                    let at = if nw > 0 { 2 * nw + 3 } else { 0 };
                    let at = if side && nw > 0 { nw + 1 } else { at };
                    let say = blanks(at)
                        .and(faint(if open { "▾ " } else { "▸ " }))
                        .and(dim(&plural(n, "line")));
                    let l = if side {
                        pad.clone()
                            .and(edge())
                            .and(cell(say, lw, Color::Default))
                            .and(edge())
                            .and(cell(Txt::new(), rw, Color::Default))
                            .and(edge())
                    } else {
                        pad.clone()
                            .and(edge())
                            .and(cell(say, inner, Color::Default))
                            .and(edge())
                    };
                    self.add_wide(&fold(k), l);
                }
                Row::Gap(n) => {
                    let at = if nw > 0 { nw } else { 0 };
                    let mut say = blanks(at).and(faint("⋯"));
                    if n > 0 {
                        say.push(faint(&format!(" {}", plural(n, "line"))));
                    }
                    let l = if side {
                        pad.clone()
                            .and(edge())
                            .and(cell(say, lw, Color::Default))
                            .and(edge())
                            .and(cell(Txt::new(), rw, Color::Default))
                            .and(edge())
                    } else {
                        pad.clone()
                            .and(edge())
                            .and(cell(say, inner, Color::Default))
                            .and(edge())
                    };
                    self.add_wide("", l);
                }
            }
        }
        // A long change: its first rows, and the rest a row away.
        if total > b.cap && (!whole || (opened && !b.whole)) {
            let say = if opened {
                faint("▾ ").and(dim("show less"))
            } else {
                faint("▸ ").and(dim("show all")).and(faint(&format!(
                    " · {} more line{}",
                    total - b.cap,
                    if total - b.cap == 1 { "" } else { "s" }
                )))
            };
            let l = pad
                .clone()
                .and(edge())
                .and(cell(say, inner, Color::Default))
                .and(edge());
            self.add_wide(b.all_ref, l);
        }
        let bottom = pad.and(faint(&format!("╰{}╯", "─".repeat(w.saturating_sub(2)))));
        self.add_wide("", bottom);
        true
    }
}
