//! The conversation as the Session draws it: the rows in view with the
//! item under the pointer lit and the dragged-over text painted, a pill
//! saying how much is below once scrolled up, and the map beside it from
//! 96 wide. After Go's internal/ui/rushmode.go (paneView's conversation
//! :2239-2245, :2420-2432, the pill :2604-2611, the map :2643-2660,
//! withMap :2690) and pane_pointer.go (ownerAt :89, paintHover :111).

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
        vec![line([sp("reading the conversation…", DIM)])]
    } else if !v.err.is_empty() && v.sess.turns.is_empty() {
        vec![line([sp(format!("couldn't read it: {}", v.err), RED)])]
    } else if v.sess.turns.is_empty() {
        vec![line([sp("  nothing yet · type below to start", DIM)])]
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
        if v.pos != Pos::End {
            let total = v.r.count(&v.sess, &v.o);
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
        mm.end = v.start + n;
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
