//! rush's /plugins: what's installed (on/off, update, remove, what each
//! brings and costs), what the marketplaces offer (search, install), and
//! the marketplaces themselves. Changes go through the agent's adapter,
//! so they're exactly what its own screen would do. After Go's
//! internal/ui/pluginsheet.go (pluginSheet :20, openPlugins :53, load
//! :64, askCost :93, run :113, shown :132, key :156, body :260,
//! installedRows :296, discoverRows :322, marketRows :338, detail :361,
//! footer :417).

use std::collections::{HashMap, HashSet};

use rush_core::adapter::{AdapterRef, Marketplace, Plugin, PluginChange, PluginOp, Profile};
use rush_core::agent::Agent;
use rush_core::text::Line;

use super::{installs, settled, sheet_do, with_sheet};
use crate::frame::picker::round_move;
use crate::frame::sheet::{sheet_row, sheet_tabs, sheet_title, text_field, window};
use crate::frame::{Confirm, Out, Press, Sheet, editor};
use crate::model::Model;
use crate::style::*;

pub const INSTALLED: usize = 0;
pub const DISCOVER: usize = 1;
pub const MARKETS: usize = 2;

/// The /plugins sheet (pluginSheet).
pub struct PluginSheet {
    /// The Session it was opened from.
    pub conn: String,
    pub ad: AdapterRef,
    /// The account's name, and its profile once read.
    pub acct: String,
    pub profile: Option<Profile>,
    pub cwd: String,

    pub tab: usize,
    pub cur: [usize; 3],
    pub query: Vec<char>,
    pub query_pos: usize,
    /// Typing a marketplace to add.
    pub adding: bool,
    pub source: Vec<char>,
    pub source_pos: usize,

    pub loaded: bool,
    pub installed: Vec<Plugin>,
    pub available: Vec<Plugin>,
    pub markets: Vec<Marketplace>,
    /// Always-on tokens, by plugin id.
    pub costs: HashMap<String, String>,
    pub cost_asked: HashSet<String>,

    /// What's running now.
    pub busy: String,
    pub err: String,
    pub done: String,
    pub changed: bool,
}

type Lists = Result<(Profile, Vec<Plugin>, Vec<Plugin>, Vec<Marketplace>), String>;

/// Opens /plugins for agent a, whose session works in cwd (openPlugins).
pub fn open(m: &mut Model, a: &Agent, cwd: &str) {
    let ad = m
        .reg
        .get(&a.kind.to_string())
        .filter(|ad| ad.plugger().is_some());
    let Some(ad) = ad else {
        let name = m.reg.harness_label(&a.kind.to_string());
        m.flash_err(&format!("{name} has no plugins rush manages"));
        return;
    };
    let cwd = if cwd.is_empty() { &a.job.cwd } else { cwd };
    let p = PluginSheet::new(a.key.clone(), ad, a.job.account.clone(), cwd.to_string());
    m.open_sheet(Box::new(p));
    with_sheet(m, |p: &mut PluginSheet, m| p.load(m));
}

impl PluginSheet {
    pub fn new(conn: String, ad: AdapterRef, acct: String, cwd: String) -> PluginSheet {
        PluginSheet {
            conn,
            ad,
            acct,
            profile: None,
            cwd,
            tab: INSTALLED,
            cur: [0; 3],
            query: Vec::new(),
            query_pos: 0,
            adding: false,
            source: Vec::new(),
            source_pos: 0,
            loaded: false,
            installed: Vec::new(),
            available: Vec::new(),
            markets: Vec::new(),
            costs: HashMap::new(),
            cost_asked: HashSet::new(),
            busy: String::new(),
            err: String::new(),
            done: String::new(),
            changed: false,
        }
    }

