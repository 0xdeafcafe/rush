//! The Start sheet: what the next session starts as, for that one session
//! (Settings stay as they are), or opened on a session, what it switches
//! to. Three linked columns (provider, harness, model) and the account,
//! effort and permissions rows. Each column shows every choice, those that
//! can't go with the others faint with why; picking one moves the other
//! column to one that can. Its last line is the #new that starts the
//! same. After Go's internal/ui/startsheet.go (startSheet :167, column
//! :189, startRows :192, tabOrder :196, openSetupSheet :208, at :255,
//! loadModels :267, sheetProviders :303, sheetHarnesses :317, choices
//! :346, now :398, word :403, body :446, mouse :543, columns :573, key
//! :665, set :735), permissions.go:12 (permissionChoices) and keys.go:947
//! (#model, #effort opening it on a row).

use rush_core::adapter::{Choice, Kind};
use rush_core::text::{Color, Line, Span};
use rush_state::profiles::BILLING_KEY;

use super::bits::{fit_w, keys_controls};
use super::look::{glyph, model_word, prov_kind, prov_label, prov_of};
use super::newsheet::load_all_models;
use super::rows::{account_of, start_dir};
use super::setup::{
    Route, StartOver, account_names, fits, models, new_words, next_start, route_rows,
    setup_chip, start_as, start_id, start_on, start_routes, start_with_words,
};
use super::state::{accounts, cfg, ensure_store};
use crate::frame::overlayclick::{MouseEv, hint_key};
use crate::frame::picker::round_move;
use crate::frame::sheet::{Sheet, sheet_row, sheet_title, window};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// The sheet's parts, as rows name them.
pub const START_ROWS: [&str; 7] = [
    "Preset",
    "Provider",
    "Harness",
    "Account",
    "Model",
    "Effort",
    "Permissions",
];

/// The order tab goes through them: the columns left to right. Preset is
/// no longer one: profiles are gone (Settings › Config).
const TAB_ORDER: [usize; 6] = [1, 2, 4, 3, 5, 6];

/// Whether row is one of the three columns.
fn column(row: usize) -> bool {
    matches!(row, 1 | 2 | 4)
}

/// A part of the body a click takes: a value in a column, or a part to
/// put in focus (startHit).
#[derive(Clone, Debug)]
struct Hit {
    y: usize,
    x0: usize,
    x1: usize,
    row: usize,
    v: String,
    /// v is the value to set; else the click focuses row.
    pick: bool,
}

/// Picks a StartOver. row is the part in focus, as START_ROWS names them.
pub struct StartSheet {
    /// Permission controls never submit a draft kept in the box.
    pub apply_only: bool,
    pub routes: Vec<Route>,
    pub o: StartOver,
    pub row: usize,
    /// The session it switches; "" for the next one.
    pub conn: String,
    /// The provider picked, kept while harnesses that can't run it go by.
    pub chose: String,
    hits: Vec<Hit>,
    lines: Vec<Line>,
}

/// Opens the sheet on what the next session would start as
/// (openStartSheet).
pub fn open_start_sheet(m: &mut Model) -> Out {
    ensure_store(m);
    let o = next_start(m, &start_dir(m));
    open_setup_sheet(m, "", o)
}

/// Opens the sheet on what session key runs as, o, to switch it
/// (openSwitchSheet).
pub fn open_switch_sheet(m: &mut Model, key: &str, o: StartOver) -> Out {
    ensure_store(m);
    open_setup_sheet(m, key, o)
}

fn open_setup_sheet(m: &mut Model, conn: &str, o: StartOver) -> Out {
    let mut rs = start_routes(m);
    if !conn.is_empty() {
        let id = start_id(&m.reg, &o);
        let mut found = false;
        for r in rs.iter_mut().filter(|r| r.id == id) {
            found = true;
            if !r.kinds.contains(&o.kind) {
                r.kinds.push(o.kind.clone());
            }
        }
        if !found {
            rs.push(Route {
                id,
                kinds: vec![o.kind.clone()],
            });
        }
    }
    if rs.is_empty() {
        m.flash_err("rush can't run any harness here yet");
        return Out::Draw;
    }
    let mut s = StartSheet {
        apply_only: false,
        routes: rs,
        o,
        row: 1,
        conn: conn.to_string(),
        chose: String::new(),
        hits: Vec::new(),
        lines: Vec::new(),
    };
    let id = start_id(&m.reg, &s.o);
    if !s.at(&id).is_some_and(|r| r.kinds.contains(&s.o.kind)) {
        let (id, k) = (s.routes[0].id.clone(), s.routes[0].kinds[0].clone());
        s.o = start_on(m, &id, &k);
    }
    let k = s.o.kind.clone();
    m.open_sheet(Box::new(s));
    load_models(m, &k);
    load_all_models(m);
    Out::Draw
}

