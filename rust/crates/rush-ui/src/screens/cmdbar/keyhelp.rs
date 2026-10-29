//! The keys: every key rush answers to, as you have them, over whatever is
//! on the screen. F1 opens it from anywhere, ? from the list with nothing
//! typed. Where you are comes first, then what works everywhere, then the
//! rest; typing finds a key by its keys or what it does, and enter takes
//! you to Settings, Keys, to change the one under the cursor. Port of
//! internal/ui/keyhelp.go:22-419 (keyHelp :22, openKeyHelp :45,
//! keyHelpHere :55, keyHelpTitle :69, keyHelpRebuild :81, keyHelpMatch
//! :148, keyText :159, prettyKey :167, keyHelpKey :189, step :255,
//! nextSection :266, keyHelpBox :290, keyHelpLine :366, overlayKeyHelp
//! :409). The stash's own words for its rows wait on the stash's port.

use rush_core::text::{Line, width};
use rush_keymap::{CONTEXTS, Context};

use super::draw::{Edge, drop_box, field, spread};
use super::keybind::{bound_key, key_map};
use crate::frame::editor::edit;
use crate::frame::{Out, PlaceId, Press};
use crate::model::Model;
use crate::style::*;

/// A section's heading (id and keys empty) or a key and what it does
/// (keyHelpRow).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub section: String,
    /// The action; "" for a key that's not one (#, /).
    pub id: String,
    pub keys: Vec<String>,
    pub title: String,
    /// Keys you gave it, not rush's.
    pub yours: bool,
    pub heading: bool,
}

/// The open keys guide (keyHelp).
#[derive(Default)]
pub struct KeyHelp {
    pub query: Vec<char>,
    pub pos: usize,
    /// Where it was opened from, shown first.
    pub here: Vec<Context>,
    pub rows: Vec<Row>,
    /// Index into rows, never a heading.
    pub cur: usize,
    /// First body line shown.
    pub top: usize,
    /// The cursor's section to the top, as tab moved there.
    pub snap: bool,
    /// Each row's body line as last drawn, for the mouse: (line, row).
    pub rows_at: Vec<(usize, usize)>,
}

/// The guide, kept while it's open.
#[derive(Default)]
pub struct HelpState {
    pub open: Option<KeyHelp>,
}

/// The action Settings, Keys should show on opening: the one the guide
/// was on when enter was pressed (showKey). Settings' port takes it.
#[derive(Default)]
pub struct ShowKey(pub Option<String>);

pub fn is_open(m: &Model) -> bool {
    m.kept.peek::<HelpState>().is_some_and(|s| s.open.is_some())
}

fn guide(m: &mut Model) -> Option<&mut KeyHelp> {
    m.kept::<HelpState>().open.as_mut()
}

/// Opens the guide on where the keys are now (openKeyHelp).
pub fn open(m: &mut Model) {
    let here = here(m);
    m.kept::<HelpState>().open = Some(KeyHelp {
        here,
        ..Default::default()
    });
    rebuild(m);
}

pub fn close(m: &mut Model) {
    m.kept::<HelpState>().open = None;
}

/// The contexts the keys land in now, most particular first
/// (keyHelpHere).
pub fn here(m: &Model) -> Vec<Context> {
    if m.session.is_some() && !m.on_place() {
        return vec![Context::Session];
    }
    if !m.on_place() {
        if m.ext.agents_box && !m.input.is_empty() {
            return vec![Context::Prompt, Context::List];
        }
        return vec![Context::List];
    }
    vec![Context::Pages]
}

/// What a section is called in the guide (keyHelpTitle).
pub fn title(c: Context) -> &'static str {
    match c {
        Context::List => "Agents and the Prompt",
        Context::Pages => "Projects, Settings and the Wall",
        _ => c.title(),
    }
}

