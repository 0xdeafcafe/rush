//! The light markdown the agent's words are drawn with: headings, lists,
//! tables, fenced code, and inline **bold**, `code` and links. Port of
//! internal/convo/render.go:1513-1650 (markdown, listItem, indentOf,
//! strong), :1673 (code), :1880-2100 (inline, pairs, mdLinks, imageChip,
//! links, urlLen) and table.go.

use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::{Color, Style};

use super::code::{HlState, highlight, lang_for};
use super::draw::Drawer;
use super::style::{
    BG_ADD, BG_DEL, BG_WELL, C_BLUE, C_DIM, C_SUB, C_TEXT, C_WHITE, dim, faint, minus_sign, paint,
    plus_sign, strong,
};
use super::txt::{Txt, blanks, wrap, wrap_at};
use super::udiff::{expand_tabs, parse_diff};

/// A run of inline text: its words and whether it's marked up.
struct Run {
    text: String,
    style: Option<Style>,
}

/// Styles **bold** and `code`, and draws links, over base colour
/// (render.go:1880).
pub(crate) fn inline(s: &str, base: Style) -> Txt {
    if s.is_empty() {
        return Txt::new();
    }
    if !s.contains(['*', '`', 'h', '[', '@']) {
        return Txt::raw(s);
    }
    let code = if base == C_SUB || base == C_DIM {
        C_TEXT
    } else {
        C_WHITE
    };
    let mut runs = vec![Run {
        text: s.to_string(),
        style: None,
    }];
    runs = pairs(runs, "**", base.bold());
    runs = pairs(runs, "`", code);
    md_links(&runs, base)
}

/// Text between two delimiters in style st, the delimiters dropped, where
/// the text holds no delimiter character (render.go:1997).
fn pairs(runs: Vec<Run>, delim: &str, st: Style) -> Vec<Run> {
    let d = delim.as_bytes()[0] as char;
    let mut out = Vec::new();
    for r in runs {
        if r.style.is_some() && r.style != Some(st.bold()) && delim == "**" {
            out.push(r);
            continue;
        }
        let s = r.text;
        let mut last = 0;
        let mut p = 0;
        let mut any = false;
        while let Some(i) = s[p..].find(delim) {
            let i = p + i;
            let from = i + delim.len();
            let j = s[from..].find(d);
            match j {
                Some(j) if j > 0 && s[from + j..].starts_with(delim) => {
                    if i > last {
                        out.push(Run {
                            text: s[last..i].to_string(),
                            style: r.style,
                        });
                    }
                    out.push(Run {
                        text: s[from..from + j].to_string(),
                        style: Some(st),
                    });
                    any = true;
                    last = from + j + delim.len();
                    p = last;
                }
                _ => p = i + 1,
            }
        }
        if !any {
            out.push(Run {
                text: s,
                style: r.style,
            });
            continue;
        }
        if last < s.len() {
            out.push(Run {
                text: s[last..].to_string(),
                style: r.style,
            });
        }
    }
    out
}

/// s drawn as a link to url (render.go:1891 link).
fn link(s: &str, url: &str) -> Txt {
    paint(C_BLUE.underline().linked(url), s)
}

/// t with every run of it linked to url (Go's OSC 8 around it).
pub(crate) fn linked(mut t: Txt, url: &str) -> Txt {
    let l = rush_core::text::Link::to(url);
    for s in &mut t.0 {
        s.style.link = l;
    }
    t
}