    /// Reads everything, off the UI (load).
    fn load(&mut self, m: &mut Model) -> Out {
        let (ad, acct, cwd, prof) = (
            self.ad.clone(),
            self.acct.clone(),
            self.cwd.clone(),
            self.profile.clone(),
        );
        sheet_do(
            m,
            move || -> Lists {
                let prof = prof.unwrap_or_else(|| profile_of(&ad, &acct));
                let pl = ad.plugger().ok_or("no plugins")?;
                let (inst, avail) = pl.plugins(&prof, &cwd).map_err(|e| e.to_string())?;
                let ms = pl.marketplaces(&prof, &cwd).map_err(|e| e.to_string())?;
                Ok((prof, inst, avail, ms))
            },
            PluginSheet::loaded,
        );
        settled(self, m, PluginSheet::loaded).unwrap_or(Out::Draw)
    }

    fn loaded(&mut self, m: &mut Model, l: Lists) -> Out {
        self.loaded = true;
        match l {
            Err(e) => self.err = e,
            Ok((prof, inst, avail, ms)) => {
                self.profile = Some(prof);
                self.installed = inst;
                self.available = avail;
                self.markets = ms;
                return self.ask_cost(m);
            }
        }
        Out::Draw
    }

    /// Fetches what the selected installed plugin adds to every session,
    /// once (askCost).
    fn ask_cost(&mut self, m: &mut Model) -> Out {
        if self.tab != INSTALLED || self.installed.is_empty() {
            return Out::Draw;
        }
        let pl = &self.installed[self.cur[INSTALLED].min(self.installed.len() - 1)];
        if !self.cost_asked.insert(pl.id.clone()) {
            return Out::Draw;
        }
        let Some(prof) = self.profile.clone() else {
            return Out::Draw;
        };
        let (ad, id) = (self.ad.clone(), pl.id.clone());
        let dir = if pl.project_path.is_empty() {
            self.cwd.clone()
        } else {
            pl.project_path.clone()
        };
        sheet_do(
            m,
            move || {
                let c = ad
                    .plugger()
                    .and_then(|p| p.plugin_cost(&prof, &dir, &id).ok())
                    .unwrap_or_default();
                (id, c)
            },
            PluginSheet::costed,
        );
        settled(self, m, PluginSheet::costed).unwrap_or(Out::Draw)
    }

    fn costed(&mut self, _m: &mut Model, (id, cost): (String, String)) -> Out {
        if !cost.is_empty() {
            self.costs.insert(id, cost);
        }
        Out::Draw
    }

    /// Makes a change through the agent, then reads everything again
    /// (run).
    fn run(&mut self, m: &mut Model, what: String, dir: String, c: PluginChange) -> Out {
        if !self.busy.is_empty() {
            return Out::Idle;
        }
        let Some(prof) = self.profile.clone() else {
            return Out::Idle;
        };
        self.busy = what;
        self.err.clear();
        self.done.clear();
        let ad = self.ad.clone();
        sheet_do(
            m,
            move || {
                ad.plugger()
                    .ok_or_else(|| "no plugins".to_string())
                    .and_then(|p| p.change_plugins(&prof, &dir, &c).map_err(|e| e.to_string()))
            },
            PluginSheet::ran,
        );
        settled(self, m, PluginSheet::ran).unwrap_or(Out::Draw)
    }

    fn ran(&mut self, m: &mut Model, r: Result<(), String>) -> Out {
        let what = std::mem::take(&mut self.busy);
        if let Err(e) = r {
            self.err = e;
            return Out::Draw;
        }
        self.done = what.trim_end_matches('…').to_string();
        self.changed = true;
        self.load(m)
    }

    /// The Discover list for the search (shown).
    pub fn shown(&self) -> Vec<&Plugin> {
        let q = self.query.iter().collect::<String>().trim().to_lowercase();
        self.available
            .iter()
            .filter(|p| {
                q.is_empty()
                    || format!("{} {} {}", p.name, p.marketplace, p.description)
                        .to_lowercase()
                        .contains(&q)
            })
            .collect()
    }