/// Lays out the rows that match what's typed, keeping the cursor on the
/// action it was on when it's still there (keyHelpRebuild).
pub fn rebuild(m: &mut Model) {
    let Some(h) = m.kept.peek::<HelpState>().and_then(|s| s.open.as_ref()) else {
        return;
    };
    let was = h
        .rows
        .get(h.cur)
        .map(|r| format!("{}\0{}", r.id, r.section))
        .unwrap_or_default();
    let query: String = h.query.iter().collect();
    let q: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    let mut order = h.here.clone();
    let n_here = order.len();
    if !order.contains(&Context::Global) {
        order.push(Context::Global);
    }
    for c in CONTEXTS {
        if !order.contains(&c) {
            order.push(c);
        }
    }
    let km = key_map(m);
    let mut all = Vec::new();
    for (i, c) in order.iter().enumerate() {
        let mut section = title(*c).to_string();
        if i < n_here {
            section = format!("Here · {section}");
        }
        let mut rows = Vec::new();
        for a in km.actions() {
            if a.context != *c {
                continue;
            }
            let r = Row {
                section: section.clone(),
                id: a.id.clone(),
                keys: km.keys(&a.id).iter().map(|s| s.to_string()).collect(),
                title: a.title.clone(),
                yours: km.changed(&a.id),
                heading: false,
            };
            // A command or a plugin's with no key isn't a shortcut; it
            // shows only when you look for it, to give it one.
            if r.keys.is_empty() && a.command() && q.is_empty() {
                continue;
            }
            rows.push(r);
        }
        if *c == Context::List || *c == Context::Session {
            for (k, t) in [
                ("#", "rush's commands, typed at the start of the box"),
                (
                    "/",
                    "the agent's own commands, typed at the start of the box",
                ),
            ] {
                rows.push(Row {
                    section: section.clone(),
                    keys: vec![k.into()],
                    title: t.into(),
                    ..Default::default()
                });
            }
        }
        rows.retain(|r| matches(r, &q));
        if rows.is_empty() {
            continue;
        }
        all.push(Row {
            section,
            heading: true,
            ..Default::default()
        });
        all.extend(rows);
    }
    let h = guide(m).expect("open");
    h.rows = all;
    let mut cur = None;
    for (i, r) in h.rows.iter().enumerate() {
        if !r.heading && (cur.is_none() || format!("{}\0{}", r.id, r.section) == was) {
            cur = Some(i);
        }
    }
    h.cur = cur.unwrap_or(0);
}

/// Whether every word typed is in the row's keys, what it does or its
/// section (keyHelpMatch).
pub fn matches(r: &Row, q: &[String]) -> bool {
    let hay = format!(
        "{} {} {} {} {}",
        r.keys.join(" "),
        key_text(&r.keys),
        r.title,
        r.section,
        r.id
    )
    .to_lowercase();
    q.iter().all(|w| hay.contains(w.as_str()))
}