/// Each markdown image as a chip, each [text](url) as its text, and the
/// bare URLs between them linked (render.go:1906). A link's words may
/// span runs already styled, `code` in them keeping its colour, underlined
/// as a link (render.go:1941, 34dfd2e7).
fn md_links(runs: &[Run], base: Style) -> Txt {
    let s: String = runs.iter().map(|r| r.text.as_str()).collect();
    // Each run's start in s, and its style.
    let mut at = Vec::with_capacity(runs.len());
    let mut n = 0;
    for r in runs {
        at.push((n, r.style));
        n += r.text.len();
    }
    // The pieces of s[from..to] by run: f(text, style) for each.
    let each = |from: usize, to: usize, f: &mut dyn FnMut(&str, Option<Style>)| {
        for (k, &(start, st)) in at.iter().enumerate() {
            let end = at.get(k + 1).map_or(s.len(), |x| x.0);
            let (a, b) = (start.max(from), end.min(to));
            if a < b {
                f(&s[a..b], st);
            }
        }
    };
    let text = |out: &mut Txt, from: usize, to: usize| {
        each(from, to, &mut |t, st| match st {
            Some(st) => out.push(Txt::styled(t, st)),
            None => out.push(links(t, base)),
        });
    };
    let mut out = Txt::new();
    let mut last = 0;
    let mut p = 0;
    while let Some(i) = s[p..].find("](") {
        let mid = p + i;
        let open = s[last..mid].rfind('[');
        let end = s[mid + 2..].find(')');
        let (Some(open), Some(end)) = (open, end) else {
            p = mid + 2;
            continue;
        };
        let open = open + last;
        let end = end + mid + 2;
        let (label, target) = (&s[open + 1..mid], s[mid + 2..end].trim());
        if label.contains(['[', ']']) || target.is_empty() || target.contains([' ', '\t']) {
            p = mid + 2;
            continue;
        }
        let image = open > last && s.as_bytes()[open - 1] == b'!';
        let start = if image { open - 1 } else { open };
        text(&mut out, last, start);
        if image {
            let name = target.rsplit('/').next().unwrap_or(target);
            let alt = if label.is_empty() { name } else { label };
            let mut chip = paint(C_BLUE, "▣ ");
            chip.push(paint(C_TEXT, alt));
            if name != alt {
                chip.push(paint(C_DIM, &format!(" {name}")));
            }
            // It opens the image when clicked (imageChip).
            match link_target(target) {
                Some(u) => out.push(linked(chip, &u)),
                None => out.push(chip),
            }
        } else {
            let u = link_target(target);
            each(open + 1, mid, &mut |t, st| {
                out.push(match (st, u.as_deref()) {
                    (Some(st), Some(u)) => Txt::styled(t, st.underline().linked(u)),
                    (Some(st), None) => Txt::styled(t, st),
                    (None, Some(u)) => link(t, u),
                    (None, None) => paint(C_BLUE, t),
                })
            });
            if let Some(u) = &u {
                out.push(Txt(crate::linkinfo::link_after(u).spans));
            }
        }
        last = end + 1;
        p = end + 1;
    }
    text(&mut out, last, s.len());
    out
}

/// Where a markdown link (or a file a step names) opens: a web address
/// as it is, a path as a file:// URL; None for anything else
/// (render.go:2031 linkTarget).
pub(crate) fn link_target(t: &str) -> Option<String> {
    if t.starts_with("http://") || t.starts_with("https://") || t.starts_with("file://") {
        return Some(t.to_string());
    }
    let home;
    let t = match t.strip_prefix("~/") {
        Some(rest) => {
            home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
            format!("{}/{rest}", home.trim_end_matches('/'))
        }
        None => t.to_string(),
    };
    if !t.starts_with('/') {
        return None;
    }
    Some(format!("file://{}", escape_path(&t)))
}

