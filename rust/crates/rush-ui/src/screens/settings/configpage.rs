//! Settings › Config: one table, harnesses down the side, providers
//! across. A harness switches on or off as a whole; a cell is where that
//! provider starts by default (★), one it may run in (●), one it may but
//! doesn't (○), or one it can't (·). enter on a cell picks the models it
//! may call there. Under the table, what rush does at a usage limit.
//! After Go's internal/ui/settings_config.go (cellPick :24, configPage
//! :31, configHarnesses :47, configProviders :61, cellMark :72, configHead
//! :112, configRows :147, configBody :180, configAbout :244, configKey
//! :303, cellModels :385, cellPickLines :395, cellPickKey :434).

use rush_core::adapter::Choice;
use rush_core::text::Line;
use rush_state::setup::{AT_LIMIT_ACCOUNT, AT_LIMIT_NEXT, AT_LIMIT_WAIT};

use super::acct::accounts;
use super::common::{agent_models, bill_word, fits, load_models, model_word, prov_label, reg};
use super::keys::controls;
use super::paint::trunc;
use super::workspace::{self, Geometry};
use super::{Page, config, config_mut, dialog, dialog_mut, settings_height};
use crate::frame::Out;
use crate::frame::picker::round_move;
use crate::frame::sheet::window;
use crate::model::Model;
use crate::style::*;

/// The models list of one cell, open under the table (cellPick).
#[derive(Clone, Debug, Default)]
pub struct CellPick {
    pub id: String,
    pub h: String,
    /// The agent that runs id in h.
    pub kind: String,
    /// 0 is "every model", then the models.
    pub cursor: usize,
}

/// The page.
pub fn page() -> Page {
    Page {
        name: "Config",
        pre: Some(|m, s| {
            if dialog(m).cell_pick.is_some() {
                return cell_pick_key(m, s).or_idle();
            }
            Out::Pass
        }),
        body: Some(body),
        key: Some(key),
        rows: Some(|m| harnesses(m).len() + 1),
        prep: Some(|m| {
            let (h, n) = (harnesses(m).len(), providers(m).len());
            let d = dialog_mut(m);
            d.cursor = d.cursor.min(h);
            d.config_col = d.config_col.min(n);
        }),
        ..Page::default()
    }
}

/// The table's rows: every harness some provider can run in; one none
/// can is left out (configHarnesses).
pub fn harnesses(m: &Model) -> Vec<String> {
    let ids = providers(m);
    reg()
        .harness_kinds()
        .into_iter()
        .filter(|h| {
            ids.iter().any(|id| {
                let (k, _, why) = fits(m, id, h.as_str());
                !k.is_empty() && why.is_empty()
            })
        })
        .map(|h| h.0)
        .collect()
}

/// The table's columns: every installed provider, the ones set up first
/// (configProviders).
pub fn providers(m: &Model) -> Vec<String> {
    super::providers::items(m)
}

/// A cell as the table shows it, and why it can't be, when it can't
/// (cellMark).
fn cell_mark(m: &Model, id: &str, h: &str) -> (Line, String) {
    let cfg = config(m);
    let (k, warn, why) = fits(m, id, h);
    if k.is_empty() {
        return (line([sp("·", FAINT)]), "it doesn't run there".into());
    }
    if !why.is_empty() {
        return (line([sp("·", FAINT)]), why);
    }
    let col = if warn.is_empty() { GREEN } else { YELLOW };
    let on = cfg.cell_on(id, h);
    let mut mark = if !cfg.harness_on(h) {
        line([sp("○", FAINT)])
    } else if cfg.harness_for(id) == h && on {
        line([sp("★", ORANGE)])
    } else if on {
        line([sp("●", col)])
    } else {
        line([sp("○", FAINT)])
    };
    let n = cfg.cell_models_of(id, h).len();
    if n > 0 {
        mark.spans.push(sp(format!(" {n}"), DIM));
    }
    (mark, String::new())
}

/// The harness column's width.
const NAME_W: usize = 19;
/// Each provider's.
const COL_W: usize = 10;

