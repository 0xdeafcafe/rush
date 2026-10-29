//! The settings shell around a page: its heading, the form laid out with a
//! rail of sections and an inspector beside or under it, the keys that
//! move through it, the mouse, the tabs in the pages row and the details
//! sheet. After Go's internal/ui/settings_workspace.go (settingsGeometry
//! :13, settingsHeading :21, settingsForm :51, settingsHelp :172,
//! settingsNavigation :203, settingsMouse :254, settingsWheel :297,
//! settingHelpSheet :312, settingsTabs :356).

use rush_core::text::{Line, width};
use rush_draw::MouseKind;

use super::form::{self, Section, Setting, flat, row_at};
use super::{
    PAGE_CONFIG, PAGE_GENERAL, PAGE_HARNESSES, PAGE_KEYS, PAGE_PLUGINS, PAGE_PROVIDERS,
    PAGE_REMOTE, PAGE_UPDATES, cur_page, dialog, dialog_len, dialog_mut, place_pages,
};
use crate::frame::overlayclick::MouseEv;
use crate::frame::sheet::window;
use crate::frame::{Mouse, Out, PlaceId, Press, Sheet};
use crate::model::Model;
use crate::style::*;

/// Where the visible rows were drawn, after scrolling and wrapping, so
/// the pointer and the keys pick the same setting at every size
/// (settingsGeometry). In the body's cells.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub top: usize,
    pub x: usize,
    pub width: usize,
    pub rail_width: usize,
    pub control_width: usize,
    /// The setting on each row drawn from top; None for a heading.
    pub rows: Vec<Option<usize>>,
    /// Each section's first setting, by the rail's rows.
    pub sections: Vec<usize>,
    /// A page that draws itself: where its rows are (Keys: the first
    /// row's line, and the rows from..to drawn).
    pub list_top: usize,
    pub list_from: usize,
    pub list_to: usize,
    /// A page laid out as a catalog.
    pub cat: super::catalog::CatGeo,
}

/// The page's description and a line under it (settingsHeading).
pub fn heading(m: &Model, w: usize) -> Vec<Line> {
    let d = dialog(m);
    let desc = match d.page {
        PAGE_PROVIDERS => "Whose models you use: sign-ins, keys and limits.",
        PAGE_HARNESSES => {
            "The programs that run them: what every one starts with, and what one changes."
        }
        PAGE_CONFIG => {
            "Which harnesses are on, which models each may call, and what happens at a limit."
        }
        PAGE_GENERAL => "Sessions, notifications, background work, and how the workspace looks.",
        PAGE_KEYS => "Find, practice and customize your shortcuts.",
        PAGE_PLUGINS => "Manage extensions and the access they need.",
        PAGE_UPDATES => "Keep rush and your installed tools up to date.",
        PAGE_REMOTE => {
            "Your sessions from your phone or another device: in a browser, or over SSH."
        }
        _ => "",
    };
    let mut title = Line::new();
    if let Some(p) = super::extra_title(m) {
        title.spans.push(sp(p, TEXT));
        title.spans.push(plain(" · "));
    }
    title.spans.push(sp(desc, SUB));
    let mut out = vec![fit(title, w), Line::new()];
    if let Some(head) = cur_page(m).head
        && let Some(first) = head(m, w).into_iter().next()
    {
        out[1] = fit(first, w);
    }
    if let Some(l) = super::totals_line(m, w) {
        out[1] = fit(l, w);
    }
    out
}

