//! A page laid out as a catalog: a list, and beside it (or under it on a
//! narrow screen) everything about the line picked, which scrolls on its
//! own. Providers and Harnesses are catalogs. After Go's
//! internal/ui/settings_catalog_layout.go (catalogGeometry :13,
//! catalogKey :21, catalogBody :52, catalogList :153, catalogDetail
//! :186, catalogMouse :271, catalogWheel :318, formLineIndices :356).

use rush_core::text::Line;

use super::common::{feature_summary, frame_cursor, model_takes, pairing_support, reg};
use super::form::{self, Section, flat, row_at};
use super::paint::trunc;
use super::workspace::Geometry;
use super::{PAGE_HARNESSES, PAGE_PROVIDERS, cur_page, dialog, dialog_len, dialog_mut};
use crate::frame::Out;
use crate::frame::sheet::window;
use crate::model::Model;
use crate::style::*;

/// Where a catalog drew its list and detail, in the body's cells
/// (catalogGeometry).
#[derive(Clone, Debug, Default)]
pub struct CatGeo {
    pub x: usize,
    pub width: usize,
    pub list_width: usize,
    pub list_top: usize,
    pub detail_x: usize,
    pub detail_top: usize,
    pub wide: bool,
    /// The item on each list row drawn; None for a heading.
    pub list_rows: Vec<Option<usize>>,
    /// The setting on each detail row drawn.
    pub detail_rows: Vec<Option<usize>>,
    /// The setting on each of the detail's rows, drawn or not.
    pub all_details: Vec<Option<usize>>,
}

/// The rows the header takes (len(m.header())).
pub fn header_rows(m: &Model) -> usize {
    m.top_h().saturating_sub(2)
}

/// Keys a catalog takes before the page: home, end and paging move the
/// detail, not the list (catalogKey). Pass when it isn't one.
pub fn key(m: &mut Model, s: &str, inside: bool) -> Out {
    let step = (m.h as isize - header_rows(m) as isize - 14).clamp(1, 8);
    let n = dialog_len(m);
    let d = dialog_mut(m);
    let scroll = d.catalog_scroll.get();
    match s {
        "home" => {
            d.cursor = 0;
            d.catalog_scroll.set(0);
            Out::Draw
        }
        "end" => {
            d.cursor = n.saturating_sub(1);
            if inside {
                d.catalog_scroll.set(-1);
            }
            Out::Draw
        }
        "pgdown" => {
            d.catalog_scroll.set(scroll.max(0) + step);
            Out::Draw
        }
        "pgup" => {
            d.catalog_scroll.set((scroll - step).max(0));
            Out::Draw
        }
        "up" | "down" | "j" | "k" => {
            d.catalog_scroll.set(if inside { -1 } else { 0 });
            Out::Pass
        }
        _ => Out::Pass,
    }
}

/// The detail of the line picked, dw wide: its rows, where its cursor
/// is, and the setting on each of its rows.
pub type Detail<'a> = dyn FnMut(usize) -> (Vec<Line>, usize, Vec<Option<usize>>) + 'a;