/// A path as a URL's, escaped as Go's url.URL.String escapes one.
fn escape_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b','
                    | b'/'
                    | b':'
                    | b';'
                    | b'='
                    | b'@'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Each URL linked, with what rush knows of it after it (render.go:2071,
/// linkAfter).
fn links(s: &str, _base: Style) -> Txt {
    let mut out = Txt::new();
    let mut last = 0;
    let mut p = 0;
    let b = s.as_bytes();
    while let Some(i) = s[p..].find("http") {
        let i = p + i;
        let mut j = i + 4;
        if j < b.len() && b[j] == b's' && s[j + 1..].starts_with("://") {
            j += 4;
        } else if s[j..].starts_with("://") {
            j += 3;
        } else {
            p = i + 1;
            continue;
        }
        let end = j + url_len(&s[j..]);
        if end == j {
            p = i + 1;
            continue;
        }
        out.push(Txt::raw(&s[last..i]));
        out.push(link(&s[i..end], &s[i..end]));
        out.push(Txt(crate::linkinfo::link_after(&s[i..end]).spans));
        last = end;
        p = end;
    }
    out.push(Txt::raw(&s[last..]));
    out
}

/// How much of s, just past a URL's scheme, is the URL (render.go:2078).
fn url_len(s: &str) -> usize {
    let b = s.as_bytes();
    let (mut angle, mut paren, mut end) = (0, 0, 0);
    while end < b.len()
        && !matches!(
            b[end],
            b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b']' | b'"' | b'\'' | b'`' | b','
        )
    {
        match b[end] {
            b'<' => angle += 1,
            b'(' => paren += 1,
            b'>' => {
                if angle == 0 {
                    break;
                }
                angle -= 1;
            }
            b')' => {
                if paren == 0 {
                    break;
                }
                paren -= 1;
            }
            _ => {}
        }
        end += 1;
    }
    s[..end].trim_end_matches(['.', ':', ';', '!', '?']).len()
}

/// An unordered item's bullet by depth.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

/// A list item's marker ("" for a bullet) and text.
fn list_item(trim: &str) -> Option<(&str, &str)> {
    let b = trim.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        return Some(("", trim[2..].trim()));
    }
    if !b.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+[.)])\s+(.*)$").unwrap());
    let m = NUMBERED.captures(trim)?;
    let (a, b) = (m.get(1)?, m.get(2)?);
    Some((&trim[a.start()..a.end()], &trim[b.start()..b.end()]))
}

/// A line's indent in columns, a tab to the next stop of four.
fn indent_of(s: &str) -> usize {
    let mut n = 0;
    for c in s.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += 4 - n % 4,
            _ => return n,
        }
    }
    n
}

/// An open list item: its marker's column in the source and its text's as
/// drawn.
struct ListLevel {
    col: usize,
    text: usize,
}

