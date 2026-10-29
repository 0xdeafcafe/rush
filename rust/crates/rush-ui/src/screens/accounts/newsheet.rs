//! The New session sheet (⌥m, shift+tab in the box): the harness that
//! leads the chat, then a model it may call. A chat has one leader
//! harness, and it controls the models: the list is its row of Settings ›
//! Config, every provider it runs and the models each may call there. It
//! changes no default: Settings does. After Go's internal/ui/newsheet.go
//! (newPick :22, newSheet :26, openComposer :45, leaders :67, newPicks
//! :80, keepPick :104, start :124, shown :134, word :151, body :163, key
//! :218, use :290, mouse :302, listLines :339), shelf.go:122 (shelfPick),
//! settings_providers.go (provItems :60, provSetUp :85),
//! settings_config.go:63 (configProviders) and startsheet.go:211
//! (loadAllModels).

use rush_core::adapter::{Feature, Kind};
use rush_core::text::Line;

use super::bits::keys_controls;
use super::look::{glyph, harness_name, model_word, pays_by_key, prov_kind, prov_label, prov_of};
use super::rows::{agent_order, start_dir};
use super::setup::{
    StartOver, by_profile, models, next_start, route_rows, start_as, start_id, start_on,
    start_with_words, start_words,
};
use super::state::{accounts, cfg, ensure_store, peek};
use crate::frame::overlayclick::MouseEv;
use crate::frame::picker::round_move;
use crate::frame::sheet::{Sheet, sheet_row, sheet_title, window};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// One model on the sheet's list: provider id's model ("" its default).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewPick {
    pub id: String,
    pub model: String,
}

/// A line of the body a click takes: a row, or a model.
#[derive(Clone, Copy, Debug)]
struct Hit {
    y: usize,
    row: usize,
    pick: Option<usize>,
}

pub struct NewSheet {
    /// 0 the harness, 1 the model.
    pub row: usize,
    /// The leader.
    pub harness: Kind,
    /// The model chosen.
    pub pick: NewPick,
    /// The model list is open.
    pub open: bool,
    /// The line of the list in focus.
    pub sel: usize,
    /// What's typed to narrow the list.
    pub filter: String,
    hits: Vec<Hit>,
}

/// Opens the sheet on what the next session would start as: its harness
/// leading, on its model when that harness may call it (openComposer).
pub fn open_composer(m: &mut Model) -> Out {
    ensure_store(m);
    let o = next_start(m, &start_dir(m));
    let leaders = leaders(m);
    if leaders.is_empty() {
        m.flash_err("rush can't run any harness here yet · Settings › Config");
        return Out::Draw;
    }
    let mut s = NewSheet {
        row: 1,
        harness: m.reg.harness_of(&o.kind),
        pick: NewPick {
            id: start_id(&m.reg, &o),
            model: o.model.clone(),
        },
        open: false,
        sel: 0,
        filter: String::new(),
        hits: Vec::new(),
    };
    if !leaders.contains(&s.harness) {
        s.harness = leaders[0].clone();
    }
    s.keep_pick(m);
    m.open_sheet(Box::new(s));
    load_all_models(m);
    Out::Draw
}

/// Reads every installed agent's models off the UI, for the sheets'
/// other routes (loadAllModels, readSettingsModels).
pub fn load_all_models(m: &mut Model) {
    let reg = m.reg.clone();
    m.later(
        move || {
            let mut out = Vec::new();
            for a in reg.installed_all() {
                let k = a.kind();
                if reg.current_kind(k.as_str()) != k {
                    continue;
                }
                if let Some(l) = a.model_lister()
                    && let Some(p) = a.profiles().into_iter().next()
                {
                    out.push((k.0, l.list_models(&p)));
                }
            }
            out
        },
        |m, found| {
            let a = accounts(m);
            for (k, v) in found {
                let e = a.listed.entry(k).or_default();
                if e.is_empty() {
                    *e = v;
                }
            }
            Out::Draw
        },
    );
}

/// Whether provider id is ready to use: its harness runs here, and its
/// key is kept or it's signed in (provSetUp).
pub fn prov_set_up(m: &Model, id: &str) -> bool {
    let k = prov_kind(&m.reg, cfg(m), id);
    if !m.reg.runs(k.as_str()) {
        return false;
    }
    if pays_by_key(&m.reg, id) {
        return cfg(m).has_api_key(&prov_of(&m.reg, id));
    }
    peek(m).why.get(k.as_str()).is_none_or(|w| w.is_empty())
}