/// #model and #effort: the sheet on its Model (4) or Effort (5) row, the
/// value named set when there is one (keys.go:947).
pub fn open_on(m: &mut Model, row: usize, arg: &str) -> Out {
    let out = open_start_sheet(m);
    let arg = arg.trim().to_string();
    if let Some(s) = m.sheet_as::<StartSheet>() {
        s.row = row;
        if !arg.is_empty() {
            if row == 4 {
                s.o.model = arg;
            } else {
                s.o.effort = arg;
            }
            s.o.profile.clear();
        }
    }
    out
}

/// Reads the models agent k offers from its home (Codex's cache), off the
/// UI thread; nothing when its adapter doesn't list them (loadModels).
pub fn load_models(m: &mut Model, k: &str) {
    let Some(ad) = m.reg.get(k) else { return };
    if ad.model_lister().is_none() {
        return;
    }
    let k = k.to_string();
    m.later(
        move || {
            let p = ad.profiles().into_iter().next();
            match (ad.model_lister(), p) {
                (Some(l), Some(p)) => l.list_models(&p),
                _ => Vec::new(),
            }
        },
        move |m, v| {
            accounts(m).listed.insert(k, v);
            Out::Draw
        },
    );
}

/// The permission modes agent kind offers (permissionChoices): a hosted
/// session's own list is the host's, not in this rush yet.
pub fn permission_choices(m: &Model, kind: &str) -> Vec<Choice> {
    m.reg.choices_of(kind).map(|c| c.modes).unwrap_or_default()
}

impl StartSheet {
    /// The route of provider id (at).
    fn at(&self, id: &str) -> Option<&Route> {
        self.routes.iter().find(|r| r.id == id)
    }