/// The catalog drawn (catalogBody). controls are the key line's own keys
/// (a page that has its own, Providers', says them).
#[allow(clippy::too_many_arguments)]
pub fn body(
    m: &Model,
    geo: &mut Geometry,
    catalog: &[Section],
    picked: usize,
    inside: bool,
    accounts: bool,
    w: usize,
    detail: &mut Detail<'_>,
    controls: Option<Vec<String>>,
) -> Vec<Line> {
    let d = dialog(m);
    geo.cat = CatGeo::default();
    let height = 6.max(m.h as isize - header_rows(m) as isize - 6) as usize;
    let mut pane_h = 2.max(height as isize - 1) as usize;
    let wide = if accounts { w >= 100 } else { w >= 72 };
    let (mut lw, mut dw) = (w, w);
    if wide {
        lw = 36.min(24.max(w / 4));
        dw = w.saturating_sub(lw + 3);
    }
    let mut catalog = catalog.to_vec();
    if let Some(first) = catalog.first_mut()
        && first.title == cur_page(m).name
    {
        first.title.clear();
    }
    let (mut list, mut list_cursor, mut list_indices) = list(&catalog, picked, inside, lw);
    let (rows, detail_cursor, all_details) = detail(dw);
    geo.cat.all_details = all_details;
    if !wide {
        // A short neighbourhood leaves a useful detail preview even on a
        // narrow terminal.
        let mut list_h = 3.min(list.len());
        if inside {
            list_h = 1;
            list = vec![line([
                sp("▸ ", ORANGE),
                plain(row_at(&catalog, picked).label),
            ])];
            list_cursor = 0;
            list_indices = vec![Some(picked)];
        }
        let (first, last) = window(list.len(), list_cursor, list_h);
        list = list[first..last].to_vec();
        list_indices = list_indices[first..last].to_vec();
        pane_h = 2.max(pane_h as isize - list.len() as isize - 1) as usize;
    }
    let max_scroll = rows.len().saturating_sub(pane_h) as isize;
    let mut scroll = d.catalog_scroll.get();
    if scroll < 0 {
        scroll = 0.max(detail_cursor as isize - pane_h as isize + 2);
    }
    let scroll = scroll.clamp(0, max_scroll) as usize;
    d.catalog_scroll.set(scroll as isize);
    let end = rows.len().min(scroll + pane_h);
    let shown = &rows[scroll.min(end)..end];
    let g = &mut geo.cat;
    g.x = 0;
    g.width = w;
    g.list_width = lw;
    g.wide = wide;
    g.list_top = 2;
    g.detail_top = g.list_top;
    g.detail_x = lw + 3;
    if !wide {
        g.detail_top += list.len() + 1;
        g.detail_x = 0;
    }
    if g.all_details.len() == rows.len() {
        g.detail_rows = g.all_details[scroll.min(end)..end].to_vec();
    }
    let mut out = Vec::new();
    if wide {
        let (first, last) = window(list.len(), list_cursor, pane_h);
        let list = &list[first..last];
        list_indices = list_indices[first..last].to_vec();
        for i in 0..pane_h {
            let mut l = fit(list.get(i).cloned().unwrap_or_default(), lw);
            l.spans.push(sp(" │ ", FAINT));
            join(&mut l, fit(shown.get(i).cloned().unwrap_or_default(), dw));
            out.push(l);
        }
    } else {
        out.extend(list);
        out.push(line([sp("─".repeat(w), FAINT)]));
        for l in shown {
            out.push(fit(l.clone(), w));
        }
    }
    let mut keys: Vec<String> = match controls {
        Some(c) => c,
        None if inside => ["↑↓", "select", "←→", "change", "enter", "action"]
            .map(String::from)
            .to_vec(),
        None => ["↑↓", "select", "→", "controls"].map(String::from).to_vec(),
    };
    if rows.len() > pane_h {
        let mut k = vec![
            "PgUp/Dn".to_string(),
            format!("detail {}/{}", scroll + 1, rows.len()),
        ];
        k.extend(keys);
        keys = k;
    }
    keys.extend(["[ ]", "pages", "esc", "back"].map(String::from));
    let pairs: Vec<(&str, &str)> = keys
        .chunks(2)
        .filter(|p| p.len() == 2)
        .map(|p| (p[0].as_str(), p[1].as_str()))
        .collect();
    out.push(keys_fit(w, &pairs));
    geo.cat.list_rows = list_indices;
    out
}

