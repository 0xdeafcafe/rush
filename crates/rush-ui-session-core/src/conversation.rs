//! The conversation as the Session draws it: the rows in view with the
//! item under the pointer lit and the dragged-over text painted, a pill
//! saying how much is below once scrolled up, and the map beside it from
//! 96 wide. After Go's internal/ui/rushmode.go (paneView's conversation
//! :2239-2245, :2420-2432, the pill :2604-2611, the map :2643-2660,
//! withMap :2690, the dock's rows on the transcript's end :2216, its
//! Needs-you section from turnwants.rs, rushPane's o.Needs :2369),
//! pane_pointer.go (ownerAt :89, paintHover :111, the @agent hovered
//! underlined :134) and promptpins.go
//! (pinPrompts :31, clickRef's pin rushmode.go:4861). Its links: a
//! left-click on one rush opens itself (messagesheet.go:326 clickMessage,
//! :335-337), a right-click's menu (linkmenu.go:88 linkMenu, model.go:1720)
//! and o's on the picked step (linkmenu.go:57 pickedLink, rushmode.go:4126). The session's views
//! go between the text and the map (rushPane :2361, :2725-2737; laid by
//! plugin_ui's views::beside).

use rush_convo::render::OWNER_SEP;
use rush_core::text::{Color, Line, Span, Style};
use rush_draw::{Button, MouseKind};

use super::minimap::{self, MINI_WIDTH};
use super::paint::spread;
use super::{conn, open_key, textsel};
use crate::frame::{Mouse, Out};
use crate::model::Model;
use crate::session::Pos;
use crate::style::*;
use rush_ui_plugin_ui::views;

/// What the card the conversation opens on names (rushPane :2371-2395):
/// the rush session it works for as a subagent and the one that started
/// it beside it, by name; whether rush runs it; and the brief its start
/// added to its prompt, read from its host's config once a session, off
/// the UI thread, there from the frame after.
fn opening(m: &mut Model, key: &str) -> (String, String, bool, String) {
    let Some(i) = m.session.as_ref().map(|v| &v.sess.info) else {
        return Default::default();
    };
    let (id, spawned_by, started_from) =
        (i.id.clone(), i.spawned_by.clone(), i.started_from.clone());
    let name_of = |by: &str| {
        if by.is_empty() {
            return String::new();
        }
        m.agents
            .iter()
            .find(|a| a.rush && a.job.id == by)
            .map(|a| a.display_name.clone())
            .unwrap_or_default()
    };
    let (parent, from) = (name_of(&spawned_by), name_of(&started_from));
    let c = conn(m, key);
    if !id.is_empty() && c.brief.0 != id && c.brief_for != id {
        c.brief_for = id.clone();
        let (k, got) = (key.to_string(), id.clone());
        m.later(
            {
                let id = id.clone();
                move || {
                    rush_host::read_config(&id)
                        .map(|c| c.system_prompt.trim().to_string())
                        .unwrap_or_default()
                }
            },
            move |m, b| {
                conn(m, &k).brief = (got, b);
                Out::Draw
            },
        );
    }
    let c = conn(m, key);
    let brief = if !id.is_empty() && c.brief.0 == id {
        c.brief.1.clone()
    } else {
        String::new()
    };
    (parent, from, !id.is_empty(), brief)
}

/// When each session's conversation was looked at, for the Catch-up
/// view's "since you last looked": a look runs while the conversation is
/// drawn frame after frame; one begun after a frame without it (and a
/// while since) starts from the end of the one before. Rust-only: Go rush
/// had no such view.
#[derive(Default)]
pub struct Looked(std::collections::HashMap<String, Look>);

#[derive(Clone, Copy, Default)]
struct Look {
    /// When the look before this one ended; 0 for the first.
    since: rush_core::time::Ms,
    /// The frame it was last drawn in, and when.
    frame: u64,
    at: rush_core::time::Ms,
}

/// A frame gap shorter than this keeps the look going: a glance at the
/// list and back isn't looking away.
const AWAY: rush_core::time::Ms = 30_000;

/// When you last looked at session key's conversation before this look
/// (0: not since rush started), noting that it's looked at now.
pub fn looked_since(m: &mut Model, key: &str) -> rush_core::time::Ms {
    let (frame, now) = (m.frames, m.now);
    let l = m.kept::<Looked>().0.entry(key.to_string()).or_default();
    if l.frame != 0 && l.frame + 1 < frame && now - l.at >= AWAY {
        l.since = l.at;
    }
    l.frame = frame;
    l.at = now;
    l.since
}