/// The table's two heading lines (configHead).
fn head(m: &Model, ids: &[String], from: usize, to: usize, in_table: bool, w: usize) -> Vec<Line> {
    let mut h1 = line([plain(blanks(NAME_W + 2))]);
    let mut h2 = line([plain(blanks(NAME_W + 2))]);
    for (i, id) in ids.iter().enumerate().take(to).skip(from) {
        let (pr, _) = reg().billed(id);
        let name = trunc(line([plain(reg().provider_label(&pr))]), COL_W - 1).plain();
        let mut sub = bill_word(id).to_string();
        if sub == "subscription" {
            sub = "plan".into();
        }
        if !super::providers::set_up(m, id) {
            sub = format!("{sub} ×").trim().to_string();
        }
        let st = if in_table && dialog(m).config_col == i + 1 {
            bold(ORANGE)
        } else {
            rush_core::text::Style::fg(DIM)
        };
        h1.spans
            .push(rush_core::text::Span::new(fit_str(&name, COL_W), st));
        h2.spans.push(sp(fit_str(&sub, COL_W), FAINT));
    }
    let mut more = String::new();
    if from > 0 {
        more = format!("‹ {from} more  ");
    }
    if to < ids.len() {
        more.push_str(&format!("{} more ›", ids.len() - to));
    }
    h1.spans.push(sp(more, FAINT));
    vec![fit(h1, w), h2]
}

/// The table's rows: each harness, on or off, then its cell for each
/// provider shown (configRows).
fn rows(m: &Model, hs: &[String], ids: &[String], from: usize, to: usize) -> Vec<Line> {
    let d = dialog(m);
    let cfg = config(m);
    let r = reg();
    let mut out = Vec::with_capacity(hs.len());
    for (row, h) in hs.iter().enumerate() {
        let label = fit_str(&r.harness_label(h), NAME_W - 2);
        let (dot, name) = if !cfg.harness_on(h) {
            (sp("○", FAINT), sp(label, FAINT))
        } else if !r.runs(h) {
            (sp("●", GREEN), sp(label, DIM))
        } else {
            (sp("●", GREEN), sp(label, TEXT))
        };
        let mut l = if row == d.cursor && d.config_col == 0 {
            line([sp("▍", ORANGE), plain(" ")])
        } else {
            line([plain("  ")])
        };
        l.spans.extend([dot, plain(" "), name]);
        for (i, id) in ids.iter().enumerate().take(to).skip(from) {
            let (mark, _) = cell_mark(m, id, h);
            if row == d.cursor && d.config_col == i + 1 {
                l.spans.push(sp("▸", ORANGE));
                let mut c = line([plain(" ")]);
                join(&mut c, mark);
                join(&mut l, fit(c, COL_W - 1));
            } else {
                let mut c = line([plain("  ")]);
                join(&mut c, mark);
                join(&mut l, fit(c, COL_W));
            }
        }
        out.push(l);
    }
    out
}

/// The page drawn (configBody).
fn body(m: &Model, _geo: &mut Geometry, w: usize, _h: usize) -> Vec<Line> {
    let d = dialog(m);
    let cfg = config(m);
    let (hs, ids) = (harnesses(m), providers(m));
    if hs.is_empty() || ids.is_empty() {
        return vec![line([sp(
            "No provider is installed yet: Settings › Providers says how to add one.",
            DIM,
        )])];
    }
    let cursor = d.cursor.min(hs.len());
    let col = d.config_col.min(ids.len());
    // Columns that fit, around the one in focus.
    let mut fits_n = (w as isize - NAME_W as isize - 4) / COL_W as isize;
    if fits_n < ids.len() as isize {
        fits_n = 1.max((w as isize - NAME_W as isize - 4 - 16) / COL_W as isize);
    }
    let (from, to) = window(ids.len(), col.saturating_sub(1), fits_n.max(0) as usize);

    let mut out = head(m, &ids, from, to, cursor < hs.len(), w);
    let mut table = rows(m, &hs, &ids, from, to);
    let mut legend = vec![Line::new()];
    for l in wrap(
        "● on   ○ off   ★ its default harness   · can't   ● yellow: on the harness's own sign-in   a number: only that many models",
        w.saturating_sub(2),
    ) {
        legend.push(line([sp(l, FAINT)]));
    }
    // What rush does at a limit.
    let limit = match cfg.at_limit.as_str() {
        AT_LIMIT_WAIT => "wait for the reset",
        AT_LIMIT_NEXT => "move on to the next provider with room",
        _ => "another account of the same provider",
    };
    let row = fit(
        line([
            sp("At a usage limit  ", DIM),
            sp("‹ ", FAINT),
            sp(limit, TEXT),
            sp(" ›", FAINT),
        ]),
        w.saturating_sub(2),
    );
    let mut tail = vec![Line::new(), cat2(line([plain("  ")]), row.clone())];
    if cursor == hs.len() {
        tail[1] = highlight(cat2(line([sp("▍", ORANGE), plain(" ")]), row), w);
    }

    // What's under it: the cell's models, or what's in focus. The page
    // fits the screen: the legend goes first, then the table scrolls.
    let avail =
        6.max(settings_height(m) as isize - workspace::heading(m, w).len() as isize) as usize;
    let mut under = vec![Line::new()];
    if let Some(p) = &d.cell_pick {
        let room = 3.max(
            avail as isize
                - out.len() as isize
                - table.len().min(4) as isize
                - tail.len() as isize
                - 4,
        ) as usize;
        under.extend(cell_pick_lines(m, p, w, room));
    } else {
        under.extend(about(m, &hs, &ids, w));
    }
    if out.len() + table.len() + legend.len() + tail.len() + under.len() > avail {
        legend.clear();
    }
    let extra = (out.len() + table.len() + tail.len() + under.len()) as isize - avail as isize;
    if extra > 0 {
        let keep = 1.max(table.len() as isize - extra) as usize;
        let (f, t) = window(table.len(), cursor.min(table.len() - 1), keep);
        table = table[f..t].to_vec();
    }
    out.extend(table);
    out.extend(legend);
    out.extend(tail);
    out.extend(under);
    out.truncate(avail);
    out
}