/// The list of a catalog, the line picked marked (catalogList): its rows,
/// the row picked, and the item on each row.
pub fn list(
    secs: &[Section],
    picked: usize,
    inside: bool,
    w: usize,
) -> (Vec<Line>, usize, Vec<Option<usize>>) {
    let (mut rows, mut indices) = (Vec::new(), Vec::new());
    let (mut cursor, mut index) = (0, 0);
    for sec in secs {
        if !sec.title.is_empty() {
            rows.push(trunc(line([spb(sec.title.clone(), SUB)]), w));
            indices.push(None);
        }
        for row in &sec.rows {
            let full = if row.painted.width() > 0 {
                row.painted.clone()
            } else {
                line([plain(row.label.clone())])
            };
            let mut label = trunc(full, 1.max(w as isize - 2) as usize);
            if row.tag.width() > 0 {
                let mut l = fit(
                    label,
                    1.max(w as isize - 4 - row.tag.width() as isize) as usize,
                );
                l.spans.push(plain("  "));
                join(&mut l, row.tag.clone());
                label = l;
            }
            let l = if index == picked {
                cursor = rows.len();
                let text = recolour(label, TEXT);
                if inside {
                    let mut l = line([sp("▸ ", ORANGE)]);
                    join(&mut l, text);
                    l
                } else {
                    let mut l = line([sp("▍ ", ORANGE)]);
                    join(&mut l, text);
                    highlight(l, w)
                }
            } else {
                let mut l = line([plain("  ")]);
                join(&mut l, label);
                l
            };
            rows.push(l);
            indices.push(Some(index));
            index += 1;
        }
    }
    (rows, cursor, indices)
}

/// l painted c as Go's paint(c, label) shows it: the uncoloured spans
/// take c up to the first span of its own colour, whose reset leaves the
/// rest in the terminal's own colour.
fn recolour(mut l: Line, c: rush_core::text::Color) -> Line {
    for s in &mut l.spans {
        if s.style.fg != rush_core::text::Color::Default {
            break;
        }
        s.style.fg = c;
    }
    l
}

/// The detail of a harness or pairing: its name, the model and what it
/// takes, the form, what the row picked does, and what rush can do with
/// it (catalogDetail).
pub fn detail(
    m: &Model,
    secs: &[Section],
    k: &str,
    model: &str,
    inside: bool,
    w: usize,
) -> (Vec<Line>, usize, Vec<Option<usize>>) {
    // The header already names this pairing; its first form title repeats it.
    let mut secs = secs.to_vec();
    if let Some(s) = secs.first_mut() {
        s.title.clear();
        s.note.clear();
    }
    let r = reg();
    let mut head = vec![line([spb(r.harness_label(k), TEXT)])];
    if !model.is_empty() {
        head.push(line([sp(model.to_string(), BRIGHT)]));
        head.push(model_takes(k, model));
    }
    if !r.runs(k) {
        head.push(line([sp("Not installed · ", YELLOW), sp(r.hint(k), DIM)]));
    }
    let n = flat(&secs).len();
    let cur = if inside {
        Some(dialog(m).cursor.min(n.saturating_sub(1)))
    } else {
        None
    };
    let head_len = head.len();
    let mut rows = head;
    rows.extend(form::form_rows(&secs, cur.unwrap_or(usize::MAX), w));
    let mut cursor = 0;
    if let Some(cur) = cur {
        cursor = frame_cursor(&rows);
        let selected = row_at(&secs, cur);
        if !selected.what.is_empty() {
            rows.push(Line::new());
            rows.push(trunc(line([sp(selected.what, DIM)]), w));
        }
    }
    rows.push(Line::new());
    rows.push(rule("What it can do", "", w));
    rows.extend(feature_summary(|f| pairing_support(k, model, f), w));
    let indices = line_indices(&secs, head_len, rows.len());
    (rows, cursor, indices)
}