/// The conversation's rows, `rows` tall and `w` wide, with the map in its
/// last MINI_WIDTH cells when it's on.
pub fn body(m: &mut Model, key: &str, w: usize, rows: usize) -> Vec<Line> {
    // A view that fits, at the moments it's worth saying (off: nothing).
    super::viewsuggest::look(m, key);

    // The views' dropdown, when open, hangs over its first rows under the
    // Conversation tab.
    let mut out = text(m, key, w, rows);
    super::presets::draw_menu(m, key, &mut out, w);
    out
}

/// The conversation's text, `rows` tall (see body).
fn text(m: &mut Model, key: &str, w: usize, rows: usize) -> Vec<Line> {
    conn(m, key).vs.rows = rows;
    // Its last answer ends with what that turn wants of you.
    super::turnwants::sync_needs(m);
    let map_on = map_on(m, key, w);
    // It runs down beside the dock and its keys too.
    let map_h = rows + conn(m, key).dock_rows + 1;
    let map_w = if map_on { MINI_WIDTH } else { 0 };
    // The session's own views sit beside its conversation, when there's
    // room: left of the map.
    let (vs, past, vw) = views::beside::views_beside(m, key, w.saturating_sub(map_w));
    let side = map_w + vw;
    let (now, tick) = (m.now, m.tick);
    let pref = m.cfg.column_width;
    let txt = conn(m, key).txt;
    let hover = conn(m, key).vs.hover_ref.clone();
    let hover_tag = conn(m, key).vs.hover_tag.clone();
    let mut mm = std::mem::take(&mut conn(m, key).map);
    let opening = opening(m, key);
    let preset = super::presets::current(m);
    let since = looked_since(m, key);
    // Rows run the whole width while no column stands right of them
    // (rushmode.go:2366 Wide: len(c.side) == 0), hosted alone or not: the
    // column is drawn over the room past the reading width, so rows wrapped
    // for the whole width would lose their ends under it.
    let wide = conn(m, key).side.rows.is_empty();
    let Some(v) = m.session.as_mut() else {
        return Vec::new();
    };
    v.side = side;
    v.sync(w, now, tick);
    let o = &mut v.o;
    o.wide = wide;
    o.preset = preset;
    o.since = since;
    (o.parent, o.started_from, o.hosted) = (opening.0, opening.1, opening.2);
    if o.brief != opening.3 {
        o.brief = opening.3;
    }
    let cw = w.saturating_sub(side);
    let mut n = rows;
    let mut out = if !v.read {
        v.note_drawn();
        vec![fit(line([sp("reading the conversation…", DIM)]), cw)]
    } else if v.sess.turns.is_empty() && !rush_convo::shellbox::unturned(&v.o.shells) {
        let first = if v.err.is_empty() {
            line([sp("  nothing yet · type below to start", DIM)])
        } else {
            line([sp(format!("couldn't read it: {}", v.err), RED)])
        };
        // The panels follow what's there, a row down.
        v.panels_at = 1;
        let mut out = vec![fit(first, cw)];
        out.extend(v.panels.iter().cloned());
        v.shown = vec![String::new(); out.len().min(rows)];
        out
    } else {
        // Scrolled up, the "more below" pill takes a row of its own rather
        // than covering the last one (which may be the one picked).
        if v.pos != Pos::End && rows > 1 {
            n = rows - 1;
        }
        let mut b = v.body(n);
        let drawn = v.r.rows();
        if !hover.is_empty() {
            let under = format!("{hover}{OWNER_SEP}");
            for (i, l) in b.iter_mut().enumerate() {
                let Some(r) = (v.start + i).checked_sub(v.base).and_then(|j| drawn.get(j)) else {
                    continue;
                };
                let o = if r.owner.is_empty() {
                    &r.r#ref
                } else {
                    &r.owner
                };
                if *o == hover || o.starts_with(&under) {
                    *l = hover_line(std::mem::take(l), cw);
                }
            }
        }
        if let Some((r, u)) = &hover_tag
            && let Some(l) = b.get_mut(*r)
        {
            rush_ui_session_cards::handles::underline_link(l, u);
        }
        textsel::paint(&mut b, v.start, &txt, cw);
        pin_prompt(v, &mut b, cw, pref);
        if v.pos != Pos::End {
            let total = v.total();
            let below = total.saturating_sub(v.start + b.len());
            let pill = line([
                Span::new(" ", Style::default().on(SEL_BG)),
                Span::new(
                    format!("↓ {below} more · end follows"),
                    Style::fg(TEXT).on(SEL_BG),
                ),
                Span::new(" ", Style::default().on(SEL_BG)),
            ]);
            b.truncate(n);
            while b.len() < n {
                b.push(Line::new());
            }
            b.push(spread(Line::new(), pill, cw));
        }
        b
    };
    out.truncate(rows);
    // The column beside the text runs down to where a card, the queue or
    // anything else under the transcript starts, the room before the box
    // its own too; else down to the body's foot (rushPane's foot,
    // rushmode.go:2694, past the transcript's blank rows).
    let foot =
        if v.read && (!v.sess.turns.is_empty() || rush_convo::shellbox::unturned(&v.o.shells)) {
            let t = v.r.count(&v.sess, &v.o).saturating_sub(v.start);
            let drawn = v.total().saturating_sub(v.start).min(n);
            if t < drawn {
                let blank = |l: &Line| {
                    l.spans
                        .iter()
                        .all(|s| s.style.bg == Color::Default && s.text.trim().is_empty())
                };
                (t..rows)
                    .find(|&i| out.get(i).is_some_and(|l| !blank(l)))
                    .unwrap_or(rows)
            } else {
                rows
            }
        } else {
            0
        };
    let top = v.top;
    super::side::paste(m, key, &mut out, foot, cw, top);
    views::beside::paste(m, key, &mut out, rows, cw, vw, &vs, &past, top);
    // The map, and what's drawn after, run beside the views.
    let cw = cw + vw;
    let Some(v) = m.session.as_mut() else {
        return out;
    };
    if map_on {
        while out.len() < rows {
            out.push(Line::new());
        }
        mm.prepare(v, cw, map_h, now);
        mm.visible = true;
        mm.x = 2 + cw;
        mm.y = v.top;
        mm.viewport = n;
        mm.start = v.start;
        // The view ends where the conversation does when it's short.
        mm.end = (v.start + n).min(v.total());
        for (y, l) in out.iter_mut().enumerate() {
            let mut r = fit(std::mem::take(l), cw);
            join(&mut r, mm.line(y));
            *l = r;
        }
    } else {
        mm = minimap::Minimap::default();
    }
    conn(m, key).map = mm;
    out
}