impl Drawer<'_> {
    /// Text indent columns in, in colour c, with light markdown; keep_blank
    /// keeps its empty lines (render.go:1513).
    pub(crate) fn markdown(&mut self, s: &str, indent: usize, c: Style, keep_blank: bool) {
        let w = (self.cw as isize - indent as isize - 1).max(0) as usize;
        let w = w.min(self.o.prose_cap());
        let pad = self.sp(indent - 1);
        let lines: Vec<&str> = s.split('\n').collect();
        let mut items: Vec<ListLevel> = Vec::new();
        let mut li = 0;
        while li < lines.len() {
            let trim = lines[li].trim();
            if trim.starts_with('|') {
                let mut end = li;
                while end < lines.len() && lines[end].trim().starts_with('|') {
                    end += 1;
                }
                if end - li >= 2 {
                    self.gap();
                    let tw = (self.cw as isize - indent as isize - 1).max(0) as usize;
                    self.table(&lines[li..end], pad.clone(), tw.min(self.o.row_cap()), c);
                    self.gap();
                    li = end;
                    continue;
                }
            }
            if let Some(tag) = trim.strip_prefix("```") {
                let mut end = li + 1;
                while end < lines.len() && !lines[end].trim().starts_with("```") {
                    end += 1;
                }
                let body = &lines[li + 1..end.min(lines.len())];
                self.code(body, tag.trim(), pad.clone());
                li = end + 1;
                continue;
            }
            if trim.is_empty() {
                if keep_blank {
                    self.gap();
                }
                li += 1;
                continue;
            }
            if trim.starts_with('#') {
                items.clear();
                self.gap();
                let h = trim.trim_start_matches('#').trim();
                self.add(
                    "",
                    Color::Default,
                    pad.clone().and(paint(strong(c), h)),
                    Txt::new(),
                );
                li += 1;
                continue;
            }
            let col = indent_of(lines[li]);
            let (marker, body, is_item) = match list_item(trim) {
                Some((m, b)) => (m, b, true),
                None => ("", trim, false),
            };
            let mut lead = marker.to_string();
            let mut ind = 0;
            if is_item {
                while items.last().is_some_and(|x| x.col >= col) {
                    items.pop();
                }
                let depth = items.len();
                if marker.is_empty() {
                    lead = BULLETS[depth % BULLETS.len()].to_string();
                }
                items.push(ListLevel {
                    col,
                    text: 2 * depth + lead.chars().count() + 1,
                });
                ind = 2 * depth;
            } else if col > 0 && !items.is_empty() {
                while items.len() > 1 && items.last().is_some_and(|x| x.col >= col) {
                    items.pop();
                }
                ind = items.last().unwrap().text;
            } else {
                items.clear();
            }
            let lead_t = if lead.is_empty() {
                Txt::new()
            } else {
                dim(&lead).and_raw(" ")
            };
            let lead_w = lead_t.width();
            let body_t = inline(body, c).over(c);
            let fit = 8.max(w.saturating_sub(ind + lead_w));
            for (k, r) in wrap(&body_t, fit).into_iter().enumerate() {
                let r = if k > 0 && !lead.is_empty() {
                    blanks(lead_w).and(r)
                } else {
                    lead_t.clone().and(r)
                };
                self.add(
                    "",
                    Color::Default,
                    pad.clone().and(blanks(ind)).and(r),
                    Txt::new(),
                );
                if k > 0 {
                    self.wrapped();
                }
            }
            li += 1;
        }
    }

    /// A fenced code block highlighted in its fence's language; a diff
    /// block colours its lines (render.go:1673).
    fn code(&mut self, lines: &[&str], tag: &str, pad: Txt) {
        let cw = self.cw;
        self.widen(lines, 7);
        let w = (self.cw as isize - 7).max(0) as usize;
        let mut lg = lang_for(tag);
        if lg.is_none()
            && let Some(f) = tag.split_whitespace().next()
        {
            lg = lang_for(f);
        }
        let is_diff = tag == "diff" || tag == "patch";
        let mut ud = if is_diff {
            parse_diff(lines, |_| true)
        } else {
            None
        };
        let mut st = HlState::default();
        let lead = pad.clone().and_raw(" ");
        for (i, l) in lines.iter().enumerate() {
            if let Some(u) = ud.as_mut()
                && u.line(self, lead.clone(), w, None, lines, i)
            {
                continue;
            }
            let l = expand_tabs(l);
            if is_diff && l.starts_with('+') && !l.starts_with("+++") {
                self.add_rows(
                    BG_ADD,
                    lead.clone(),
                    plus_sign(),
                    paint(C_TEXT, &l[1..]),
                    w.saturating_sub(1),
                    0,
                );
            } else if is_diff && l.starts_with('-') && !l.starts_with("---") {
                self.add_rows(
                    BG_DEL,
                    lead.clone(),
                    minus_sign(),
                    paint(C_TEXT, &l[1..]),
                    w.saturating_sub(1),
                    0,
                );
            } else if is_diff && l.starts_with("@@") {
                self.add_rows(BG_WELL, lead.clone(), Txt::new(), paint(C_BLUE, &l), w, 0);
            } else {
                let h = highlight(lg, &mut st, &l, C_SUB, None);
                self.add_rows(BG_WELL, lead.clone(), Txt::new(), h, w, 0);
            }
        }
        self.cw = cw;
    }

    /// A markdown table in a frame of faint lines; too wide for any frame,
    /// each row's cells one under another (table.go:57).
    fn table(&mut self, rows: &[&str], pad: Txt, w: usize, col: Style) {
        let (cells, head, align) = table_cells(rows);
        let cols = cells.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let styled: Vec<Vec<Txt>> = cells
            .iter()
            .enumerate()
            .map(|(ri, r)| {
                (0..cols)
                    .map(|i| {
                        let v = r.get(i).map_or("", String::as_str);
                        let ink = if Some(ri) == head {
                            strong(col)
                        } else if i == 0 {
                            col.bold()
                        } else {
                            col
                        };
                        inline(v, ink).over(ink)
                    })
                    .collect()
            })
            .collect();
        let mut width = vec![0usize; cols];
        let mut least = vec![0usize; cols];
        for r in &styled {
            for (i, c) in r.iter().enumerate() {
                width[i] = width[i].max(c.width());
                for word in c.plain().split_whitespace() {
                    least[i] = least[i].max(super::txt::width(word).min(14));
                }
            }
        }
        for i in 0..cols {
            least[i] = least[i].min(width[i]).max(1);
        }
        let chrome = 3 * cols + 1;
        if least.iter().sum::<usize>() + chrome > w {
            self.table_records(&styled, head, pad, w);
            return;
        }
        let mut total = width.iter().sum::<usize>() + chrome;
        while total > w {
            let mut i: Option<usize> = None;
            for j in 0..cols {
                if width[j] > least[j] && i.is_none_or(|i| width[j] > width[i]) {
                    i = Some(j);
                }
            }
            let Some(i) = i else { break };
            width[i] -= 1;
            total -= 1;
        }
        let mut parts: Vec<Vec<Vec<Txt>>> = Vec::new();
        let mut tall = false;
        for (ri, r) in styled.iter().enumerate() {
            let mut row = Vec::new();
            for (i, c) in r.iter().enumerate() {
                let p = cell_rows(c, width[i]);
                if Some(ri) != head && p.len() > 1 {
                    tall = true;
                }
                row.push(p);
            }
            parts.push(row);
        }
        let rule = |d: &mut Self, l: &str, m: &str, r: &str, fill: &str| {
            let mut b = String::from(l);
            for (i, cw) in width.iter().enumerate() {
                if i > 0 {
                    b.push_str(m);
                }
                b.push_str(&fill.repeat(cw + 2));
            }
            b.push_str(r);
            d.add("", Color::Default, pad.clone().and(faint(&b)), Txt::new());
        };
        rule(self, "╭", "┬", "╮", "─");
        #[allow(clippy::needless_range_loop)]
        for ri in 0..styled.len() {
            if ri > 0 && Some(ri - 1) == head {
                rule(self, "╞", "╪", "╡", "═");
            } else if ri > 0 && tall {
                rule(self, "├", "┼", "┤", "─");
            }
            let height = parts[ri].iter().map(Vec::len).max().unwrap_or(1).max(1);
            for line in 0..height {
                let mut b = faint("│");
                for i in 0..cols {
                    let v = parts[ri][i].get(line).cloned().unwrap_or_default();
                    let gap = width[i].saturating_sub(v.width());
                    let a = if Some(ri) != head {
                        align.get(i).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    match a {
                        1 => b = b.and_raw(" ").and(blanks(gap)).and(v).and_raw(" "),
                        2 => {
                            b = b
                                .and_raw(" ")
                                .and(blanks(gap / 2))
                                .and(v)
                                .and(blanks(gap - gap / 2))
                                .and_raw(" ")
                        }
                        _ => b = b.and_raw(" ").and(v).and(blanks(gap)).and_raw(" "),
                    }
                    b.push(faint("│"));
                }
                self.add("", Color::Default, pad.clone().and(b), Txt::new());
            }
        }
        rule(self, "╰", "┴", "╯", "─");
    }

    fn table_records(&mut self, styled: &[Vec<Txt>], head: Option<usize>, pad: Txt, w: usize) {
        let mut names: Vec<String> = Vec::new();
        let mut name_w = 0;
        if let Some(h) = head {
            for c in &styled[h] {
                let n = c.plain().trim().to_string();
                name_w = name_w.max(super::txt::width(&n));
                names.push(n);
            }
        }
        let lined = name_w <= w / 3;
        let mut first = true;
        for (ri, r) in styled.iter().enumerate() {
            if Some(ri) == head {
                continue;
            }
            if !first {
                self.add(
                    "",
                    Color::Default,
                    pad.clone().and(faint(&"─".repeat(w.min(12)))),
                    Txt::new(),
                );
            }
            first = false;
            if head.is_none() {
                let mut j = Txt::new();
                for (i, c) in r.iter().enumerate() {
                    if i > 0 {
                        j.push(faint(" · "));
                    }
                    j.push(c.clone());
                }
                for part in wrap(&j, w.max(1)) {
                    self.add("", Color::Default, pad.clone().and(part), Txt::new());
                }
                continue;
            }
            for (i, c) in r.iter().enumerate() {
                let name = names.get(i).map_or("", String::as_str);
                let (mut lead, hang) = if lined {
                    (
                        dim(name).and(blanks(name_w - super::txt::width(name) + 2)),
                        blanks(name_w + 2),
                    )
                } else if !name.is_empty() {
                    (dim(name).and_raw(" "), Txt::raw("  "))
                } else {
                    (Txt::new(), Txt::raw("  "))
                };
                let rows = if lined {
                    wrap(c, (w.saturating_sub(hang.width())).max(1))
                } else {
                    let rs = wrap(&lead.clone().and(c.clone()), w.saturating_sub(2).max(1));
                    lead = Txt::new();
                    rs
                };
                for (j, part) in rows.into_iter().enumerate() {
                    let part = if j == 0 {
                        lead.clone().and(part)
                    } else {
                        hang.clone().and(part)
                    };
                    self.add("", Color::Default, pad.clone().and(part), Txt::new());
                    if j > 0 {
                        self.wrapped();
                    }
                }
            }
        }
    }
}