    fn size(&self) -> usize {
        match self.tab {
            INSTALLED => self.installed.len(),
            DISCOVER => self.shown().len(),
            _ => self.markets.len(),
        }
    }
}

/// The agent's profile for an account, by its name; the first, else one
/// named so.
fn profile_of(ad: &AdapterRef, acct: &str) -> Profile {
    let ps = ad.profiles();
    ps.iter()
        .find(|p| p.name == acct)
        .or(ps.first())
        .cloned()
        .unwrap_or_else(|| Profile {
            kind: ad.kind(),
            name: acct.to_string(),
            dir: String::new(),
        })
}

fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

impl Sheet for PluginSheet {
    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let s = k.s.as_str();
        if self.adding {
            match s {
                "esc" => self.adding = false,
                "enter" => {
                    let src = self.source.iter().collect::<String>().trim().to_string();
                    self.adding = false;
                    self.source.clear();
                    self.source_pos = 0;
                    if !src.is_empty() {
                        let c = PluginChange {
                            op: PluginOp::MarketAdd,
                            id: src.clone(),
                            scope: String::new(),
                        };
                        return self.run(m, format!("adding {src}…"), self.cwd.clone(), c);
                    }
                }
                _ => {
                    if let Some((b, p)) = editor::edit(&self.source, self.source_pos, k) {
                        self.source = b;
                        self.source_pos = p;
                    }
                }
            }
            return Out::Draw;
        }
        let n = self.size();
        match s {
            "esc" | "ctrl+c" => return Out::Close,
            "[" | "]" => {
                self.tab = (self.tab + if s == "[" { 2 } else { 1 }) % 3;
                return self.ask_cost(m);
            }
            "up" => {
                self.cur[self.tab] = round_move(self.cur[self.tab], -1, n);
                return self.ask_cost(m);
            }
            "down" => {
                self.cur[self.tab] = round_move(self.cur[self.tab], 1, n);
                return self.ask_cost(m);
            }
            "pgup" => {
                self.cur[self.tab] = self.cur[self.tab].saturating_sub(10);
                return self.ask_cost(m);
            }
            "pgdown" => {
                self.cur[self.tab] = (self.cur[self.tab] + 10).min(n.saturating_sub(1));
                return self.ask_cost(m);
            }
            _ => {}
        }
        let cur = self.cur[self.tab];
        match self.tab {
            INSTALLED => {
                if self.installed.is_empty() {
                    return Out::Idle;
                }
                let pl = self.installed[cur.min(self.installed.len() - 1)].clone();
                let dir = or(&pl.project_path, &self.cwd).to_string();
                let scope = or(&pl.scope, "user").to_string();
                let change = |op| PluginChange {
                    op,
                    id: pl.id.clone(),
                    scope: scope.clone(),
                };
                match s {
                    "space" | "enter" => {
                        if pl.enabled {
                            return self.run(
                                m,
                                format!("turning off {}…", pl.name),
                                dir,
                                change(PluginOp::Disable),
                            );
                        }
                        return self.run(
                            m,
                            format!("turning on {}…", pl.name),
                            dir,
                            change(PluginOp::Enable),
                        );
                    }
                    "u" => {
                        return self.run(
                            m,
                            format!("updating {}…", pl.name),
                            dir,
                            change(PluginOp::Update),
                        );
                    }
                    "x" | "delete" | "backspace" => {
                        let c = change(PluginOp::Remove);
                        let name = pl.name.clone();
                        m.ask(
                            Confirm::new(format!("Remove the {name} plugin?"), move |m| {
                                rerun(m, format!("removing {name}…"), dir, c)
                            })
                            .detail(pl.id.clone()),
                        );
                        return Out::Draw;
                    }
                    _ => {}
                }
            }
            DISCOVER => {
                if s == "enter" {
                    let Some(pl) = self.shown().get(cur).map(|p| (*p).clone()) else {
                        return Out::Idle;
                    };
                    let c = PluginChange {
                        op: PluginOp::Install,
                        id: pl.id.clone(),
                        scope: "user".into(),
                    };
                    return self.run(m, format!("installing {}…", pl.name), self.cwd.clone(), c);
                }
                if let Some((q, pos)) = editor::edit(&self.query, self.query_pos, k) {
                    self.query = q;
                    self.query_pos = pos;
                    self.cur[DISCOVER] = 0;
                }
            }
            _ => {
                match s {
                    "a" | "+" => {
                        self.adding = true;
                        return Out::Draw;
                    }
                    "U" => {
                        let c = PluginChange {
                            op: PluginOp::MarketUpdate,
                            ..PluginChange::default()
                        };
                        return self.run(
                            m,
                            "updating every marketplace…".into(),
                            self.cwd.clone(),
                            c,
                        );
                    }
                    _ => {}
                }
                if self.markets.is_empty() {
                    return Out::Idle;
                }
                let mk = self.markets[cur.min(self.markets.len() - 1)].clone();
                match s {
                    "u" | "enter" => {
                        let c = PluginChange {
                            op: PluginOp::MarketUpdate,
                            id: mk.name.clone(),
                            scope: String::new(),
                        };
                        return self.run(m, format!("updating {}…", mk.name), self.cwd.clone(), c);
                    }
                    "x" | "delete" | "backspace" => {
                        let c = PluginChange {
                            op: PluginOp::MarketRemove,
                            id: mk.name.clone(),
                            scope: String::new(),
                        };
                        let (name, cwd) = (mk.name.clone(), self.cwd.clone());
                        m.ask(
                            Confirm::new(format!("Remove the {name} marketplace?"), move |m| {
                                rerun(m, format!("removing {name}…"), cwd, c)
                            })
                            .detail(mk.place().to_string()),
                        );
                        return Out::Draw;
                    }
                    _ => {}
                }
            }
        }
        Out::Draw
    }

    fn body(&mut self, _m: &Model, w: usize, h: usize) -> Vec<Line> {
        let enabled = self.installed.iter().filter(|p| p.enabled).count();
        let tabs = [
            format!("Installed {enabled}/{} on", self.installed.len()),
            format!("Discover {}", self.available.len()),
            format!("Marketplaces {}", self.markets.len()),
        ];
        let names: Vec<&str> = tabs.iter().map(String::as_str).collect();
        let mut out = vec![
            sheet_title(
                "Plugins",
                &format!("for {} · {}", self.acct, tildify(&self.cwd)),
                w,
            ),
            Line::new(),
            sheet_tabs(&names, self.tab),
            Line::new(),
        ];
        let detail = self.detail(w);
        let list_h = 3.max(h as isize - out.len() as isize - detail.len() as isize - 5) as usize;
        if !self.loaded {
            out.push(line([sp("  reading plugins…", DIM)]));
        } else if self.tab == INSTALLED {
            out.extend(self.installed_rows(w, list_h));
        } else if self.tab == DISCOVER {
            let mut l = line([plain("  "), sp("⌕ ", ORANGE)]);
            join(
                &mut l,
                text_field(
                    &self.query,
                    self.query_pos,
                    true,
                    &format!("search {} plugins", self.available.len()),
                    w.saturating_sub(6),
                ),
            );
            out.push(l);
            out.push(Line::new());
            out.extend(self.discover_rows(w, list_h.saturating_sub(2)));
        } else {
            out.extend(self.market_rows(w, list_h));
        }
        out.push(Line::new());
        out.push(line([sp("─".repeat(w), FAINT)]));
        out.extend(detail);
        out.push(Line::new());
        out.push(self.footer(w));
        out
    }
}