/// Every installed provider, the ones set up first (provItems,
/// configProviders).
pub fn config_providers(m: &Model) -> Vec<String> {
    let (mut ready, mut rest) = (Vec::new(), Vec::new());
    for ad in agent_order(m) {
        let pr = m.reg.provider_of(ad.kind().as_str());
        let mut ids = vec![pr.clone()];
        if m.reg.split(&pr) {
            ids.push(rush_core::adapter::key_of(&pr));
        }
        for id in ids {
            if ready.contains(&id) || rest.contains(&id) {
                continue;
            }
            if prov_set_up(m, &id) {
                ready.push(id);
            } else {
                rest.push(id);
            }
        }
    }
    ready.extend(rest);
    ready
}

/// The harnesses that can lead a chat now: on, with a provider it may run
/// that's ready (leaders).
pub fn leaders(m: &Model) -> Vec<Kind> {
    let c = cfg(m);
    let mut out: Vec<Kind> = Vec::new();
    for r in route_rows(m) {
        if r.why.is_empty() && c.cell_on(&r.id, r.harness.as_str()) && !out.contains(&r.harness) {
            out.push(r.harness);
        }
    }
    out
}

/// The models harness h may call: its row of Config, by provider, the
/// ones set up first (newPicks).
pub fn new_picks(m: &Model, h: &Kind) -> Vec<NewPick> {
    let c = cfg(m);
    let ready = route_rows(m);
    let mut out: Vec<NewPick> = Vec::new();
    for id in config_providers(m) {
        if !c.cell_on(&id, h.as_str())
            || !ready
                .iter()
                .any(|r| r.id == id && r.harness == *h && r.why.is_empty())
        {
            continue;
        }
        let (k, _, _) = m.reg.compat(&id, h.as_str());
        let n = out.len();
        for ch in models(m, k.as_str()) {
            let p = NewPick {
                id: id.clone(),
                model: ch.id.clone(),
            };
            if c.calls(&id, h.as_str(), &ch.id) && !out.contains(&p) {
                out.push(p);
            }
        }
        if out.len() == n {
            // It lists none: its own default.
            out.push(NewPick {
                id: id.clone(),
                model: String::new(),
            });
        }
    }
    out
}

/// What a harness may lack that's worth saying before it starts
/// (cantFeatures).
const CANT: [(Feature, &str); 4] = [
    (Feature::Rewind, "rewind"),
    (Feature::Images, "images"),
    (Feature::Resume, "resume"),
    (Feature::Subagents, "subagents"),
];

impl NewSheet {
    /// Keeps the model when the leader may call it, else takes the one its
    /// first provider starts with (keepPick).
    fn keep_pick(&mut self, m: &Model) {
        let picks = new_picks(m, &self.harness);
        if picks.contains(&self.pick) || picks.is_empty() {
            return;
        }
        let mut id = picks[0].id.clone();
        if picks.iter().any(|p| p.id == self.pick.id) {
            id = self.pick.id.clone(); // same provider, another model
        }
        let (k, _, _) = m.reg.compat(&id, self.harness.as_str());
        self.pick = NewPick {
            id: id.clone(),
            model: start_on(m, &id, k.as_str()).model,
        };
        if !picks.contains(&self.pick) {
            let i = picks.iter().position(|p| p.id == id).unwrap_or(0);
            self.pick = picks[i].clone();
        }
    }

    /// What the sheet starts: the model in the leader, with what Settings
    /// gives that route for the rest (start).
    pub fn start(&self, m: &Model) -> StartOver {
        let (k, _, _) = m.reg.compat(&self.pick.id, self.harness.as_str());
        let mut o = start_on(m, &self.pick.id, k.as_str());
        if !self.pick.model.is_empty() {
            o.model = self.pick.model.clone();
        }
        o
    }