// What a pinned row's ref starts with is the base's (crate::sess).
pub use crate::sess::PIN_REF;

/// Whether turn t began with something you said (yourPrompt).
fn your_prompt(t: &rush_convo::Turn) -> bool {
    t.from.is_empty() && !t.prompt.trim().is_empty()
}

/// Prompts pinned over the body so you can find your way while scrolling
/// (pinPrompts, promptpins.go:31, and more): the first, at the top, once
/// it's scrolled off; the one before what you're reading under it ("current",
/// or "latest" at the end); the next one after what you're reading, over the
/// bottom row while it's below the screen. A short pane keeps its rows for
/// the conversation: none under 7 rows, one top pin (the before one wins)
/// under 10, the bottom one from 7.
/// The pins shown with the view at row start, h rows tall: (screen row, turn
/// index, label).
pub fn pin_plan(
    v: &crate::session::SessionView,
    start: usize,
    h: usize,
) -> Vec<(usize, usize, &'static str)> {
    if h < 7 {
        return Vec::new(); // a short pane keeps its rows for the conversation
    }
    let end = start + h;
    let turns = &v.sess.turns;
    let Some(first) = turns.iter().position(your_prompt) else {
        return Vec::new();
    };
    let off = |i: usize| v.r.turn_row(i) < start;
    let (mut latest, mut current) = (None, None);
    for i in (first..turns.len()).rev() {
        if !your_prompt(&turns[i]) {
            continue;
        }
        latest.get_or_insert(i);
        if off(i) {
            current = Some(i);
            break;
        }
    }
    let mut label = "current";
    if current.is_some() && current == latest {
        label = "latest ";
    } else if latest.is_some_and(|l| v.r.turn_row(l) < end) {
        current = None; // the latest is on screen: that's what it's on
    }
    let before = current.filter(|&i| i != first && off(i));
    let top_first = off(first).then_some(first);
    let next = (first..turns.len()).find(|&i| your_prompt(&turns[i]) && v.r.turn_row(i) >= end);
    let mut pins: Vec<(usize, usize, &'static str)> = Vec::new();
    match (top_first, before) {
        (Some(f), Some(c)) if h >= 10 => {
            pins.push((0, f, "first  "));
            pins.push((1, c, label));
        }
        (_, Some(c)) => pins.push((0, c, label)),
        (Some(f), None) => pins.push((0, f, "first  ")),
        (None, None) => {}
    }
    if let Some(n) = next {
        pins.push((h - 1, n, "next   "));
    }
    pins
}

fn pin_prompt(v: &mut crate::session::SessionView, b: &mut [Line], w: usize, pref: i64) {
    let h = b.len();
    let pins = pin_plan(v, v.start, h);
    let turns = &v.sess.turns;
    // With a column beside the text, the pin stops where it starts, as
    // the text does, rather than running across a wide Session.
    let sw = super::side::side_width_with(w, pref);
    let pw = if sw > 0 { w - sw - 2 } else { w };
    let mut refs = Vec::new();
    for (y, i, label) in pins {
        let t = &turns[i];
        let text = one_line(&rush_convo::render::fold_pastes(&t.prompt));
        let row = line([
            sp("▍", BLUE),
            plain(" "),
            sp(label, FAINT),
            plain(" "),
            sp(text, TEXT),
        ]);
        b[y] = on_ground(fit(row, pw), pw, CHROME_BG);
        refs.push((y, format!("{PIN_REF}t{}", t.n)));
    }
    for (y, r) in refs {
        if v.shown.len() <= y {
            v.shown.resize(y + 1, String::new());
        }
        v.shown[y] = r;
    }
}

/// A click on a pinned prompt goes to its turn, picked, landing just below
/// the pins that will show there, so its heading is in view (clickRef's pin;
/// the landing is `SessionView::want_start`).
pub fn click_pin(v: &mut crate::session::SessionView, r: &str) -> bool {
    let Some(t) = r.strip_prefix(PIN_REF) else {
        return false;
    };
    let Some(n) = t.strip_prefix('t').and_then(|n| n.parse().ok()) else {
        return false;
    };
    v.sel = t.to_string();
    v.pos = Pos::At { n, off: 0 };
    v.land = true;
    true
}

/// Whether the map is beside session key's conversation, its pane w
/// wide: from 96 wide, on the conversation (minimapEnabled).
pub fn map_on(m: &mut Model, key: &str, w: usize) -> bool {
    !m.cfg.hide_minimap
        && !super::core(m).hide_map
        && minimap::enabled(w)
        && super::views::view_name(m, key) == "conversation"
}

/// The map on down beside the dock and the keys under it, over the
/// finished frame (paneView :2656, the map beside the dock).
#[allow(clippy::ptr_arg)] // the frame's Overlay hook's type
pub fn over(m: &mut Model, out: &mut Vec<Line>) {
    let Some(key) = open_key(m) else {
        return;
    };
    if m.on_place() {
        return;
    }
    let rows = conn(m, &key).vs.rows;
    let mm = &conn(m, &key).map;
    if !mm.visible {
        return;
    }
    for y in rows..mm.height {
        let Some(l) = out.get_mut(mm.y + y) else {
            break;
        };
        let mut r = fit(super::paint::cut(l, 0, mm.x), mm.x);
        join(&mut r, mm.line(y));
        *l = r;
    }
}

/// The item conversation row i on screen is part of: its owner, or its
/// own ref (ownerAt).
pub fn owner_at(m: &Model, i: usize) -> String {
    let Some(v) = m.session.as_ref() else {
        return String::new();
    };
    let rows = v.r.rows();
    let Some(r) = (v.start + i)
        .checked_sub(v.base)
        .and_then(|j| rows.get(j))
        .filter(|_| i < v.shown.len())
    else {
        return String::new();
    };
    if r.owner.is_empty() {
        r.r#ref.clone()
    } else {
        r.owner.clone()
    }
}

/// A press, drag or release on the map scrolls the conversation there
/// (pressMinimap, dragMinimap).
/// The link drawn in the conversation's text at screen cell (x, y), if
/// any: only over the conversation's own rows, not the header, the dock
/// or the map (linkAt over textCell).
pub fn link_under(m: &mut Model, key: &str, x: usize, y: usize) -> Option<String> {
    let l = cell_link(m, key, x, y)?;
    l.url().map(|u| u.to_string())
}

/// The link carried by the conversation's cell at (x, y): a quiet one
/// is a picture's, rush's alone (rush_core::text::Link::quiet).
fn cell_link(m: &mut Model, key: &str, x: usize, y: usize) -> Option<rush_core::text::Link> {
    if super::views::view_name(m, key) != "conversation" {
        return None;
    }
    let v = m.session.as_ref()?;
    if y < v.top || y >= v.top + v.shown.len() || x < 2 || x >= 2 + v.o.width {
        return None;
    }
    if super::side::over(m, key, x, y) {
        return None; // the side column's, not the text's
    }
    let at = textsel::text_cell(m, x, y)?;
    let row = textsel::drawn_row(m, at.row)?;
    let mut col = 0;
    for s in &row.line.spans {
        let w = s.width();
        if at.col < col + w {
            return Some(s.style.link).filter(|l| !l.is_none());
        }
        col += w;
    }
    None
}

/// Whether a link is an image's file, one Preview opens.
fn image_link(u: &str) -> bool {
    u.starts_with("file:") && rush_ui_cmdbar::linkmenu::previewable(u)
}

/// The first file linked in the rows of the picked step: its own, and
/// those under it with no step of their own, as its pictures
/// (linkmenu.go:57 pickedLink).
pub fn picked_link(m: &Model) -> Option<String> {
    let v = m.session.as_ref()?;
    if v.sel.is_empty() {
        return None;
    }
    let mut inside = false;
    for r in v.r.rows() {
        if !r.r#ref.is_empty() {
            inside = r.r#ref == v.sel;
        }
        if !inside {
            continue;
        }
        for s in &r.line.spans {
            if let Some(u) = s.style.link.url()
                && u.starts_with("file:")
            {
                return Some(u.to_string());
            }
        }
    }
    None
}

