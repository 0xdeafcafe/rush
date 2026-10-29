//! The Projects page's rows and the boxes they're drawn in. Go:
//! internal/ui/workstreams.go (workRow :155, pickable :170, pickRow :248),
//! projects.go (pbox :222, psection :243, thead :252, shade :261,
//! drawRows :268, sideBySide :285, pad :302), deletesheet.go (kindMark
//! :368, kindLine :382), tempwork.go:89 (disk), rushmode.go:2175
//! (spread), keys.go:1266 (trimCmd).

use std::time::SystemTime;

use rush_core::text::{Color, Line, Span, width};
use rush_fleet::machine::Role;
use rush_fleet::worktree::Worktree;
use rush_gh::ghpr::PR;

use crate::style::*;

/// Row kinds on the Projects page and the Delete sheet, each with its own
/// mark and colour.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Project,
    Folder,
    Worktree,
    Temp,
}

/// A kind's mark, in its colour.
pub(crate) fn kind_mark(k: Kind) -> Span {
    match k {
        Kind::Project => sp("◆", ORANGE),
        Kind::Folder => sp("◇", SUB),
        Kind::Worktree => sp("⎇", BLUE),
        Kind::Temp => sp("◌", YELLOW),
    }
}

/// An item's head on the Delete sheet: its mark and name, what it is and
/// how it goes, and its size at the right.
pub(crate) fn kind_line(k: Kind, name: &str, what: &str, size: &str, w: usize) -> Line {
    let head = line([
        kind_mark(k),
        plain(" "),
        spb(name, TEXT),
        plain("  "),
        sp(what, DIM),
    ]);
    let mut l = fit(head, w.saturating_sub(8));
    l.spans.push(plain(right(size, 8)));
    l
}

/// Bytes of disk the way mem says memory.
pub(crate) fn disk(n: i64) -> String {
    if n >= 1 << 30 {
        format!("{:.1}G", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{}M", n >> 20)
    } else if n > 0 {
        format!("{}K", n >> 10)
    } else {
        "–".into()
    }
}

pub(crate) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A command with home as ~, cut to n cells.
pub(crate) fn trim_cmd(s: &str, n: usize) -> String {
    let mut s = s.to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if !home.is_empty() {
            s = s.replace(home.as_ref(), "~");
        }
    }
    if width(&s) > n {
        return rush_draw::truncate(&s, n, "…").into_owned();
    }
    s
}

/// left and right on one line w wide, or left alone when there's no room.
pub(crate) fn spread(left: Line, right: Line, w: usize) -> Line {
    let gap = w as isize - left.width() as isize - right.width() as isize;
    if gap < 2 {
        return fit(left, w);
    }
    let mut l = left;
    l.spans.push(plain(blanks(gap as usize)));
    join(&mut l, right);
    l
}

/// A process in the System section.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProcRow {
    pub pid: i32,
    pub depth: usize,
    pub n: usize,
    pub label: String,
    pub cmd: String,
    pub mem: u64,
    pub cpu: f64,
    pub start: SystemTime,
    pub role: Role,
    /// A Claude process of no agent, not an orphan's child.
    pub other: bool,
}

/// A thing on disk a row stands for, which x deletes on its own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PathRow {
    pub path: String,
    /// The session whose temp folder it's in.
    pub temp: String,
    /// The project whose agent scratch folder it is.
    pub scratch: String,
}

/// A pull request on the page, and the repository it's in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PrRow {
    pub root: String,
    pub pr: PR,
}

/// What a row stands for.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum What {
    #[default]
    Text,
    /// A project's head, by its key.
    Proj(String),
    /// An agent, by key.
    Agent(String),
    Wt(Box<Worktree>),
    Proc(Box<ProcRow>),
    /// The /tmp row.
    Tmp,
    /// A finished agent's temp work, by key.
    Temp(String),
    /// Temporary, System or Pull requests in the list.
    Pane,
    Path(PathRow),
    Pr(Box<PrRow>),
}

/// One line of the Projects page: something to pick, or text.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WorkRow {
    pub id: String,
    pub line: Line,
    pub what: What,
    /// The project an agent or worktree row is inside.
    pub owner: String,
}

impl WorkRow {
    pub fn text(l: Line) -> WorkRow {
        WorkRow {
            line: l,
            ..WorkRow::default()
        }
    }

    pub fn blank() -> WorkRow {
        WorkRow::default()
    }

    pub fn pickable(&self) -> bool {
        !self.id.is_empty() && self.what != What::Text
    }
}

/// The row picked, by its id at sel, following it if rows moved, else the
/// nearest row that can be picked to pos.
pub(crate) fn pick_row(rows: &[WorkRow], pos: &mut usize, sel: &mut String) -> Option<usize> {
    if *pos < rows.len() && rows[*pos].id == *sel && rows[*pos].pickable() {
        return Some(*pos);
    }
    if let Some(i) = rows.iter().position(|r| r.id == *sel && r.pickable()) {
        *pos = i;
        return Some(i);
    }
    for d in 0..rows.len() as isize {
        for i in [*pos as isize + d, *pos as isize - d] {
            if i >= 0 && (i as usize) < rows.len() && rows[i as usize].pickable() {
                *pos = i as usize;
                *sel = rows[i as usize].id.clone();
                return Some(i as usize);
            }
        }
    }
    None
}