    /// The list's lines that match what's typed (shown).
    fn shown(&self, m: &Model) -> Vec<NewPick> {
        let all = new_picks(m, &self.harness);
        let q = self.filter.trim().to_lowercase();
        if q.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|p| {
                format!("{} {}", self.word(m, p), prov_label(&m.reg, &p.id))
                    .to_lowercase()
                    .contains(&q)
            })
            .collect()
    }

    /// Model p in words (word).
    fn word(&self, m: &Model, p: &NewPick) -> String {
        if p.model.is_empty() {
            return "its default model".into();
        }
        let (k, _, _) = m.reg.compat(&p.id, self.harness.as_str());
        model_word(&m.reg, k.as_str(), &p.model)
    }

    /// Opens the model list on the model picked (openList).
    fn open_list(&mut self, m: &Model) {
        self.open = true;
        self.filter.clear();
        self.sel = self
            .shown(m)
            .iter()
            .position(|p| *p == self.pick)
            .unwrap_or(0);
    }

    /// Closes the sheet on what it starts: the typed task starts as it,
    /// else the next session will (use).
    fn use_it(&mut self, m: &mut Model) -> Out {
        let o = self.start(m);
        if !m.input.is_empty() {
            let msg = m.input.sent(true);
            m.input.clear();
            let r = start_as(m, o, &msg);
            return if r == Out::Quit { r } else { Out::Close };
        }
        shelf_pick(m, o);
        Out::Close
    }

    /// The list's lines after out, grouped by provider, as many as fit in h
    /// (listLines).
    fn list_lines(&mut self, m: &Model, out: &mut Vec<Line>, w: usize, h: usize) {
        out.push(Line::new());
        out.push(line([
            plain("  "),
            sp("find ", DIM),
            sp(self.filter.clone(), TEXT),
            sp("▏", ORANGE),
        ]));
        let list = self.shown(m);
        self.sel = self.sel.min(list.len().saturating_sub(1));
        let room = if h > 0 {
            3.max(h.saturating_sub(out.len() + 4))
        } else {
            1 << 20
        };
        let mut lines = Vec::new();
        let mut at: Vec<Option<usize>> = Vec::new();
        for (i, p) in list.iter().enumerate() {
            if i == 0 || list[i - 1].id != p.id {
                let mut l = line([plain("  ")]);
                join(
                    &mut l,
                    rule(&prov_label(&m.reg, &p.id), "", w.saturating_sub(4)),
                );
                lines.push(l);
                at.push(None);
            }
            // Two of one name: their ids tell them apart.
            let twin = list
                .iter()
                .any(|q| q != p && q.id == p.id && self.word(m, q) == self.word(m, p));
            let mut row = line([plain("  "), sp(self.word(m, p), TEXT)]);
            if twin {
                row.spans.push(sp(format!("  {}", p.model), FAINT));
            }
            lines.push(sheet_row(row, i == self.sel, w));
            at.push(Some(i));
        }
        if list.is_empty() {
            lines = vec![line([plain("  "), sp("no model matches", FAINT)])];
            at = vec![None];
        }
        let cur = at.iter().position(|a| *a == Some(self.sel)).unwrap_or(0);
        let (from, to) = window(lines.len(), cur, room);
        for i in from..to {
            if let Some(p) = at[i] {
                self.hits.push(Hit {
                    y: out.len(),
                    row: 0,
                    pick: Some(p),
                });
            }
            out.push(lines[i].clone());
        }
    }
}

/// Makes o what the next session starts as, or the profile's own again
/// when it's that (shelfPick).
pub fn shelf_pick(m: &mut Model, o: StartOver) {
    let dir = start_dir(m);
    let same = start_words(m, &o) == start_words(m, &by_profile(m, &dir));
    accounts(m).start_over = if same { None } else { Some(o) };
    let w = start_with_words(m, &dir);
    m.flash(&format!("the next session starts as {w}"));
}