/// Runs a change on the open /plugins, once a question said yes.
fn rerun(m: &mut Model, what: String, dir: String, c: PluginChange) -> Out {
    with_sheet(m, |p: &mut PluginSheet, m| p.run(m, what, dir, c))
}

impl PluginSheet {
    fn installed_rows(&self, w: usize, h: usize) -> Vec<Line> {
        if self.installed.is_empty() {
            return vec![line([sp("  nothing installed yet · ] to Discover", DIM)])];
        }
        let cur = self.cur[INSTALLED].min(self.installed.len() - 1);
        let (from, to) = window(self.installed.len(), cur, h);
        let mut out = Vec::new();
        for i in from..to {
            let pl = &self.installed[i];
            let (mark, col) = if pl.enabled {
                (sp("●", GREEN), TEXT)
            } else {
                (sp("○", DIM), DIM)
            };
            let mut scope = pl.scope.clone();
            if scope == pl.marketplace {
                scope.clear(); // claude.ai's synced plugins
            }
            if !pl.project_path.is_empty() {
                scope += &format!(" · {}", tildify(&pl.project_path));
            }
            let l = line([
                mark,
                plain(" "),
                spb(fit_str(&pl.name, 24), col),
                plain(" "),
                sp(fit_str(&pl.version, 9), DIM),
                plain(" "),
                sp(fit_str(&pl.marketplace, 24), DIM),
                plain(" "),
                sp(
                    rush_draw::truncate(&scope, w.saturating_sub(66), "…").into_owned(),
                    FAINT,
                ),
            ]);
            out.push(sheet_row(l, i == cur, w));
        }
        out
    }