/// A form laid out h tall: the rows scrolled to the cursor, a rail of
/// sections on a wide screen, the highlighted row explained beside them
/// or under them, then a rule and the keys (settingsForm). Only the rows
/// move; the keys and the explanation never fall off screen.
pub fn form(
    m: &Model,
    geo: &mut Geometry,
    secs: &[Section],
    page_keys: &[String],
    w: usize,
    h: usize,
) -> Vec<Line> {
    let all = flat(secs);
    let previews = all.iter().any(|s| s.preview.is_some());
    let cursor = dialog(m).cursor.min(all.len().saturating_sub(1));
    let cur = row_at(secs, cursor);
    let h = h.max(1);
    let help_w = if w >= 100 && !previews {
        58.min(w / 3)
    } else {
        0
    };
    let rail_w = if w >= 145 && secs.len() > 1 && !previews {
        24
    } else {
        0
    };
    let mut main_w = w;
    if help_w > 0 {
        main_w = main_w.saturating_sub(help_w + 3);
    }
    if rail_w > 0 {
        main_w = main_w.saturating_sub(rail_w + 3);
    }

    // The whole form keeps each section's context, and one arrow crosses
    // a section's edge. The rail is a shortcut, not a second way to move.
    let rows = form::form_rows(secs, cursor, main_w);
    let mut indices: Vec<Option<usize>> = vec![None; rows.len()];
    let (mut at, mut index, mut selected) = (0, 0, 0);
    let mut starts = Vec::with_capacity(secs.len());
    for (si, sec) in secs.iter().enumerate() {
        starts.push(index);
        at += 1;
        if !sec.title.is_empty() {
            at += 1;
        }
        for _ in &sec.rows {
            if at < indices.len() {
                indices[at] = Some(index);
            }
            if index == cursor {
                selected = si;
            }
            index += 1;
            at += 1;
        }
    }
    let cursor_line = indices.iter().position(|n| *n == Some(cursor)).unwrap_or(0);
    // A narrow screen's inspector is small and under the rows; a wide one
    // has the right of the screen, with a preview where there is one.
    let mut extra: Vec<String> = vec!["?".into(), "details".into()];
    if secs.len() > 1 {
        extra.push("ctrl+↑↓".into());
        extra.push("section".into());
    }
    extra.extend(page_keys.iter().cloned());
    let footer = form::form_keys(&cur, &extra, w);
    let content_h = h.saturating_sub(2).max(1);
    let mut help = Vec::new();
    if help_w > 0 {
        help = self::help(&cur, help_w, content_h);
    }
    let mut bottom_h = 0;
    if help_w == 0 && content_h >= 9 {
        bottom_h = 7.min(content_h / 3);
        if previews && content_h >= 24 {
            bottom_h = 26.min(content_h * 2 / 3);
        }
        help = self::help(&cur, w, bottom_h);
    }
    let list_h = content_h.saturating_sub(bottom_h).max(1);
    let (first, last) = window(rows.len(), cursor_line, list_h);
    *geo = Geometry {
        top: 2,
        x: 0,
        width: w,
        rail_width: rail_w,
        control_width: main_w,
        rows: indices[first..last].to_vec(),
        sections: if rail_w > 0 { starts } else { Vec::new() },
        ..Geometry::default()
    };
    let mut out = Vec::with_capacity(h);
    for i in 0..content_h {
        let mut left = Line::new();
        let mut middle = Line::new();
        let mut right = Line::new();
        if rail_w > 0 && i < secs.len() {
            left = if i == selected {
                line([sp("▸ ", ORANGE), spb(secs[i].title.clone(), TEXT)])
            } else {
                line([sp(format!("  {}", secs[i].title), SUB)])
            };
        }
        if i < list_h && first + i < last {
            middle = rows[first + i].clone();
        }
        if help_w == 0
            && i >= list_h
            && let Some(l) = help.get(i - list_h)
        {
            middle = l.clone();
        }
        if help_w > 0
            && let Some(l) = help.get(i)
        {
            right = l.clone();
        }
        let mut row = fit(middle, main_w);
        if rail_w > 0 {
            let mut r = fit(left, rail_w);
            r.spans.push(sp(" │ ", FAINT));
            join(&mut r, row);
            row = r;
        }
        if help_w > 0 {
            row.spans.push(sp(" │ ", FAINT));
            join(&mut row, fit(right, help_w));
        }
        out.push(row);
    }
    let progress = if all.is_empty() {
        String::new()
    } else {
        format!("{} of {}", cursor + 1, all.len())
    };
    out.push(rule("", &progress, w));
    out.push(footer);
    out.truncate(h);
    out
}