/// A markdown table's rows split into cells: the header row's index and
/// each column's alignment (0 left, 1 right, 2 centre), as its rule says.
fn table_cells(rows: &[&str]) -> (Vec<Vec<String>>, Option<usize>, Vec<u8>) {
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut head = None;
    let mut align = Vec::new();
    let sep = |p: &str| {
        let q = p.strip_prefix(':').unwrap_or(p);
        let q = q.strip_suffix(':').unwrap_or(q);
        q.len() >= 2 && q.bytes().all(|c| c == b'-')
    };
    for r in rows {
        let r = r.trim();
        let r = r.strip_prefix('|').unwrap_or(r);
        let r = r.strip_suffix('|').unwrap_or(r);
        let parts: Vec<String> = r.split('|').map(|p| p.trim().to_string()).collect();
        if parts.iter().all(|p| sep(p)) && head.is_none() {
            head = Some(cells.len().wrapping_sub(1)).filter(|h| *h != usize::MAX);
            if head.is_none() {
                head = None;
            }
            for p in &parts {
                align.push(if p.starts_with(':') && p.ends_with(':') {
                    2
                } else if p.ends_with(':') {
                    1
                } else {
                    0
                });
            }
            continue;
        }
        cells.push(parts);
    }
    (cells, head, align)
}