fn cat2(mut a: Line, b: Line) -> Line {
    join(&mut a, b);
    a
}

/// What the part in focus is, and its keys (configAbout).
fn about(m: &Model, hs: &[String], ids: &[String], w: usize) -> Vec<Line> {
    let d = dialog(m);
    let cfg = config(m);
    let r = reg();
    let (title, what, keys): (String, String, Vec<&str>);
    if d.cursor >= hs.len() {
        title = "At a usage limit".into();
        what = "What rush does when the account a session runs on is nearly out. Moving on hands the conversation, with what it was doing, to the next provider set up here that has room, and new sessions start there until the first resets.".into();
        keys = vec!["←→", "change"];
    } else if d.config_col == 0 {
        let h = &hs[d.cursor];
        title = r.harness_label(h);
        let mut w2 = format!("{title} is on: rush may start sessions in it.");
        if !cfg.harness_on(h) {
            w2 = format!(
                "{title} is off: rush starts no session in it, nor runs a subagent in it; its providers start in their next harness that's on."
            );
        }
        if !r.runs(h) {
            w2.push_str(" It isn't installed here.");
        }
        what = w2;
        keys = vec!["space", "on or off", "enter", "its settings", "→", "cells"];
    } else {
        let h = &hs[d.cursor];
        let id = &ids[(d.config_col - 1).min(ids.len() - 1)];
        let hl = r.harness_label(h);
        let pl = prov_label(id);
        title = format!("{pl} in {hl}");
        let (_, warn, _) = fits(m, id, h);
        let (_, why) = cell_mark(m, id, h);
        let mut w2 = if !why.is_empty() {
            format!("It can't: {why}.")
        } else if !cfg.harness_on(h) {
            format!("{hl} is off.")
        } else if cfg.harness_for(id) == *h && cfg.cell_on(id, h) {
            format!(
                "★ {pl} starts here by default, and {hl} may call its models for the chats it leads and their subagents."
            )
        } else if cfg.cell_on(id, h) {
            format!(
                "On: {hl} may call {pl}'s models, for the chats it leads and their subagents; by default {pl} starts in {}.",
                r.harness_label(&cfg.harness_for(id))
            )
        } else {
            "Off: it could run here; space switches it on.".into()
        };
        if !warn.is_empty() && why.is_empty() {
            w2.push_str(&format!(" It {warn}."));
        }
        let mut k = Vec::new();
        if why.is_empty() {
            let ms = cfg.cell_models_of(id, h);
            if ms.is_empty() {
                w2.push_str(" It may call every model.");
            } else {
                w2.push_str(&format!(" It may call {}.", ms.join(", ")));
            }
            k = vec![
                "space",
                "on or off",
                "*",
                "its default harness",
                "enter",
                "models",
            ];
        }
        what = w2;
        keys = k;
    }
    let mut out = vec![rule(&format!("About {title}"), "", w)];
    for l in wrap(&what, w.saturating_sub(4)) {
        out.push(line([plain("  "), sp(l, SUB)]));
    }
    out.push(Line::new());
    let mut pairs: Vec<(&str, &str)> = keys.chunks(2).map(|p| (p[0], p[1])).collect();
    pairs.extend([("[ ]", "page"), ("esc", "back")]);
    out.extend(controls(w, &pairs));
    out
}