    fn discover_rows(&self, w: usize, h: usize) -> Vec<Line> {
        let list = self.shown();
        if list.is_empty() {
            return vec![line([sp("  no plugins match", DIM)])];
        }
        let cur = self.cur[DISCOVER].min(list.len() - 1);
        let (from, to) = window(list.len(), cur, h);
        let mut out = Vec::new();
        for (i, pl) in list.iter().enumerate().take(to).skip(from) {
            let l = line([
                spb(fit_str(&pl.name, 26), TEXT),
                plain(" "),
                sp(fit_str(&installs(pl.installs), 7), DIM),
                plain(" "),
                sp(
                    rush_draw::truncate(&one_line(&pl.description), w.saturating_sub(40), "…")
                        .into_owned(),
                    FAINT,
                ),
            ]);
            out.push(sheet_row(l, i == cur, w));
        }
        out
    }

    fn market_rows(&self, w: usize, h: usize) -> Vec<Line> {
        if self.markets.is_empty() {
            return vec![line([sp("  no marketplaces · a adds one", DIM)])];
        }
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for pl in self.available.iter().chain(&self.installed) {
            *counts.entry(pl.marketplace.as_str()).or_default() += 1;
        }
        let cur = self.cur[MARKETS].min(self.markets.len() - 1);
        let (from, to) = window(self.markets.len(), cur, h);
        let mut out = Vec::new();
        for i in from..to {
            let mk = &self.markets[i];
            let n = counts.get(mk.name.as_str()).copied().unwrap_or(0);
            let l = line([
                spb(fit_str(&mk.name, 28), TEXT),
                plain(" "),
                sp(fit_str(&format!("{n} plugins"), 12), DIM),
                plain(" "),
                sp(
                    rush_draw::truncate(mk.place(), w.saturating_sub(46), "…").into_owned(),
                    FAINT,
                ),
            ]);
            out.push(sheet_row(l, i == cur, w));
        }
        out
    }