/// Keys as the guide writes them: ⌘ for super, ⌥ for alt on a Mac, one
/// key from the next with a dot (keyText).
pub fn key_text(keys: &[String]) -> String {
    keys.iter()
        .map(|k| pretty_key(k))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A key as the guide writes it: arrows drawn, ⌥ and ⌘ (prettyKey).
pub fn pretty_key(k: &str) -> String {
    let parts: Vec<String> = k
        .split_whitespace()
        .map(|p| {
            let (mods, name) = match p.rfind('+') {
                Some(j) if j > 0 && j < p.len() - 1 => (&p[..=j], &p[j + 1..]),
                _ => ("", p),
            };
            let name = match name {
                "up" => "↑",
                "down" => "↓",
                "left" => "←",
                "right" => "→",
                n => n,
            };
            format!("{mods}{name}")
        })
        .collect();
    rush_convo::render::key_word(&parts.join(" ")).replace("super+", "⌘")
}

/// A key while the guide is open (keyHelpKey). Every key is its.
pub fn key(m: &mut Model, k: &Press) -> Out {
    if !is_open(m) || k.s == "ctrl+q" {
        return Out::Pass;
    }
    let s = k.s.as_str();
    let h = guide(m).expect("open");
    match s {
        "esc" | "ctrl+c" => {
            if !h.query.is_empty() {
                h.query.clear();
                h.pos = 0;
                rebuild(m);
            } else {
                close(m);
            }
        }
        "f1" => close(m),
        "up" | "ctrl+p" => h.step(-1),
        "down" | "ctrl+n" => h.step(1),
        "pgup" => (0..10).for_each(|_| h.step(-1)),
        "pgdown" => (0..10).for_each(|_| h.step(1)),
        "tab" => h.next_section(1),
        "shift+tab" => h.next_section(-1),
        "enter" => {
            let Some(id) = h
                .rows
                .get(h.cur)
                .map(|r| r.id.clone())
                .filter(|i| !i.is_empty())
            else {
                return Out::Idle;
            };
            close(m);
            m.session = None;
            m.set_place(PlaceId::Settings);
            if let Some(t) = m.page_tabs()
                && let Some(i) = t.names.iter().position(|n| n == "Keys")
            {
                m.set_page(i);
            }
            m.kept::<ShowKey>().0 = Some(id);
        }
        _ => {
            let Some((buf, pos)) = edit(&h.query, h.pos, k) else {
                return Out::Idle;
            };
            if buf.contains(&'\n') {
                return Out::Idle;
            }
            let changed = buf != h.query;
            h.query = buf;
            h.pos = pos;
            if changed {
                rebuild(m);
            }
        }
    }
    Out::Draw
}

impl KeyHelp {
    /// Moves the cursor d rows, over headings, stopping at either end
    /// (step).
    pub fn step(&mut self, d: isize) {
        let mut i = self.cur as isize + d;
        while i >= 0 && (i as usize) < self.rows.len() {
            if !self.rows[i as usize].heading {
                self.cur = i as usize;
                return;
            }
            i += d;
        }
    }

    /// Puts the cursor on the first row of the next section (or the one
    /// before), round (nextSection).
    pub fn next_section(&mut self, d: isize) {
        let starts: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.rows[i].heading && i + 1 < self.rows.len())
            .map(|i| i + 1)
            .collect();
        if starts.is_empty() {
            return;
        }
        let at = starts.iter().rposition(|&s| s <= self.cur).unwrap_or(0) as isize;
        let n = starts.len() as isize;
        self.cur = starts[((at + d) % n + n) as usize % starts.len()];
        self.snap = true;
    }
}

/// The guide's body, w wide and at most rows tall: the box you type in,
/// then the rows that fit, by section (keyHelpBox).
pub fn body(m: &mut Model, w: usize, rows: usize) -> Vec<Line> {
    let Some(h) = guide(m) else {
        return Vec::new();
    };
    let mut head = line([spb("❯ ", ORANGE)]);
    join(
        &mut head,
        field(&h.query, h.pos, "find a key by its keys or what it does"),
    );
    let n = h.rows.iter().filter(|r| !r.heading).count();
    let count = line([sp(format!("{n} key{}", if n == 1 { "" } else { "s" }), DIM)]);
    let mut out = vec![spread(head, count, w), line([sp("─".repeat(w), FAINT)])];

    // The keys line up in one column, as wide as the widest that fits.
    let kw = h
        .rows
        .iter()
        .filter(|r| !r.heading)
        .map(|r| width(&key_text(&r.keys)).min(20).min(w / 4))
        .max()
        .unwrap_or(0)
        .max(8);
    // (text, row, section)
    let mut lines: Vec<(Line, Option<usize>, String)> = Vec::new();
    for (i, r) in h.rows.iter().enumerate() {
        if r.heading {
            if i > 0 {
                lines.push((Line::new(), None, String::new()));
            }
            lines.push((rule(&r.section, "", w), None, String::new()));
            continue;
        }
        for l in row_lines(r, w, kw, i == h.cur) {
            lines.push((l, Some(i), r.section.clone()));
        }
    }
    if h.rows.is_empty() {
        lines.push((Line::new(), None, String::new()));
        lines.push((
            line([sp("  no key matches · esc clears what's typed", DIM)]),
            None,
            String::new(),
        ));
    }
    let avail = rows.saturating_sub(out.len() + 2).max(3);
    let (mut first, mut last) = (0, 0);
    for (i, l) in lines.iter().enumerate() {
        if l.1 == Some(h.cur) {
            if first == 0 || lines[first].1 != Some(h.cur) {
                first = i;
            }
            last = i;
        }
    }
    // Keep the cursor on screen, with its heading when it's near the top;
    // a section tab moved to starts at the top.
    if h.snap {
        h.top = first.saturating_sub(1);
        h.snap = false;
    }
    if first < h.top + 1 {
        h.top = first.saturating_sub(2);
    }
    if last >= h.top + avail {
        h.top = last + 1 - avail;
    }
    h.top = h.top.min(lines.len().saturating_sub(avail));
    let end = lines.len().min(h.top + avail);
    h.rows_at.clear();
    for i in h.top..end {
        let (l, row, section) = &lines[i];
        if let Some(r) = row {
            h.rows_at.push((out.len(), *r));
        }
        if i == h.top && !section.is_empty() {
            out.push(rule(section, "", w)); // the section scrolled into stays named
        } else {
            out.push(l.clone());
        }
    }
    for _ in 0..avail - (end - h.top) {
        out.push(Line::new()); // the box stays one size as you type
    }
    out.push(Line::new());
    out.push(keys_fit(
        w,
        &[
            ("↑↓", "choose"),
            ("enter", "change its keys"),
            ("tab", "next section"),
            ("esc", "close"),
        ],
    ));
    out
}