/// A key on the page (configKey).
fn key(m: &mut Model, s: &str) -> Out {
    let (hs, ids) = (harnesses(m), providers(m));
    if hs.is_empty() || ids.is_empty() {
        return Out::Idle;
    }
    let save = |m: &mut Model| {
        super::changed(m);
        (accounts().spill_to)(m);
    };
    let d = dialog(m);
    let (cursor, col) = (d.cursor.min(hs.len()), d.config_col.min(ids.len()));
    if cursor >= hs.len() {
        let vals = [AT_LIMIT_ACCOUNT, AT_LIMIT_WAIT, AT_LIMIT_NEXT];
        let dir = match s {
            "right" | "l" | "enter" | "space" => 1,
            "left" | "h" => -1,
            _ => 0,
        };
        if dir != 0 {
            let cfg = config_mut(m);
            let at = vals.iter().position(|v| *v == cfg.at_limit).unwrap_or(0);
            cfg.at_limit = vals[round_move(at, dir, vals.len())].into();
            cfg.sync_legacy();
            save(m);
            return Out::Draw;
        }
        return Out::Idle;
    }
    let h = hs[cursor].clone();
    match s {
        "left" | "h" => {
            let d = dialog_mut(m);
            d.config_col = col.saturating_sub(1);
            return Out::Draw;
        }
        "right" | "l" => {
            let d = dialog_mut(m);
            d.config_col = ids.len().min(col + 1);
            return Out::Draw;
        }
        _ => {}
    }
    if col == 0 {
        match s {
            "space" => {
                let cfg = config_mut(m);
                let on = cfg.harness_on(&h);
                cfg.set_harness_on(&h, !on);
                save(m);
                return Out::Draw;
            }
            "enter" => {
                super::harnesses::open_harness(m, &h);
                return Out::Draw;
            }
            _ => return Out::Idle,
        }
    }
    let id = ids[col - 1].clone();
    if !matches!(s, "space" | "*" | "enter") {
        return Out::Idle;
    }
    let (k, _, why) = fits(m, &id, &h);
    let hl = reg().harness_label(&h);
    if k.is_empty() || !why.is_empty() {
        let (_, why) = cell_mark(m, &id, &h);
        m.flash_err(&format!("{} in {hl}: {why}", prov_label(&id)));
        return Out::Draw;
    }
    match s {
        "space" => {
            if config(m).star_harness(&id) == h {
                m.flash(&format!(
                    "{} starts here by default · * on another cell moves that first",
                    prov_label(&id)
                ));
                return Out::Draw;
            }
            let cfg = config_mut(m);
            let uses = cfg.uses(&id, &h);
            cfg.set_uses(&id, &h, !uses);
            save(m);
        }
        "*" => {
            let cfg = config_mut(m);
            cfg.set_runs_in(&id, &h);
            cfg.set_uses(&id, &h, false); // the ★ is on by being it
            if !cfg.harness_on(&h) {
                cfg.set_harness_on(&h, true);
            }
            save(m);
            m.flash(&format!("{} starts in {hl} by default", prov_label(&id)));
        }
        _ => {
            dialog_mut(m).cell_pick = Some(CellPick {
                id,
                h,
                kind: k.clone(),
                cursor: 0,
            });
            load_models(m, &k);
        }
    }
    Out::Draw
}

/// What cell p's agent offers, then any it's kept to that the list lacks
/// (cellModels).
fn cell_models(m: &Model, p: &CellPick) -> Vec<Choice> {
    let mut out = agent_models(m, &p.kind);
    for v in config(m).cell_models_of(&p.id, &p.h) {
        if !out.iter().any(|c| c.id == v) {
            out.push(Choice {
                id: v,
                ..Choice::default()
            });
        }
    }
    out
}