/// Lines in a rounded box w wide, its title and meta in the top edge; lit
/// when it has the cursor. Each line is fitted inside.
pub(crate) fn pbox(title: Line, meta: Line, body: Vec<Line>, w: usize, lit: bool) -> Vec<Line> {
    let col: Color = if lit { ORANGE } else { EDGE };
    let mut head = line([sp("╭─ ", col)]);
    join(&mut head, title);
    head.spans.push(plain(" "));
    let mut tail = Line::new();
    if meta.width() > 0 {
        tail.spans.push(plain(" "));
        join(&mut tail, meta);
        tail.spans.push(plain(" "));
    }
    let fill = 1.max(w as isize - head.width() as isize - tail.width() as isize - 2) as usize;
    let mut top = head;
    top.spans.push(sp("─".repeat(fill), col));
    join(&mut top, tail);
    top.spans.push(sp("─╮", col));
    let mut out = vec![fit(top, w)];
    for l in body {
        let mut r = line([sp("│", col), plain(" ")]);
        join(&mut r, fit(l, w.saturating_sub(4)));
        r.spans.push(plain(" "));
        r.spans.push(sp("│", col));
        out.push(r);
    }
    out.push(line([sp(
        format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
        col,
    )]));
    out
}

/// The edge of a box without the cursor (Go's cEdge).
pub(crate) const EDGE: Color = Color::Rgb(79, 73, 67);

/// A part of a box's head: a label in capitals, a count, and a hairline
/// to the edge.
pub(crate) fn psection(label: &str, meta: &str, w: usize) -> Line {
    let mut l = line([spb(label.to_uppercase(), SUB)]);
    if !meta.is_empty() {
        l.spans.push(plain("  "));
        l.spans.push(sp(meta, DIM));
    }
    let n = w.saturating_sub(l.width() + 1);
    l.spans.push(plain(" "));
    l.spans.push(sp("─".repeat(n), FAINT));
    l
}

/// A table's column names, each fitted to its width.
pub(crate) fn thead(cols: &[(&str, usize)]) -> Line {
    let s: String = cols.iter().map(|(n, w)| fit_str(n, *w)).collect();
    line([sp(s, FAINT)])
}

/// A whole line on bg, w wide.
pub(crate) fn shade(l: Line, bg: Color, w: usize) -> Line {
    on_ground(l, w, bg)
}

/// rows drawn w wide, the one at pick on the cursor: selected when the
/// cursor is here, faintly otherwise.
pub(crate) fn draw_rows(rows: &[WorkRow], pick: Option<usize>, w: usize, here: bool) -> Vec<Line> {
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            if Some(i) == pick {
                shade(r.line.clone(), if here { SEL_BG } else { HOVER_BG }, w)
            } else {
                fit(r.line.clone(), w)
            }
        })
        .collect()
}

/// Two boxes of lines side by side, the shorter padded with blanks.
pub(crate) fn side_by_side(l: Vec<Line>, r: Vec<Line>, lw: usize) -> Vec<Line> {
    let n = l.len().max(r.len());
    let mut l = l.into_iter();
    let mut r = r.into_iter();
    (0..n)
        .map(|_| {
            let mut a = fit(l.next().unwrap_or_default(), lw);
            a.spans.push(plain(" "));
            join(&mut a, r.next().unwrap_or_default());
            a
        })
        .collect()
}

/// body filled out to n lines, so boxes side by side end together.
pub(crate) fn pad_rows(mut body: Vec<Line>, n: usize) -> Vec<Line> {
    while body.len() < n {
        body.push(Line::new());
    }
    body
}

/// Spans joined into a line, with no empty ones.
pub(crate) fn ln(spans: impl IntoIterator<Item = Span>) -> Line {
    line(spans)
}

/// l then o.
pub(crate) fn cat(mut l: Line, o: Line) -> Line {
    join(&mut l, o);
    l
}

/// The parts joined with sep between.
pub(crate) fn joined(parts: Vec<Line>, sep: Span) -> Line {
    let mut out = Line::new();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.spans.push(sep.clone());
        }
        join(&mut out, p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str) -> WorkRow {
        WorkRow {
            id: id.into(),
            what: What::Pane,
            ..WorkRow::default()
        }
    }

    #[test]
    fn disk_sizes() {
        for (n, want) in [
            (0, "–"),
            (2048, "2K"),
            (300 << 20, "300M"),
            (3 << 29, "1.5G"),
        ] {
            assert_eq!(disk(n), want);
        }
    }

    #[test]
    fn picks_follow_their_row() {
        let rows = vec![WorkRow::blank(), r("a"), WorkRow::blank(), r("b")];
        let (mut pos, mut sel) = (0, "b".to_string());
        assert_eq!(pick_row(&rows, &mut pos, &mut sel), Some(3));
        let (mut pos, mut sel) = (2, "gone".to_string());
        assert_eq!(pick_row(&rows, &mut pos, &mut sel), Some(3));
        assert_eq!(sel, "b");
        assert_eq!(
            pick_row(&[WorkRow::blank()], &mut 0, &mut String::new()),
            None
        );
    }

    #[test]
    fn boxes_are_w_wide() {
        let b = pbox(
            line([plain("Projects")]),
            line([plain("2")]),
            vec![line([plain("x")])],
            40,
            true,
        );
        assert_eq!(b.len(), 3);
        for l in &b {
            assert_eq!(l.width(), 40, "{:?}", l.plain());
        }
        assert!(b[0].plain().starts_with("╭─ Projects ─"));
        assert!(b[0].plain().ends_with(" 2 ─╮"));
    }
}