/// The highlighted setting explained, w wide in at most h rows: its
/// title, what it does, what it's set to, and a preview where there's
/// room (settingsHelp).
pub fn help(st: &Setting, w: usize, h: usize) -> Vec<Line> {
    let (title, what, now) = st.explained();
    let w1 = w.max(1);
    let mut out = vec![line([spb(fit_str(&title, w), TEXT)]), Line::new()];
    if !what.is_empty() {
        for l in wrap(&what, w1) {
            out.push(line([sp(l, SUB)]));
        }
    }
    if !now.is_empty() {
        out.push(Line::new());
        for l in wrap(&now, w1) {
            out.push(line([sp(l, TEXT)]));
        }
    }
    if let Some(p) = &st.preview
        && h.saturating_sub(out.len()) >= 5
        && h > out.len()
    {
        out.push(Line::new());
        out.push(line([sp("Preview", SUB)]));
        // The room left; a sheet's endless room stops where the example
        // does.
        let room = 18.min(h.saturating_sub(out.len()));
        out.extend(p(w1, room));
    }
    if out.len() > h {
        out.truncate(h.max(1));
        // The whole explanation is there on a larger screen; the value
        // stays in view even when the description is cut.
        if !now.is_empty() && h > 2 {
            out[h - 1] = line([sp(fit_str(&now, w), TEXT)]);
        }
    }
    out
}

/// The keys that move through a form: ? its details, ctrl+↑↓ a section,
/// home end pgup pgdown (settingsNavigation). False when s isn't one.
pub fn navigation(m: &mut Model, s: &str) -> bool {
    if dialog(m).page == PAGE_KEYS {
        return false;
    }
    let Some(f) = cur_page(m).form else {
        return false;
    };
    let n = dialog_len(m);
    let step = (super::settings_height(m) as isize - 4).max(2) as usize / 2;
    let step = step.max(1);
    let cursor = dialog(m).cursor;
    let to = match s {
        "?" => {
            let st = row_at(&f(m), cursor);
            m.open_sheet(Box::new(HelpSheet {
                setting: st,
                scroll: 0,
                height: 0,
                count: 0,
            }));
            return true;
        }
        "ctrl+up" | "ctrl+down" => {
            let mut starts = Vec::new();
            let mut at = 0;
            for sec in f(m) {
                if !sec.rows.is_empty() {
                    starts.push(at);
                }
                at += sec.rows.len();
            }
            if s == "ctrl+down" {
                starts.into_iter().find(|&a| a > cursor).unwrap_or(cursor)
            } else {
                starts
                    .into_iter()
                    .rev()
                    .find(|&a| a < cursor)
                    .unwrap_or(cursor)
            }
        }
        "home" => 0,
        "end" => n.saturating_sub(1),
        "pgup" => cursor.saturating_sub(step),
        "pgdown" => (cursor + step).min(n.saturating_sub(1)),
        _ => return false,
    };
    dialog_mut(m).cursor = to;
    true
}

/// The mouse on a page (settingsMouse, settingsWheel): x and y in the
/// body's cells.
pub fn mouse(m: &mut Model, g: &Geometry, e: &Mouse, x: usize, y: usize) -> Out {
    if !dialog(m).asking.is_empty() {
        return Out::Idle;
    }
    match e.kind {
        MouseKind::ScrollUp => wheel(m, g, x, y, -1),
        MouseKind::ScrollDown => wheel(m, g, x, y, 1),
        MouseKind::Down(rush_draw::Button::Left) => click(m, g, x, y),
        MouseKind::Moved if dialog(m).page == PAGE_KEYS => {
            if super::keys::hover(m, g, y) {
                Out::Draw
            } else {
                Out::Idle
            }
        }
        _ => Out::Idle,
    }
}

fn click(m: &mut Model, g: &Geometry, x: usize, y: usize) -> Out {
    if dialog(m).page == PAGE_KEYS {
        if y < g.list_top {
            return Out::Idle;
        }
        let row = y - g.list_top + g.list_from;
        if x < g.width && row >= g.list_from && row < g.list_to {
            if dialog(m).cursor == row {
                return super::dialog_key(m, &Press::of("enter").expect("enter"));
            }
            dialog_mut(m).cursor = row;
            return Out::Draw;
        }
        return Out::Idle;
    }
    let p = cur_page(m);
    let Some(f) = p.form else {
        return super::catalog_mouse(m, g, x, y);
    };
    if y < g.top || x < g.x || x >= g.x + g.width {
        return Out::Idle;
    }
    let at = y - g.top;
    if g.rail_width > 0 && x < g.x + g.rail_width {
        if let Some(&s) = g.sections.get(at) {
            dialog_mut(m).cursor = s;
            return Out::Draw;
        }
        return Out::Idle;
    }
    let mut control_x = g.x;
    if g.rail_width > 0 {
        control_x += g.rail_width + 3;
    }
    if x < control_x || x >= control_x + g.control_width {
        return Out::Idle;
    }
    let Some(Some(row)) = g.rows.get(at).copied() else {
        return Out::Idle;
    };
    if dialog(m).cursor == row {
        let secs = f(m);
        return form::form_key(m, &secs, "enter");
    }
    dialog_mut(m).cursor = row;
    Out::Draw
}