/// A styled cell wrapped to w, each row's trailing spaces dropped.
fn cell_rows(s: &Txt, w: usize) -> Vec<Txt> {
    if s.width() <= w {
        return vec![s.clone()];
    }
    let mut rows = wrap_at(s, w.max(1), "");
    while rows.len() > 1 && rows.last().is_some_and(Txt::is_blank) {
        rows.pop();
    }
    for r in &mut rows {
        while let Some(last) = r.0.last_mut() {
            let t = last.text.trim_end_matches(' ').len();
            if t == 0 {
                r.0.pop();
                continue;
            }
            last.text.truncate(t);
            break;
        }
    }
    rows
}

/// Draws markdown on its own, w wide, for showing outside a conversation:
/// a pull request's body, say (markdown.go:7).
pub fn markdown_lines(s: &str, w: usize) -> Vec<rush_core::text::Line> {
    let sess = crate::model::Session::new();
    let turn = crate::model::Turn::default();
    let o = super::Options {
        width: w,
        no_rule: true,
        ..super::Options::default()
    };
    let p = super::shell::Places::default();
    let mut d = Drawer::new(&sess, &turn, &o, &p);
    d.cw = w;
    let s = s.replace("\r\n", "\n");
    d.markdown(s.trim_end_matches([' ', '\t', '\n']), 1, C_TEXT, true);
    let mut ls = std::mem::take(&mut d.lines);
    super::draw::squeezed(&mut ls);
    ls.into_iter().map(|l| l.line).collect()
}

