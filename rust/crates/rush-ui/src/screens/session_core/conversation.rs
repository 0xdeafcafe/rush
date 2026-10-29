//! The conversation as the Session draws it: the rows in view with the
//! item under the pointer lit and the dragged-over text painted, a pill
//! saying how much is below once scrolled up, and the map beside it from
//! 96 wide. After Go's internal/ui/rushmode.go (paneView's conversation
//! :2239-2245, :2420-2432, the pill :2604-2611, the map :2643-2660,
//! withMap :2690, the dock's rows on the transcript's end :2216, its
//! Needs-you section from turnwants.rs, rushPane's o.Needs :2369),
//! pane_pointer.go (ownerAt :89, paintHover :111) and promptpins.go
//! (pinPrompts :31, clickRef's pin rushmode.go:4861).

use rush_convo::render::OWNER_SEP;
use rush_core::text::{Line, Span, Style};
use rush_draw::{Button, MouseKind};

use super::minimap::{self, MINI_WIDTH};
use super::paint::spread;
use super::{conn, open_key, textsel};
use crate::frame::{Mouse, Out};
use crate::model::Model;
use crate::session::Pos;
use crate::style::*;

/// The conversation's rows, `rows` tall and `w` wide, with the map in its
/// last MINI_WIDTH cells when it's on.
pub fn body(m: &mut Model, key: &str, w: usize, rows: usize) -> Vec<Line> {
    // Its last answer ends with what that turn wants of you.
    super::turnwants::sync_needs(m);
    let map_on = map_on(m, key, w);
    // It runs down beside the dock and its keys too.
    let map_h = rows + conn(m, key).dock_rows + 1;
    let side = if map_on { MINI_WIDTH } else { 0 };
    let (now, tick) = (m.now, m.tick);
    let txt = conn(m, key).txt;
    let hover = conn(m, key).vs.hover_ref.clone();
    let mut mm = std::mem::take(&mut conn(m, key).map);
    let Some(v) = m.session.as_mut() else {
        return Vec::new();
    };
    v.side = side;
    v.sync(w, now, tick);
    let cw = w.saturating_sub(side);
    let mut n = rows;
    let mut out = if !v.read {
        vec![fit(line([sp("reading the conversation…", DIM)]), cw)]
    } else if v.sess.turns.is_empty() {
        let first = if v.err.is_empty() {
            line([sp("  nothing yet · type below to start", DIM)])
        } else {
            line([sp(format!("couldn't read it: {}", v.err), RED)])
        };
        // The panels follow what's there.
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
        textsel::paint(&mut b, v.start, &txt, cw);
        pin_prompt(v, &mut b, cw);
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
    // The column beside the text, at the foot of the transcript's rows on
    // screen: where a card or the queue under it starts, else the body's
    // (rushPane's foot, rushmode.go:2694).
    let foot = if v.read && !v.sess.turns.is_empty() {
        let t = v.r.count(&v.sess, &v.o).saturating_sub(v.start);
        let drawn = v.total().saturating_sub(v.start).min(n);
        if t < drawn { t } else { rows }
    } else {
        0
    };
    let top = v.top;
    super::side::paste(m, key, &mut out, foot, cw, top);
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

/// What a pinned row's ref starts with: a click on it goes to its turn.
pub const PIN_REF: &str = "pin:";

/// Whether turn t began with something you said (yourPrompt).
fn your_prompt(t: &rush_convo::Turn) -> bool {
    t.from.is_empty() && !t.prompt.trim().is_empty()
}

/// What you asked that you're reading the answer to, pinned over the top
/// row while it's scrolled off it: scrolled up, the current one; at the
/// end, the latest. Only with a body to spare (pinPrompts,
/// promptpins.go:31).
fn pin_prompt(v: &mut crate::session::SessionView, b: &mut [Line], w: usize) {
    let (start, end) = (v.start, v.start + b.len());
    let turns = &v.sess.turns;
    let Some(first) = turns.iter().position(your_prompt) else {
        return;
    };
    let off = |i: usize| v.r.turn_row(i) < start;
    let (mut latest, mut current) = (None, None);
    for i in (first + 1..turns.len()).rev() {
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
    let Some(i) = current.filter(|&i| off(i)) else {
        return;
    };
    if end - start < 1 + 6 {
        return; // a short pane keeps its rows for the conversation
    }
    let t = &turns[i];
    let text = one_line(&rush_convo::render::fold_pastes(&t.prompt));
    let row = line([
        sp("▍", BLUE),
        plain(" "),
        sp(label, FAINT),
        plain(" "),
        sp(text, TEXT),
    ]);
    b[0] = on_ground(fit(row, w), w, CHROME_BG);
    let r = format!("{PIN_REF}t{}", t.n);
    match v.shown.first_mut() {
        Some(s) => *s = r,
        None => v.shown.push(r),
    }
}

/// A click on a pinned prompt goes to its turn, picked, at the top
/// (clickRef's pin).
pub fn click_pin(v: &mut crate::session::SessionView, r: &str) -> bool {
    let Some(t) = r.strip_prefix(PIN_REF) else {
        return false;
    };
    let Some(n) = t.strip_prefix('t').and_then(|n| n.parse().ok()) else {
        return false;
    };
    v.sel = t.to_string();
    v.pos = Pos::At { n, off: 0 };
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
    let to = match e.kind {
        MouseKind::Down(Button::Left) => {
            let mm = &mut conn(m, &key).map;
            if !mm.hit(x, y) {
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