/// A row: its keys in their column, what it does beside them; the row
/// under the cursor shows all of what it does, wrapped (keyHelpLine).
pub fn row_lines(r: &Row, w: usize, kw: usize, sel: bool) -> Vec<Line> {
    let mut k = if r.keys.is_empty() {
        line([sp("no key", FAINT)])
    } else {
        // As many of its keys as fit, the first brightest.
        let first = pretty_key(&r.keys[0]);
        let mut used = width(&first);
        let mut k = line([spb(first, TEXT)]);
        for o in &r.keys[1..] {
            let p = pretty_key(o);
            let add = 3 + width(&p);
            if used + add > kw {
                break;
            }
            k.spans.push(sp(" · ", FAINT));
            k.spans.push(sp(p, SUB));
            used += add;
        }
        if used > kw {
            k = rush_draw::truncate_line(&k, kw, "…");
        }
        k
    };
    let mut lead = if r.yours {
        line([sp("•", ORANGE)])
    } else {
        line([plain(" ")])
    };
    join(&mut lead, std::mem::take(&mut k));
    let mut l = line([plain(" ")]);
    join(&mut l, lead);
    let kwidth = l.width() - 1;
    l.spans.push(plain(blanks(kw.saturating_sub(kwidth) + 1)));
    l.spans.push(plain("  "));
    let lw = l.width();
    let dw = 10.max(w.saturating_sub(lw));
    if sel {
        let mut out = Vec::new();
        for (i, t) in wrap(&r.title, dw).into_iter().enumerate() {
            let mut row = if i == 0 {
                let mut row = l.clone();
                row.spans[0] = sp("▍", ORANGE);
                row
            } else {
                line([plain(blanks(lw))])
            };
            row.spans.push(sp(t, TEXT));
            out.push(highlight(row, w));
        }
        return out;
    }
    l.spans
        .push(sp(rush_draw::truncate(&r.title, dw, "…"), SUB));
    vec![l]
}

/// The guide over the screen, in the command bar's frame
/// (overlayKeyHelp).
pub fn overlay(m: &mut Model, out: &mut [Line]) {
    let bw = m.w.saturating_sub(6).min(104);
    let body = body(m, bw.saturating_sub(4), (m.h * 4 / 5).max(10) - 2);
    let e = Edge {
        w: bw,
        h: body.len() + 2,
        glint: None,
    };
    let note = if key_map(m).problems().is_empty() {
        Line::new()
    } else {
        line([sp(
            " keybindings.json has problems: Settings, Keys ",
            YELLOW,
        )])
    };
    let mut k = bound_key(m, "guide.open");
    if k.is_empty() {
        k = "f1".into();
    }
    drop_box(
        m,
        out,
        &body,
        e,
        line([spb(" ⌨ Keys ", TEXT)]),
        line([sp(format!(" {} ", pretty_key(&k)), DIM)]),
        note,
    );
}