    /// The provider column: every provider with a route that runs here,
    /// and the one picked (sheetProviders).
    fn providers(&self, m: &Model) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in route_rows(m) {
            if r.why.is_empty() && !out.contains(&r.id) {
                out.push(r.id);
            }
        }
        let id = start_id(&m.reg, &self.o);
        if !out.contains(&id) {
            out.push(id);
        }
        out
    }

    /// The harness column: every harness with a route that runs here, and
    /// the one picked (sheetHarnesses).
    fn harnesses(&self, m: &Model) -> Vec<Kind> {
        let rows = route_rows(m);
        let mut out: Vec<Kind> = m
            .reg
            .harness_kinds()
            .into_iter()
            .filter(|h| rows.iter().any(|r| r.harness == *h && r.why.is_empty()))
            .collect();
        let h = m.reg.harness_of(&self.o.kind);
        if !out.contains(&h) {
            out.push(h);
        }
        out
    }

    /// Row's values, as the StartOver field it sets holds them (choices).
    pub fn choices(&self, m: &Model, row: usize) -> Vec<String> {
        let k = self.o.kind.as_str();
        match row {
            0 => return vec![String::new()],
            1 => return self.providers(m),
            2 => return self.harnesses(m).into_iter().map(|h| h.0).collect(),
            3 => {
                if self.o.billing == BILLING_KEY {
                    return vec![String::new()];
                }
                let mut out = vec![String::new()];
                out.extend(account_names(m, k));
                return out;
            }
            6 => {
                let mut out = Vec::new();
                if self.conn.is_empty() {
                    out.push(String::new());
                }
                out.extend(permission_choices(m, k).into_iter().map(|c| c.id));
                if !out.contains(&self.o.mode) {
                    out.push(self.o.mode.clone());
                }
                return out;
            }
            _ => {}
        }
        let (list, now) = if row == 5 {
            (
                m.reg.choices_of(k).map(|c| c.efforts).unwrap_or_default(),
                &self.o.effort,
            )
        } else {
            (models(m, k), &self.o.model)
        };
        let c = cfg(m);
        let (id, h) = (start_id(&m.reg, &self.o), m.reg.harness_of(k));
        let mut out = vec![String::new()];
        for ch in list {
            // The harness may call only what Config lets it.
            if row == 5 || c.calls(&id, h.as_str(), &ch.id) {
                out.push(ch.id);
            }
        }
        if !out.contains(now) {
            out.push(now.clone()); // one Settings names that the list doesn't
        }
        out
    }

    /// Row's value in o (now).
    pub fn now(&self, m: &Model, row: usize) -> String {
        match row {
            0 => self.o.profile.clone(),
            1 => start_id(&m.reg, &self.o),
            2 => m.reg.harness_of(&self.o.kind).0,
            3 => self.o.account.clone(),
            4 => self.o.model.clone(),
            5 => self.o.effort.clone(),
            _ => self.o.mode.clone(),
        }
    }

    /// Row's value v in words (word).
    pub fn word(&self, m: &Model, row: usize, v: &str) -> String {
        let k = self.o.kind.as_str();
        match row {
            0 => or(v, "none: as below"),
            1 => prov_label(&m.reg, v),
            2 => m.reg.harness_label(v),
            3 => {
                let in_use = account_of(m, k);
                if self.o.billing == BILLING_KEY {
                    "its API key".into()
                } else if v.is_empty() && !in_use.is_empty() {
                    format!("{in_use}, in use")
                } else if v.is_empty() {
                    "its own sign-in".into()
                } else if v == in_use {
                    format!("{v}, in use")
                } else {
                    format!(
                        "{v}, switched to for every {} session",
                        m.reg.harness_label(k)
                    )
                }
            }
            6 => {
                if !v.is_empty() {
                    v.to_string()
                } else if !self.conn.is_empty() {
                    "not reported by harness".into()
                } else {
                    "harness default".into()
                }
            }
            4 => {
                if v.is_empty() {
                    "its default model".into()
                } else {
                    model_word(&m.reg, k, v)
                }
            }
            _ => or(v, "its default effort"),
        }
    }

    /// The part in focus's field: its name, then its value between ‹ ›
    /// when there's another to go to.
    fn field(&self, m: &Model, row: usize) -> Line {
        let v = self.word(m, row, &self.now(m, row));
        if self.choices(m, row).len() < 2 {
            return line([sp(format!("{} ", START_ROWS[row]), DIM), sp(v, TEXT)]);
        }
        line([
            sp(format!("{} ", START_ROWS[row]), DIM),
            sp("‹ ", FAINT),
            spb(v, TEXT),
            sp(" ›", FAINT),
        ])
    }

    /// The provider, harness and model columns, n lines tall, from line y
    /// of the body; where each value landed is kept (columns).
    fn columns(&mut self, m: &Model, w: usize, n: usize, y: usize) -> Vec<Line> {
        let inner = w.saturating_sub(4);
        let (pw, hw) = (inner * 35 / 100, inner * 37 / 100);
        let mw = inner - pw - hw;
        let reg = &m.reg;
        let id = start_id(reg, &self.o);
        let h = reg.harness_of(&self.o.kind);
        let row = self.row;
        let head = |r: usize, name: &str, cw: usize| -> Span {
            let s = fit_str(&name.to_uppercase(), cw);
            if r == row { spb(s, ORANGE) } else { sp(s, DIM) }
        };
        let cell = |r: usize, on: bool, text: Line, cw: usize| -> Line {
            let mut l = if on {
                line([sp("▸ ", ORANGE)])
            } else {
                line([plain("  ")])
            };
            if on && r == row {
                join(&mut l, recolor(text, BRIGHT, true));
            } else {
                join(&mut l, text);
            }
            fit_w(l, cw)
        };
        let provs_v = self.providers(m);
        let mut provs = Vec::new();
        for p in &provs_v {
            let (k, _, why) = fits(m, p, h.as_str());
            let pk = prov_kind(reg, cfg(m), p);
            // ★ and the glyph of the harness it starts in by default.
            let mut text = line([
                glyph(reg, &prov_of(reg, p)),
                plain(" "),
                sp(fit_str(&prov_label(reg, p), pw.saturating_sub(10)), TEXT),
                plain(" "),
                sp("★", DIM),
                glyph(reg, reg.harness_of(pk.as_str()).as_str()),
            ]);
            if !why.is_empty() || k.is_empty() {
                text = recolor(text, FAINT, false);
            }
            provs.push(cell(1, *p == id, text, pw));
        }
        let pk = reg.harness_of(prov_kind(reg, cfg(m), &id).as_str());
        let harns_v = self.harnesses(m);
        let mut harns = Vec::new();
        for hk in &harns_v {
            let (k, warn, why) = fits(m, &id, hk.as_str());
            let name = fit_str(&reg.harness_label(hk.as_str()), 13);
            let mut text = line([glyph(reg, hk.as_str()), plain(" "), sp(name.clone(), TEXT)]);
            if !why.is_empty() {
                text = line([
                    sp(format!("{} {name} – {why}", glyph(reg, hk.as_str()).text), FAINT),
                ]);
            } else if k.is_empty() {
            } else if !warn.is_empty() {
                text.spans.push(sp(" ⚠ on your plan", YELLOW));
            } else if *hk == pk {
                text.spans.push(plain(" "));
                text.spans.push(sp("★", ORANGE));
            }
            harns.push(cell(2, *hk == h, text, hw));
        }
        let def = start_on(m, &id, &self.o.kind).model;
        let mods_v = self.choices(m, 4);
        let mut mods = Vec::new();
        for v in &mods_v {
            let mut text = line([sp(
                fit_str(&self.word(m, 4, v), mw.saturating_sub(8)),
                TEXT,
            )]);
            if !v.is_empty() && *v == def {
                text.spans.push(plain(" "));
                text.spans.push(sp("★", ORANGE));
            }
            mods.push(cell(4, *v == self.o.model, text, mw));
        }
        let x0 = |r: usize| match r {
            1 => 2,
            2 => 2 + pw,
            _ => 2 + pw + hw,
        };
        let cw = |r: usize| match r {
            1 => pw,
            2 => hw,
            _ => mw,
        };
        let mut at = |r: usize, list: Vec<Line>, vals: &[String], now: Option<usize>| {
            let (from, to) = window(list.len(), now.unwrap_or(0), n);
            for i in from..to.min(vals.len()) {
                self.hits.push(Hit {
                    y: y + 1 + i - from,
                    x0: x0(r),
                    x1: x0(r) + cw(r),
                    row: r,
                    v: vals[i].clone(),
                    pick: true,
                });
            }
            list[from..to].to_vec()
        };
        let pi = provs_v.iter().position(|p| *p == id);
        let hi = harns_v.iter().position(|x| *x == h);
        let mi = mods_v.iter().position(|v| *v == self.o.model);
        let harns_vals: Vec<String> = harns_v.iter().map(|h| h.0.clone()).collect();
        let provs = at(1, provs, &provs_v, pi);
        let harns = at(2, harns, &harns_vals, hi);
        let mods = at(4, mods, &mods_v, mi);
        for r in [1, 2, 4] {
            self.hits.push(Hit {
                y,
                x0: x0(r),
                x1: x0(r) + cw(r),
                row: r,
                v: String::new(),
                pick: false,
            }); // its heading
        }
        let mut out = vec![line([
            plain("  "),
            head(1, "Provider", pw),
            head(2, "Harness", hw),
            head(4, "Model", mw),
        ])];
        let tall = provs.len().max(harns.len()).max(mods.len());
        for i in 0..tall {
            let mut l = line([plain("  ")]);
            for (list, cw) in [(&provs, pw), (&harns, hw), (&mods, mw)] {
                match list.get(i) {
                    Some(c) => join(&mut l, c.clone()),
                    None => l.spans.push(plain(blanks(cw))),
                }
            }
            out.push(l);
        }
        out
    }

    /// Puts v in the part in focus. A provider keeps the harness when it
    /// runs there, else goes to its default one; a harness keeps the
    /// provider when it runs it, else takes the first that it runs.
    /// Anything but a profile makes it a setup of its own (set).
    pub fn set(&mut self, m: &mut Model, v: &str) -> Out {
        match self.row {
            1 => {
                let h = m.reg.harness_of(&self.o.kind);
                let (mut k, _, why) = fits(m, v, h.as_str());
                if !why.is_empty() || k.is_empty() {
                    k = prov_kind(&m.reg, cfg(m), v);
                }
                self.o = start_on(m, v, k.as_str());
                self.chose = v.to_string();
                load_models(m, k.as_str());
                return Out::Draw;
            }
            2 => {
                let h = Kind::from(v);
                let mut id = if self.chose.is_empty() {
                    start_id(&m.reg, &self.o)
                } else {
                    self.chose.clone()
                };
                self.chose = id.clone();
                let (mut k, _, why) = fits(m, &id, h.as_str());
                if !why.is_empty() || k.is_empty() {
                    if let Some(r) = route_rows(m)
                        .into_iter()
                        .find(|r| r.harness == h && r.why.is_empty())
                    {
                        id = r.id;
                        k = r.kind;
                    }
                }
                self.o = start_on(m, &id, k.as_str());
                load_models(m, k.as_str());
                return Out::Draw;
            }
            3 => self.o.account = v.to_string(),
            4 => self.o.model = v.to_string(),
            5 => self.o.effort = v.to_string(),
            6 => {
                self.o.mode = v.to_string();
                self.o.mode_set = true;
            }
            _ => {}
        }
        self.o.profile.clear();
        Out::Draw
    }

    /// Moves the part in focus's value by d.
    fn step(&mut self, m: &mut Model, d: isize) -> Out {
        let vals = self.choices(m, self.row);
        let now = self.now(m, self.row);
        let i = vals.iter().position(|v| *v == now).unwrap_or(0);
        let v = vals[round_move(i, d, vals.len())].clone();
        self.set(m, &v)
    }
}