/// An answer as a turn draws it, w wide: a column further in than
/// [`markdown_lines`], and a row clear of what's above (convo
/// render.go:1647 Session.Answer). The /btw panel and the guide draw
/// theirs so.
pub fn answer_lines(s: &str, w: usize) -> Vec<rush_core::text::Line> {
    let sess = crate::model::Session::new();
    let turn = crate::model::Turn::default();
    let o = super::Options {
        width: w,
        ..super::Options::default()
    };
    let p = super::shell::Places::default();
    let mut d = Drawer::new(&sess, &turn, &o, &p);
    d.cw = w.min(super::CAP_ROW);
    d.answer(s);
    std::mem::take(&mut d.lines)
        .into_iter()
        .map(|l| l.line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_markup() {
        let t = inline(
            "the **renderer** caches `Render` see https://x.dev.",
            C_TEXT,
        );
        assert_eq!(t.plain(), "the renderer caches Render see https://x.dev.");
        let bold = t.0.iter().find(|s| s.text == "renderer").unwrap();
        assert!(bold.style.mods.contains(rush_core::text::Mods::BOLD));
        let url = t.0.iter().find(|s| s.text == "https://x.dev").unwrap();
        assert_eq!(url.style.fg, C_BLUE.fg);
    }

    #[test]
    fn links_and_images() {
        assert_eq!(
            inline("see [the docs](https://x.dev/a) now", C_TEXT).plain(),
            "see the docs now"
        );
        assert_eq!(
            inline("![shot](/tmp/a.png)", C_TEXT).plain(),
            "▣ shot a.png"
        );
    }

    /// Each link carries where it goes, as Go's OSC 8 does: a markdown
    /// link its target, a bare URL itself, an image chip its file, a path
    /// as a file:// URL; a word that opens nothing carries none
    /// (render.go:1891, :1958, :1998, :2031).
    #[test]
    fn links_carry_their_targets() {
        let url = |t: &Txt, word: &str| {
            t.0.iter()
                .find(|s| s.text.contains(word))
                .and_then(|s| s.style.link.url())
                .map(|u| u.to_string())
        };
        let t = inline(
            "see [the docs](https://x.dev/a), https://y.dev and [here](notes.md)",
            C_TEXT,
        );
        assert_eq!(url(&t, "the docs").as_deref(), Some("https://x.dev/a"));
        assert_eq!(url(&t, "https://y.dev").as_deref(), Some("https://y.dev"));
        assert_eq!(url(&t, "here"), None, "a relative target opens nothing");
        assert_eq!(url(&t, "see"), None);
        let t = inline("![shot](/tmp/my shots/a.png) [b](/tmp/a b.md)", C_TEXT);
        assert_eq!(url(&t, "▣"), None, "a target with a space isn't one");
        let t = inline("![shot](/tmp/a%.png)", C_TEXT);
        assert_eq!(url(&t, "shot").as_deref(), Some("file:///tmp/a%25.png"));
        assert_eq!(url(&t, "▣").as_deref(), Some("file:///tmp/a%25.png"));
    }

    /// Rows past the gutter's column, a blank row at the end dropped.
    fn rows(ls: &[rush_core::text::Line]) -> Vec<String> {
        let mut t: Vec<String> = ls
            .iter()
            .map(|l| l.plain().trim_end().chars().skip(1).collect())
            .collect();
        while t.last().is_some_and(String::is_empty) {
            t.pop();
        }
        t
    }

    #[test]
    fn tables_frame() {
        let ls = markdown_lines("| a | b |\n|---|--:|\n| x | 12 |", 40);
        let t = rows(&ls);
        assert_eq!(
            t,
            [
                "╭───┬────╮",
                "│ a │ b  │",
                "╞═══╪════╡",
                "│ x │ 12 │",
                "╰───┴────╯"
            ]
        );
    }

    #[test]
    fn lists_nest() {
        let ls = markdown_lines("- one\n  - two\n1. three", 40);
        let t = rows(&ls);
        assert_eq!(t, ["• one", "  ◦ two", "1. three"]);
    }
}