pub fn mouse(m: &mut Model, e: &Mouse) -> Out {
    if m.session.is_none() || m.confirm.is_some() || m.sheet.is_some() || m.on_place() {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let (x, y) = (e.x as usize, e.y as usize);
    let rows = conn(m, &key).vs.rows.max(1);
    if e.kind == MouseKind::Down(Button::Left) && super::side::click(m, x, y) {
        return Out::Draw;
    }
    // Right-clicking a link asks what to do with it: open it, in Preview,
    // Quick Look, reveal or copy it (model.go:1720, linkMenu); on a
    // picture, nothing: a click opens it.
    if e.kind == MouseKind::Down(Button::Right) {
        if cell_link(m, &key, x, y).is_some_and(|l| l.is_quiet()) {
            return Out::Pass;
        }
        return match link_under(m, &key, x, y) {
            Some(u) if rush_ui_cmdbar::linkmenu::open_link_menu(m, &u) => Out::Draw,
            _ => Out::Pass,
        };
    }
    let to = match e.kind {
        MouseKind::Down(Button::Left) => {
            let mm = &mut conn(m, &key).map;
            if !mm.hit(x, y) {
                // A link of a kind rush opens itself, a chip of a message
                // sent or an @agent, before the text's selection
                // (clickMessage).
                if let Some(u) = link_under(m, &key, x, y) {
                    // A shell box's buttons: run, copy, stop, again, send
                    // to the agent, clear.
                    if let Some(o) = super::shellbox::click(m, &key, &u) {
                        return o;
                    }
                    let o = rush_ui_session_cards::message::message_link(m, &key, &u);
                    if o.used() {
                        return o;
                    }
                    // An image's chip or thumbnail opens its menu, the
                    // image drawn above what can be done with it, as o on
                    // its step does.
                    if image_link(&u) && rush_ui_cmdbar::linkmenu::open_link_menu(m, &u) {
                        return Out::Draw;
                    }
                }
                return Out::Pass;
            }
            let to = mm.press(y);
            conn(m, &key).txt = textsel::TextSel::default();
            match to {
                Some(t) => t,
                None => return Out::Draw,
            }
        }
        MouseKind::Drag(Button::Left) if conn(m, &key).map.dragging => {
            let mm = &conn(m, &key).map;
            if y == mm.drag_y {
                mm.drag_start
            } else {
                mm.drag(y)
            }
        }
        MouseKind::Up(Button::Left) if conn(m, &key).map.dragging => {
            conn(m, &key).map.dragging = false;
            return Out::Draw;
        }
        _ => return Out::Pass,
    };
    let mut mm = std::mem::take(&mut conn(m, &key).map);
    if let Some(v) = m.session.as_mut() {
        minimap::seek(v, &mut mm, to, rows);
    }
    conn(m, &key).map = mm;
    Out::Draw
}