    /// The selected row, in full: what it is, what it brings (detail).
    fn detail(&self, w: usize) -> Vec<Line> {
        let wrap_dim = |s: &str| -> Vec<Line> {
            wrap(&one_line(s), w.saturating_sub(4))
                .into_iter()
                .take(3)
                .map(|l| line([plain("  "), sp(l, SUB)]))
                .collect()
        };
        match self.tab {
            INSTALLED => {
                if self.installed.is_empty() {
                    return Vec::new();
                }
                let pl = &self.installed[self.cur[INSTALLED].min(self.installed.len() - 1)];
                let mut out = vec![line([plain("  "), spb(pl.id.clone(), TEXT)])];
                out.extend(wrap_dim(or(&pl.description, "no description")));
                let mut parts = Vec::new();
                let mut add = |n: &[String], one: &str, many: &str| match n.len() {
                    0 => {}
                    1 => parts.push(format!("1 {one}: {}", n[0])),
                    k => parts.push(format!("{k} {many}: {}", n.join(", "))),
                };
                add(&pl.parts.skills, "skill", "skills");
                add(&pl.parts.agents, "agent", "agents");
                add(&pl.parts.commands, "command", "commands");
                add(&pl.parts.hooks, "hook", "hooks");
                add(&pl.parts.mcp, "MCP server", "MCP servers");
                if let Some(c) = self.costs.get(&pl.id) {
                    parts.push(format!("{c} in every session"));
                }
                if !parts.is_empty() {
                    out.push(line([
                        plain("  "),
                        sp(
                            rush_draw::truncate(&parts.join(" · "), w.saturating_sub(4), "…")
                                .into_owned(),
                            DIM,
                        ),
                    ]));
                }
                out
            }
            DISCOVER => {
                let list = self.shown();
                if list.is_empty() {
                    return Vec::new();
                }
                let pl = list[self.cur[DISCOVER].min(list.len() - 1)];
                let mut out = vec![line([
                    plain("  "),
                    spb(pl.id.clone(), TEXT),
                    sp(format!("  {} installs", installs(pl.installs)), DIM),
                ])];
                out.extend(wrap_dim(or(&pl.description, "no description")));
                out
            }
            _ => {
                if self.markets.is_empty() {
                    return Vec::new();
                }
                let mk = &self.markets[self.cur[MARKETS].min(self.markets.len() - 1)];
                vec![
                    line([plain("  "), spb(mk.name.clone(), TEXT)]),
                    line([
                        plain("  "),
                        sp(format!("{} · {}", mk.source, mk.place()), DIM),
                    ]),
                ]
            }
        }
    }

    fn footer(&self, w: usize) -> Line {
        if self.adding {
            let mut l = line([sp("add a marketplace ❯ ", ORANGE)]);
            join(
                &mut l,
                text_field(
                    &self.source,
                    self.source_pos,
                    true,
                    "owner/repo, a URL or a folder",
                    w.saturating_sub(24),
                ),
            );
            return l;
        }
        if !self.busy.is_empty() {
            return line([sp(format!("⋯ {}", self.busy), YELLOW)]);
        }
        if !self.err.is_empty() {
            return line([sp(rush_draw::truncate(&self.err, w, "…").into_owned(), RED)]);
        }
        let room = w.saturating_sub(30);
        let mut k = match self.tab {
            INSTALLED => keys_fit(
                room,
                &[
                    ("space", "on/off"),
                    ("u", "update"),
                    ("x", "remove"),
                    ("[ ]", "discover"),
                    ("esc", "done"),
                ],
            ),
            DISCOVER => keys_fit(
                room,
                &[
                    ("type", "search"),
                    ("enter", "install"),
                    ("[ ]", "marketplaces"),
                    ("esc", "done"),
                ],
            ),
            _ => keys_fit(
                room,
                &[
                    ("a", "add"),
                    ("u", "update"),
                    ("U", "update all"),
                    ("x", "remove"),
                    ("esc", "done"),
                ],
            ),
        };
        if !self.done.is_empty() {
            let mut l = line([sp(format!("✓ {}", self.done), GREEN), plain("   ")]);
            join(&mut l, k);
            k = l;
        }
        if self.changed {
            k.spans.push(sp("  · sessions reload plugins", DIM));
        }
        k
    }
}