/// s, else or.
fn or(s: &str, or: &str) -> String {
    if s.is_empty() { or.into() } else { s.into() }
}

/// l in one colour, bold or not (paint over ansi.Strip).
fn recolor(l: Line, c: Color, b: bool) -> Line {
    let text = l.plain();
    if b { line([spb(text, c)]) } else { line([sp(text, c)]) }
}

impl Sheet for StartSheet {
    fn width(&self, m: &Model) -> usize {
        60.max(112.min(m.w.saturating_sub(6)))
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let (title, about) = if self.conn.is_empty() {
            (
                "Start as",
                "the next session only; Settings stay as they are",
            )
        } else {
            (
                "Switch to",
                "in place in the same harness, else the conversation is handed over",
            )
        };
        self.hits.clear();
        let mut bottom: Vec<Line> = Vec::new();
        let mut bottom_hits: Vec<Hit> = Vec::new(); // y counts from bottom's first line
        let fields: Vec<Line> = [3, 5, 6].iter().map(|r| self.field(m, *r)).collect();
        let all_w = fields.iter().map(Line::width).sum::<usize>() + 8;
        if all_w + 2 <= w {
            let mut l = line([plain(" ")]);
            for (i, (row, f)) in [3, 5, 6].into_iter().zip(fields).enumerate() {
                if i > 0 {
                    l.spans.push(plain("   "));
                }
                let x = l.width();
                l.spans.push(if self.row == row {
                    sp("▍", ORANGE)
                } else {
                    plain(" ")
                });
                join(&mut l, f);
                bottom_hits.push(Hit {
                    y: 0,
                    x0: x,
                    x1: l.width(),
                    row,
                    v: String::new(),
                    pick: false,
                });
            }
            bottom.push(l);
        } else {
            for (row, f) in [3, 5, 6].into_iter().zip(fields) {
                bottom_hits.push(Hit {
                    y: bottom.len(),
                    x0: 0,
                    x1: w,
                    row,
                    v: String::new(),
                    pick: false,
                });
                bottom.push(sheet_row(f, self.row == row, w));
            }
        }
        if self.row == 6 {
            let choices = permission_choices(m, &self.o.kind);
            let mut note = "Permissions are per session; pending approvals and queued messages stay unchanged.".to_string();
            if choices.is_empty() {
                note = "This harness has not advertised selectable permission modes.".into();
            }
            for v in &choices {
                if v.id == self.o.mode && !v.note.is_empty() {
                    note = v.note.clone();
                }
            }
            for l in wrap(&note, w) {
                bottom.push(line([sp(l, TEXT)]));
            }
        }
        let reg = &m.reg;
        let (_, warn, _) = reg.compat(
            &start_id(reg, &self.o),
            reg.harness_of(&self.o.kind).as_str(),
        );
        let mut chip = line([plain("  ")]);
        join(&mut chip, setup_chip(m, &self.o));
        if !warn.is_empty() {
            chip.spans.push(plain("  "));
            chip.spans.push(sp(format!("⚠ {warn}"), YELLOW));
        }
        let said = vec![
            Line::new(),
            chip,
            line([
                plain("  "),
                sp("same as  ", DIM),
                sp(new_words(reg, &self.o), TEXT),
            ]),
        ];
        let act = if self.row == 6 || self.apply_only {
            "Apply"
        } else if !self.conn.is_empty() {
            "switch"
        } else if !m.input.is_empty() {
            "start it"
        } else {
            "use it"
        };
        let footer = keys_controls(
            w,
            &[
                ("←→", "column or value"),
                ("↑↓", "choose"),
                ("tab", "next part"),
                ("enter", act),
                ("esc", "cancel"),
            ],
        );
        let mut out = vec![sheet_title(title, about, w)];
        let mut room: isize = 1 << 20;
        if h > 0 {
            room = h as isize - out.len() as isize - bottom.len() as isize - footer.len() as isize;
        }
        if room >= said.len() as isize + 6 {
            room -= said.len() as isize;
            bottom.extend(said);
        }
        if room >= 4 {
            // columns three choices tall
            out.push(Line::new());
            let y = out.len();
            let cols = self.columns(m, w, 8.min(room as usize - 2), y);
            out.extend(cols);
            out.push(Line::new());
        }
        for mut b in bottom_hits {
            b.y += out.len();
            self.hits.push(b);
        }
        out.extend(bottom);
        let fl = footer.len();
        out.extend(footer.clone());
        if h > 0 && out.len() > h {
            out.truncate(h.saturating_sub(fl));
            out.extend(footer);
            // What was cut takes no clicks: its lines are the footer's now.
            let cut = out.len() - fl;
            self.hits.retain(|x| x.y < cut);
        }
        let out: Vec<Line> = out.into_iter().map(|l| fit(l, w)).collect();
        self.lines = out.clone();
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let col = column(self.row);
        match k.s.as_str() {
            "esc" | "ctrl+c" => return Out::Close,
            "tab" | "shift+tab" => {
                let d = if k.s == "shift+tab" { -1 } else { 1 };
                let i = TAB_ORDER.iter().position(|r| *r == self.row).unwrap_or(0);
                self.row = TAB_ORDER[round_move(i, d, TAB_ORDER.len())];
            }
            "up" | "down" => {
                let d = if k.s == "up" { -1 } else { 1 };
                if col {
                    return self.step(m, d);
                } else if d < 0 {
                    self.row = 4; // from the rows below, back into the columns
                } else if self.row != 0 {
                    self.row = match self.row {
                        3 => 5,
                        5 => 6,
                        _ => 3,
                    };
                }
            }
            "left" | "right" => {
                let d: isize = if k.s == "left" { -1 } else { 1 };
                if col {
                    let cols = [1, 2, 4];
                    let i = cols.iter().position(|r| *r == self.row).unwrap_or(0) as isize + d;
                    if i >= 0 && (i as usize) < cols.len() {
                        self.row = cols[i as usize];
                    }
                    return Out::Draw;
                }
                return self.step(m, d);
            }
            "enter" => {
                let o = self.o.clone();
                if !self.conn.is_empty() {
                    let key = self.conn.clone();
                    let out = match accounts(m).switcher {
                        Some(f) => f(m, &key, o, ""),
                        None => {
                            m.flash_err("switching a session isn't in this rush yet");
                            Out::Draw
                        }
                    };
                    return if out == Out::Quit { out } else { Out::Close };
                }
                if !m.input.is_empty() && self.row != 6 && !self.apply_only {
                    let msg = m.input.string();
                    m.input.clear();
                    let out = start_as(m, o, &msg);
                    return if out == Out::Quit { out } else { Out::Close };
                }
                accounts(m).start_over = Some(o);
                let w = start_with_words(m, &start_dir(m));
                m.flash(&format!("the next session starts as {w}"));
                return Out::Close;
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }

    /// A click on a value in a column picks it, on another part puts it in
    /// focus, and again on a part in focus moves to its next value. The
    /// wheel and the keys at the foot do as they say (mouse).
    fn mouse(&mut self, m: &mut Model, ev: MouseEv, x: i32, y: i32) -> Out {
        match ev {
            MouseEv::WheelUp => return self.key(m, &Press::of("up").expect("up")),
            MouseEv::WheelDown => return self.key(m, &Press::of("down").expect("down")),
            MouseEv::Press => {}
            _ => return Out::Idle,
        }
        if x < 0 || y < 0 {
            return Out::Pass;
        }
        let (x, y) = (x as usize, y as usize);
        if let Some(h) = self
            .hits
            .iter()
            .find(|h| h.y == y && x >= h.x0 && x < h.x1)
            .cloned()
        {
            if h.pick {
                self.row = h.row;
                return self.set(m, &h.v);
            }
            if self.row == h.row && !column(h.row) {
                return self.key(m, &Press::of("right").expect("right"));
            }
            self.row = h.row;
            return Out::Draw;
        }
        if let Some(k) = self.lines.get(y).and_then(|l| hint_key(l, x)) {
            return m.press(&k);
        }
        Out::Idle
    }
}