fn wheel(m: &mut Model, g: &Geometry, x: usize, y: usize, delta: isize) -> Out {
    let page = dialog(m).page;
    if cur_page(m).form.is_some() || page == PAGE_KEYS {
        if y >= 2 && x < g.width.max(1) {
            let n = dialog_len(m) as isize;
            let d = dialog_mut(m);
            d.cursor = (d.cursor as isize + delta).clamp(0, (n - 1).max(0)) as usize;
            return Out::Draw;
        }
        return Out::Idle;
    }
    super::catalog_wheel(m, g, x, y, delta)
}

/// The highlighted setting's whole explanation, for a small screen
/// (settingHelpSheet).
pub struct HelpSheet {
    pub setting: Setting,
    pub scroll: usize,
    pub height: usize,
    pub count: usize,
}

impl Sheet for HelpSheet {
    fn width(&self, _m: &Model) -> usize {
        92
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let rows = help(&self.setting, w, 10000);
        self.height = h.saturating_sub(2).max(1);
        self.count = rows.len();
        self.scroll = self.scroll.min(rows.len().saturating_sub(self.height));
        let to = rows.len().min(self.scroll + self.height);
        let mut out = rows[self.scroll..to].to_vec();
        out.push(Line::new());
        out.push(keys_fit(w, &[("↑↓", "scroll"), ("esc", "back")]));
        out
    }

    fn key(&mut self, _m: &mut Model, k: &Press) -> Out {
        let mut s = self.scroll as isize;
        let h = self.height as isize;
        match k.s.as_str() {
            "esc" | "q" | "?" => return Out::Close,
            "up" | "k" => s -= 1,
            "down" | "j" => s += 1,
            "pgup" => s -= h,
            "pgdown" | "space" => s += h,
            "home" => s = 0,
            "end" => s = self.count as isize,
            _ => {}
        }
        self.scroll = s.clamp(0, (self.count as isize - h).max(0)) as usize;
        Out::Draw
    }

    fn mouse(&mut self, m: &mut Model, ev: MouseEv, _x: i32, _y: i32) -> Out {
        match ev {
            MouseEv::WheelDown => self.key(m, &Press::of("down").expect("down")),
            MouseEv::WheelUp => self.key(m, &Press::of("up").expect("up")),
            _ => Out::Idle,
        }
    }
}

/// The place's pages as tabs, the page showing and its neighbours kept
/// in view when they don't all fit (settingsTabs). Clicks read the same
/// names.
pub fn tabs(m: &Model, place: PlaceId, w: usize) -> Line {
    let (lo, hi) = place_pages(place);
    let all = super::pages();
    let names: Vec<&str> = all[lo..hi].iter().map(|p| p.name).collect();
    let cur = dialog(m).page.clamp(lo, hi - 1) - lo;
    let render = |from: usize, to: usize| {
        let mut l = Line::new();
        let mut first = true;
        let mut sep = |l: &mut Line| {
            if !first {
                l.spans.push(plain("   "));
            }
            first = false;
        };
        if from > 0 {
            sep(&mut l);
            l.spans.push(sp("‹", DIM));
        }
        for (i, name) in names.iter().enumerate().take(to).skip(from) {
            sep(&mut l);
            if i == cur {
                let mut st = bold(ORANGE);
                st.mods = st.mods.with(rush_core::text::Mods::UNDERLINE);
                l.spans.push(rush_core::text::Span::new(*name, st));
            } else if m.top_hover == Some((i, true)) {
                join(&mut l, hover_line(line([sp(*name, TEXT)]), width(name)));
            } else {
                l.spans.push(sp(*name, SUB));
            }
        }
        if to < names.len() {
            sep(&mut l);
            l.spans.push(sp("›", DIM));
        }
        l
    };
    let (mut from, mut to) = (0, names.len());
    while render(from, to).width() > w && to - from > 1 {
        if cur - from > to - cur - 1 {
            from += 1;
        } else {
            to -= 1;
        }
    }
    let mut tabs = render(from, to);
    if tabs.width() + 6 <= w {
        tabs.spans.push(sp("   [ ]", DIM));
    }
    fit(tabs, w)
}