/// Each line of a form drawn under head lines of something else: the
/// setting on it (formLineIndices).
pub fn line_indices(secs: &[Section], head: usize, total: usize) -> Vec<Option<usize>> {
    let mut out = vec![None; total];
    let (mut at, mut index) = (head, 0);
    for sec in secs {
        at += 1;
        if !sec.title.is_empty() {
            at += 1;
        }
        for _ in &sec.rows {
            if at < total {
                out[at] = Some(index);
            }
            at += 1;
            index += 1;
        }
    }
    out
}

/// A click on a catalog (catalogMouse), in the body's cells.
pub fn mouse(m: &mut Model, geo: &Geometry, x: usize, y: usize) -> Out {
    let page = dialog(m).page;
    if !dialog(m).asking.is_empty() || (page != PAGE_PROVIDERS && page != PAGE_HARNESSES) {
        return Out::Idle;
    }
    let g = &geo.cat;
    if x < g.x || x >= g.x + g.width {
        return Out::Idle;
    }
    if y >= g.list_top && y < g.list_top + g.list_rows.len() && (!g.wide || x < g.x + g.list_width)
    {
        let Some(at) = g.list_rows[y - g.list_top] else {
            return Out::Idle;
        };
        let d = dialog(m);
        if !d.harness_inside && !d.inside && d.cursor == at {
            if let Some(k) = cur_page(m).key {
                return k(m, "enter");
            }
            return Out::Idle;
        }
        let d = dialog_mut(m);
        d.harness_inside = false;
        d.inside = false;
        d.cursor = at;
        d.catalog_scroll.set(0);
        return Out::Draw;
    }
    if x < g.detail_x || y < g.detail_top || y >= g.detail_top + g.detail_rows.len() {
        return Out::Idle;
    }
    let Some(at) = g.detail_rows[y - g.detail_top] else {
        return Out::Idle;
    };
    if page != PAGE_HARNESSES {
        let d = dialog(m);
        if d.inside && d.cursor == at {
            return super::providers::key(m, "enter");
        }
        let d = dialog_mut(m);
        if !d.inside {
            d.pick = d.cursor;
            d.inside = true;
        }
        d.cursor = at;
        return Out::Draw;
    }
    if dialog(m).harness_inside && dialog(m).cursor == at {
        // Every harness's rows when it's open, else the harness's own.
        let secs = super::harnesses::sections(m);
        return form::form_key(m, &secs, "enter");
    }
    let d = dialog_mut(m);
    if !d.harness_inside {
        d.harness_return = d.cursor;
        d.harness_inside = true;
    }
    d.cursor = at;
    Out::Draw
}

/// The wheel on a catalog (catalogWheel).
pub fn wheel(m: &mut Model, geo: &Geometry, x: usize, y: usize, delta: isize) -> Out {
    let page = dialog(m).page;
    if !dialog(m).asking.is_empty() || (page != PAGE_PROVIDERS && page != PAGE_HARNESSES) {
        return Out::Idle;
    }
    let g = &geo.cat;
    if x < g.x || x >= g.x + g.width {
        return Out::Idle;
    }
    if y >= g.list_top && y < g.list_top + g.list_rows.len() && (!g.wide || x < g.x + g.list_width)
    {
        let d = dialog(m);
        let (at, n) = if page != PAGE_HARNESSES {
            let at = if d.inside { d.pick } else { d.cursor };
            (at, super::providers::items(m).len())
        } else {
            let at = if d.harness_inside {
                d.harness_return
            } else {
                d.cursor
            };
            (at, flat(&super::harnesses::catalog_sections(m)).len())
        };
        let d = dialog_mut(m);
        d.harness_inside = false;
        d.inside = false;
        d.cursor = (at as isize + delta).clamp(0, (n as isize - 1).max(0)) as usize;
        d.catalog_scroll.set(0);
        return Out::Draw;
    }
    if x >= g.detail_x && y >= g.detail_top {
        let d = dialog_mut(m);
        d.catalog_scroll
            .set((d.catalog_scroll.get() + delta).max(0));
        return Out::Draw;
    }
    Out::Idle
}