/// A cell's models, h tall (cellPickLines).
fn cell_pick_lines(m: &Model, p: &CellPick, w: usize, h: usize) -> Vec<Line> {
    let cfg = config(m);
    let kept = cfg.cell_models_of(&p.id, &p.h);
    let def = cfg.dispatch.start_on(&p.id, &p.kind).model;
    let mut out = vec![rule(
        &format!(
            "Models · {} in {}",
            prov_label(&p.id),
            reg().harness_label(&p.h)
        ),
        "the ones it may call; ★ starts with",
        w,
    )];
    let mut rows = vec![line([
        if kept.is_empty() {
            sp("●", GREEN)
        } else {
            sp("○", FAINT)
        },
        plain(" "),
        sp("every model", TEXT),
    ])];
    let models = cell_models(m, p);
    for c in &models {
        let mark = if kept.is_empty() || kept.contains(&c.id) {
            sp("●", GREEN)
        } else {
            sp("○", FAINT)
        };
        let star = if c.id == def {
            sp("★ ", ORANGE)
        } else {
            plain("  ")
        };
        rows.push(line([
            mark,
            plain(" "),
            star,
            sp(fit_str(&model_word(&p.kind, &c.id), 28), TEXT),
            plain(" "),
            sp(c.note.clone(), FAINT),
        ]));
    }
    if models.is_empty() {
        rows.push(line([sp(
            format!(
                "  {} lists no models here; it starts on its own default",
                reg().harness_label(&p.h)
            ),
            FAINT,
        )]));
    }
    let cursor = p.cursor.min(models.len());
    let (from, to) = window(rows.len(), cursor, h);
    for (i, r) in rows.into_iter().enumerate().take(to).skip(from) {
        if i == cursor {
            out.push(highlight(cat2(line([sp("▍", ORANGE), plain(" ")]), r), w));
        } else {
            out.push(cat2(line([plain("  ")]), r));
        }
    }
    out.push(Line::new());
    out.extend(controls(
        w,
        &[
            ("↑↓", "model"),
            ("space", "may call it or not"),
            ("*", "starts with it"),
            ("esc", "done"),
        ],
    ));
    out
}

/// A key while a cell's models are open (cellPickKey).
fn cell_pick_key(m: &mut Model, s: &str) -> Out {
    let Some(mut p) = dialog(m).cell_pick.clone() else {
        return Out::Pass;
    };
    let models = cell_models(m, &p);
    p.cursor = p.cursor.min(models.len());
    match s {
        "esc" | "enter" | "q" | "backspace" | "left" => {
            dialog_mut(m).cell_pick = None;
            return Out::Draw;
        }
        "up" | "k" => p.cursor = p.cursor.saturating_sub(1),
        "down" | "j" => p.cursor = models.len().min(p.cursor + 1),
        "space" | " " => {
            let cfg = config_mut(m);
            if p.cursor == 0 {
                cfg.set_cell_models(&p.id, &p.h, Vec::new());
            } else {
                let v = models[p.cursor - 1].id.clone();
                let mut kept = cfg.cell_models_of(&p.id, &p.h);
                if kept.is_empty() {
                    // From every model to every one but this.
                    kept = models
                        .iter()
                        .filter(|c| c.id != v)
                        .map(|c| c.id.clone())
                        .collect();
                } else if kept.contains(&v) {
                    kept.retain(|x| *x != v);
                    if kept.is_empty() {
                        m.flash_err("it has to call one: space on every model gives it all again");
                        return Out::Draw;
                    }
                } else {
                    kept.push(v);
                    if kept.len() == models.len() {
                        kept.clear();
                    }
                }
                cfg.set_cell_models(&p.id, &p.h, kept);
            }
            super::changed(m);
        }
        "*" => {
            if p.cursor == 0 {
                return Out::Draw;
            }
            let mut v = models[p.cursor - 1].id.clone();
            let cfg = config_mut(m);
            let mut st = cfg.dispatch.start_on(&p.id, &p.kind);
            if st.model == v {
                v.clear();
            }
            st.model = v.clone();
            cfg.dispatch.set_start_on(&p.id, &p.kind, st);
            if !v.is_empty() && !cfg.calls(&p.id, &p.h, &v) {
                let mut ms = cfg.cell_models_of(&p.id, &p.h);
                ms.push(v);
                cfg.set_cell_models(&p.id, &p.h, ms);
            }
            super::changed(m);
        }
        _ => return Out::Idle,
    }
    dialog_mut(m).cell_pick = Some(p);
    Out::Draw
}