impl Sheet for NewSheet {
    fn width(&self, m: &Model) -> usize {
        64.max(96.min(m.w.saturating_sub(6)))
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        self.hits.clear();
        let o = self.start(m);
        let k = o.kind.clone();
        let reg = &m.reg;
        let mut out = vec![
            sheet_title(
                "New session",
                "the harness that leads it, and a model it may call · Settings stay as they are",
                w,
            ),
            Line::new(),
        ];
        let hname = harness_name(reg, self.harness.as_str());
        let mut harness = Line::new();
        let many = leaders(m).len() > 1;
        if many {
            harness.spans.push(sp("‹ ", FAINT));
        }
        harness.spans.push(glyph(reg, self.harness.as_str()));
        harness.spans.push(plain(" "));
        harness.spans.push(spb(hname, TEXT));
        if many {
            harness.spans.push(sp(" ›", FAINT));
        }
        harness
            .spans
            .push(sp("  leads; its subagents run on its models too", DIM));
        let (pr, _) = reg.billed(&self.pick.id);
        let model = line([
            glyph(reg, &pr),
            plain(" "),
            spb(self.word(m, &self.pick), TEXT),
            sp(format!("  {}", prov_label(reg, &self.pick.id)), DIM),
        ]);
        self.hits.push(Hit {
            y: out.len(),
            row: 0,
            pick: None,
        });
        let mut l = line([sp(fit_str("harness", 10), DIM)]);
        join(&mut l, harness);
        out.push(sheet_row(l, self.row == 0, w));
        self.hits.push(Hit {
            y: out.len(),
            row: 1,
            pick: None,
        });
        let mut l = line([sp(fit_str("model", 10), DIM)]);
        join(&mut l, model);
        out.push(sheet_row(l, self.row == 1 && !self.open, w));

        // Only when it matters: what this one can't do.
        let cant: Vec<&str> = CANT
            .iter()
            .filter(|(f, _)| !reg.supports(&k, *f))
            .map(|(_, w)| *w)
            .collect();
        let (_, warn, _) = reg.compat(&start_id(reg, &o), reg.harness_of(&k).as_str());
        if !warn.is_empty() {
            out.push(line([plain("  "), sp(format!("⚠ {warn}"), YELLOW)]));
        } else if !cant.is_empty() {
            out.push(line([
                plain("  "),
                sp("⚠ ", YELLOW),
                sp(
                    format!("{} can't {}", harness_name(reg, &k), cant.join(" · ")),
                    DIM,
                ),
            ]));
        }
        if self.open {
            self.list_lines(m, &mut out, w, h);
        }
        let act = if m.input.is_empty() {
            "use it"
        } else {
            "start it"
        };
        out.push(Line::new());
        if self.open {
            out.extend(keys_controls(
                w,
                &[
                    ("type", "find"),
                    ("↑↓", "model"),
                    ("enter", "pick"),
                    ("esc", "back"),
                ],
            ));
        } else {
            out.extend(keys_controls(
                w,
                &[
                    ("↑↓", "row"),
                    ("←→", "change"),
                    ("enter", act),
                    ("esc", "cancel"),
                ],
            ));
        }
        if h > 0 && out.len() > h {
            out.truncate(h);
        }
        out.into_iter().map(|l| fit(l, w)).collect()
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if self.open {
            let list = self.shown(m);
            match k.s.as_str() {
                "esc" => {
                    self.open = false;
                    self.filter.clear();
                }
                "up" => self.sel = self.sel.saturating_sub(1),
                "down" => self.sel = (self.sel + 1).min(list.len().saturating_sub(1)),
                "enter" | "tab" => {
                    if let Some(p) = list.get(self.sel) {
                        self.pick = p.clone();
                    }
                    self.open = false;
                    self.filter.clear();
                }
                "backspace" => {
                    if self.filter.pop().is_some() {
                        self.sel = 0;
                    }
                }
                _ => {
                    let Some(c) = k.text() else {
                        return Out::Idle;
                    };
                    self.filter.push(c);
                    self.sel = 0;
                }
            }
            return Out::Draw;
        }
        match k.s.as_str() {
            "esc" | "ctrl+c" => return Out::Close,
            "up" | "down" | "tab" | "shift+tab" => self.row = 1 - self.row,
            "left" | "right" => {
                if self.row == 1 {
                    self.open_list(m);
                    return Out::Draw;
                }
                let leaders = leaders(m);
                let i = leaders.iter().position(|h| *h == self.harness).unwrap_or(0);
                let d = if k.s == "left" { -1 } else { 1 };
                if let Some(h) = leaders.get(round_move(i, d, leaders.len())) {
                    self.harness = h.clone();
                }
                self.keep_pick(m);
            }
            "space" => {
                if self.row == 1 {
                    self.open_list(m);
                }
            }
            "enter" => return self.use_it(m),
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn mouse(&mut self, m: &mut Model, ev: MouseEv, _x: i32, y: i32) -> Out {
        match ev {
            MouseEv::WheelUp => return self.key(m, &Press::of("up").expect("up")),
            MouseEv::WheelDown => return self.key(m, &Press::of("down").expect("down")),
            MouseEv::Press => {}
            _ => return Out::Idle,
        }
        let Some(h) = self.hits.iter().find(|h| h.y as i32 == y).copied() else {
            return Out::Pass;
        };
        if let Some(i) = h.pick {
            if let Some(p) = self.shown(m).get(i) {
                self.pick = p.clone();
            }
            self.open = false;
            self.filter.clear();
            return Out::Draw;
        }
        if self.row == h.row && h.row == 1 {
            self.open_list(m);
            return Out::Draw;
        }
        if self.row == h.row {
            return self.key(m, &Press::of("right").expect("right"));
        }
        self.row = h.row;
        Out::Draw
    }
}
