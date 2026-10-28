//! The screens' state, their keys and mouse, and the frame. After Go's
//! internal/ui/model.go (Update, focused :2122, the tick :1081),
//! view.go (title :44, listView :1095, cardLines :1828, ctxBar :1919,
//! frame/statusOr :663-731, keys :859) and keys.go (listKey :273). The
//! UI thread owns all of it; workers send it Notes (worker.rs).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rush_claude::fleet::Predicates;
use rush_core::agent::{Agent, Preview, State};
use rush_core::text::{Line, Span, width};
use rush_core::time::{Ms, now};
use rush_draw::{App, Button, Canvas, Event, Key, KeyMods, MouseKind, Msg, Sender, Step};

use crate::config::Config;
use crate::list::ListLine;
use crate::session::{self, SessionView};
use crate::style::*;
use crate::worker::{self, Fleet, Note};

pub struct Model {
    pub agents: Vec<Agent>,
    pub roots: Vec<String>,
    pub previews: HashMap<String, Preview>,
    pub cfg: Config,
    pub now: Ms,
    pub tick: usize,
    pub w: usize,
    pub h: usize,
    pub lines: Vec<ListLine>,
    pub order: Vec<usize>,
    pub group_of: HashMap<String, String>,
    pub sel: String,
    pub shown: String,
    pub hover: String,
    pub scroll: usize,
    pub row_keys: Vec<String>,
    pub list_top: usize,
    pub folds: HashMap<String, bool>,
    pub titles: HashMap<String, String>,
    pub run_since: HashMap<String, Ms>,
    pub run_init: bool,
    pub loaded: bool,
    pub account: String,
    pub launch_dir: String,
    pub version: String,
    /// A fixed clock, for tests; None reads the real one.
    pub clock: Option<Ms>,
    flash: Option<(String, Ms)>,
    quit_armed: Ms,
    help: bool,
    term_focused: bool,
    pub session: Option<SessionView>,
    tx: Option<Sender<Note>>,
    r#gen: u64,
    title: String,
    /// A spinner or clock was drawn: the next frame is due in a second.
    spinning: bool,
    ticked: Instant,
    asked: HashMap<String, Ms>,
}

impl Model {
    /// A model with no workers: fed by hand (tests, rush --render).
    pub fn new(version: &str) -> Model {
        Model {
            agents: Vec::new(),
            roots: Vec::new(),
            previews: HashMap::new(),
            cfg: Config::default(),
            now: now(),
            tick: 0,
            w: 80,
            h: 24,
            lines: Vec::new(),
            order: Vec::new(),
            group_of: HashMap::new(),
            sel: String::new(),
            shown: String::new(),
            hover: String::new(),
            scroll: 0,
            row_keys: Vec::new(),
            list_top: 0,
            folds: HashMap::new(),
            titles: HashMap::new(),
            run_since: HashMap::new(),
            run_init: false,
            loaded: false,
            account: "default".into(),
            launch_dir: std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
            version: version.to_string(),
            clock: None,
            flash: None,
            quit_armed: 0,
            help: false,
            term_focused: true,
            session: None,
            tx: None,
            r#gen: 0,
            title: String::new(),
            spinning: false,
            ticked: Instant::now(),
            asked: HashMap::new(),
        }
    }

    /// A model whose workers send on tx.
    pub fn with_workers(version: &str, tx: Sender<Note>) -> Model {
        let mut m = Model::new(version);
        m.tx = Some(tx);
        m
    }

    /// Takes a reading of the list.
    pub fn set_fleet(&mut self, f: Fleet) {
        self.now = self.clock.unwrap_or_else(now);
        self.agents = f.agents;
        self.roots = f.roots;
        // Forget agents that are gone.
        let keys: HashSet<&str> = self.agents.iter().map(|a| a.key.as_str()).collect();
        self.asked.retain(|k, _| keys.contains(k.as_str()));
        // Live previews come with the reading; a picked one (read wider)
        // stays while it's as new, else it's read again.
        self.previews.retain(|k, _| self.asked.contains_key(k));
        for (k, p) in f.previews {
            if let Some(old) = self.previews.get(&k)
                && old.at >= p.at
            {
                continue;
            }
            self.asked.remove(&k);
            self.previews.insert(k, p);
        }
        self.loaded = true;
        self.rebuild();
        self.ask_preview();
    }

    /// The agent whose card is open: the selection, or on a heading the
    /// agent picked before it.
    pub fn focused(&self) -> Option<usize> {
        let key = if self.sel.starts_with('§') {
            &self.shown
        } else {
            &self.sel
        };
        self.order
            .iter()
            .copied()
            .find(|&i| self.agents[i].key == *key)
    }

    fn select(&mut self, key: String) {
        if !self.sel.is_empty() && !self.sel.starts_with('§') {
            self.shown = self.sel.clone();
        }
        self.sel = key;
        self.ask_preview();
    }

    /// The picked agent's preview, read on a worker: once, and again when
    /// its transcript moves on.
    fn ask_preview(&mut self) {
        let Some(tx) = &self.tx else { return };
        let Some(i) = self.selected() else { return };
        let a = &self.agents[i];
        if a.job.transcript_path.is_empty() {
            return;
        }
        if self.asked.contains_key(&a.key) {
            return;
        }
        self.asked.insert(a.key.clone(), now());
        worker::spawn_preview(tx.clone(), a.key.clone(), a.job.transcript_path.clone());
    }

    fn flash(&mut self, s: &str) {
        self.flash = Some((s.to_string(), now()));
    }

    /// Opens agent i's conversation.
    pub fn open(&mut self, i: usize) {
        let a = &self.agents[i];
        self.r#gen += 1;
        if a.job.transcript_path.is_empty() {
            self.flash("no transcript to show for this one yet");
            return;
        }
        let mut v = SessionView::new(
            a.key.clone(),
            PathBuf::from(&a.job.transcript_path),
            self.r#gen,
        );
        match &self.tx {
            Some(tx) => {
                v.tail = worker::spawn_tail(
                    v.path.clone(),
                    self.r#gen,
                    tx.clone(),
                    rush_claude::messages,
                );
            }
            None => v.read_now(),
        }
        self.session = Some(v);
    }

    fn close(&mut self) {
        self.session = None; // the tail's worker stops with its handle
    }

    fn quit_key(&mut self) -> Step {
        Step::Quit
    }

    /// Handles a key; Quit when it ends rush.
    pub fn key(&mut self, code: Key, mods: KeyMods) -> Step {
        let s = rush_draw::key_string(code, mods);
        self.hover.clear();
        if s == "ctrl+c" {
            return self.quit_key();
        }
        if self.help {
            self.help = false;
            return Step::Draw;
        }
        if s == "?" || s == "f1" {
            self.help = true;
            return Step::Draw;
        }
        if self.session.is_some() {
            return self.session_key(&s);
        }
        match s.as_str() {
            "up" | "k" => self.step(-1),
            "down" | "j" => self.step(1),
            "pgup" => self.step(-10),
            "pgdown" => self.step(10),
            "home" | "shift+up" | "g" => self.step(-(self.lines.len() as isize)),
            "end" | "shift+down" | "G" => self.step(self.lines.len() as isize),
            "ctrl+n" => {
                let order = self.order.clone();
                let at = order
                    .iter()
                    .position(|&i| self.agents[i].key == self.sel)
                    .map_or(0, |p| p + 1);
                let next = order[at.min(order.len())..]
                    .iter()
                    .chain(order.iter())
                    .find(|&&i| self.agents[i].needs_you())
                    .copied();
                match next {
                    Some(i) => {
                        let k = self.agents[i].key.clone();
                        self.select(k);
                    }
                    None => self.flash("nothing needs you"),
                }
            }
            "enter" | "right" | "l" => {
                if let Some(t) = self.sel.strip_prefix('§').map(str::to_string) {
                    if s == "enter" || self.folded(&t) {
                        self.toggle_fold(&t);
                    }
                } else if let Some(i) = self.selected() {
                    self.open(i);
                }
            }
            "left" | "h" => {
                if let Some(t) = self.sel.strip_prefix('§').map(str::to_string)
                    && !self.folded(&t)
                {
                    self.toggle_fold(&t);
                }
            }
            "q" => return self.quit_key(),
            "esc" => {
                if now() - self.quit_armed < 2000 {
                    return self.quit_key();
                }
                self.quit_armed = now();
                self.flash("esc again to quit");
            }
            _ => return Step::Idle,
        }
        Step::Draw
    }

    fn step(&mut self, d: isize) {
        self.move_sel(d);
        self.ask_preview();
    }

    fn session_rows(&self) -> usize {
        self.h
            .saturating_sub(self.top_h() + session::HEAD_H + 1)
            .max(1)
    }

    fn session_key(&mut self, s: &str) -> Step {
        let rows = self.session_rows();
        let (w, now, tick) = (self.w.saturating_sub(3), self.now, self.tick);
        let Some(v) = self.session.as_mut() else {
            return Step::Idle;
        };
        let o = v.options(w, now, tick);
        match s {
            "esc" | "q" => {
                if !v.sel.is_empty() && s == "esc" {
                    v.sel.clear();
                } else {
                    self.close();
                }
            }
            "up" | "k" => v.move_sel(-1, &o, rows),
            "down" | "j" => v.move_sel(1, &o, rows),
            "enter" | "space" | " " => v.toggle(&o),
            "right" | "l" => {
                if !v.sel.is_empty() {
                    v.open.insert(v.sel.clone(), true);
                }
            }
            "left" | "h" => {
                if !v.sel.is_empty() {
                    v.open.insert(v.sel.clone(), false);
                }
            }
            "pgup" => v.scroll(rows as isize - 1),
            "pgdown" => v.scroll(-(rows as isize - 1)),
            "home" | "g" => v.from_bottom = usize::MAX / 4,
            "end" | "G" => {
                v.from_bottom = 0;
                v.sel.clear();
            }
            "ctrl+o" => v.verbose = !v.verbose,
            _ => return Step::Idle,
        }
        Step::Draw
    }

    fn mouse(&mut self, kind: MouseKind, x: u16, y: u16) -> Step {
        let _ = x;
        let y = y as usize;
        if let Some(v) = self.session.as_mut() {
            match kind {
                MouseKind::ScrollUp => v.scroll(3),
                MouseKind::ScrollDown => v.scroll(-3),
                MouseKind::Down(Button::Left) => {
                    let Some(r) = v.ref_at(y).map(str::to_string) else {
                        return Step::Idle;
                    };
                    v.sel = r;
                    let o = v.options(self.w.saturating_sub(3), self.now, self.tick);
                    v.toggle(&o);
                }
                _ => return Step::Idle,
            }
            return Step::Draw;
        }
        let at = y
            .checked_sub(self.list_top)
            .and_then(|i| self.row_keys.get(i))
            .cloned()
            .unwrap_or_default();
        match kind {
            MouseKind::ScrollUp => self.step(-1),
            MouseKind::ScrollDown => self.step(1),
            MouseKind::Moved => {
                if at == self.hover {
                    return Step::Idle;
                }
                self.hover = at;
            }
            MouseKind::Down(Button::Left) => {
                if at.is_empty() {
                    return Step::Idle;
                }
                if let Some(t) = at.strip_prefix('§') {
                    let t = t.to_string();
                    self.select(at);
                    self.toggle_fold(&t);
                } else if at == self.sel {
                    if let Some(i) = self.selected() {
                        self.open(i);
                    }
                } else {
                    self.select(at);
                }
            }
            _ => return Step::Idle,
        }
        Step::Draw
    }

    /// The window title: where you are, and how many wait on you.
    fn make_title(&self) -> String {
        let where_ = match &self.session {
            Some(v) => self
                .agents
                .iter()
                .find(|a| a.key == v.key)
                .map(|a| one_line(&a.display_name))
                .unwrap_or_else(|| "Agents".into()),
            None => "Agents".into(),
        };
        let n = self
            .agents
            .iter()
            .filter(|a| a.needs_you() || a.halted())
            .count();
        if n > 0 {
            format!("({n}) rush · {where_}")
        } else {
            format!("rush · {where_}")
        }
    }

    /// The frame as lines, top to bottom, h of them.
    pub fn frame(&mut self) -> Vec<Line> {
        let (w, h) = (self.w, self.h);
        self.now = self.clock.unwrap_or_else(now);
        let mut out = self.header();
        let head = out.len();
        let body_h = h.saturating_sub(head + 1);
        if self.session.is_some() {
            self.session_frame(&mut out, body_h);
        } else {
            self.list_frame(&mut out, body_h);
        }
        out.truncate(h.saturating_sub(1));
        while out.len() < h.saturating_sub(1) {
            out.push(Line::new());
        }
        out.push(self.status_line());
        if self.help {
            self.help_over(&mut out);
        }
        out.truncate(h);
        let _ = w;
        out
    }

    fn list_frame(&mut self, out: &mut Vec<Line>, body_h: usize) {
        let w = self.w;
        let dl = self.cfg.dock_lines(self.h);
        let mut dock = Vec::new();
        if self.h >= 20 + dl {
            match self.focused() {
                Some(i) => {
                    for l in self.card_lines(i, w.saturating_sub(4), dl) {
                        let mut r = line([plain("  ")]);
                        join(&mut r, l);
                        dock.push(r);
                    }
                }
                None => {
                    dock.push(Line::new());
                    dock.push(line([sp(
                        "  select an agent to see what it is doing",
                        FAINT,
                    )]));
                }
            }
            while dock.len() < 4 + dl {
                dock.push(Line::new());
            }
        }
        if body_h < dock.len() + 5 {
            dock.clear();
        }
        let body_h = body_h.saturating_sub(dock.len()).max(3);
        self.list_top = out.len() + 1;
        out.push(self.column_header(w));
        let rows = self.list_lines(w, body_h - 1);
        out.extend(rows);
        let target = self.list_top - 1 + body_h;
        while out.len() < target {
            out.push(Line::new());
        }
        out.extend(dock);
        let live = self.order.iter().any(|&i| {
            let a = &self.agents[i];
            a.job.live() && !a.needs_you()
        });
        self.spinning = live;
    }

    fn session_frame(&mut self, out: &mut Vec<Line>, body_h: usize) {
        let (w, now, tick) = (self.w, self.now, self.tick);
        let Some(v) = self.session.as_mut() else {
            return;
        };
        let a = self.agents.iter().find(|a| a.key == v.key);
        out.extend(session::header(a, v, w, now));
        let rows = body_h.saturating_sub(session::HEAD_H);
        v.top = out.len();
        let iw = w.saturating_sub(3);
        let o = v.options(iw, now, tick);
        let body = if !v.read {
            vec![line([sp("reading the conversation…", DIM)])]
        } else if !v.err.is_empty() && v.sess.turns.is_empty() {
            vec![line([sp(format!("couldn't read it: {}", v.err), RED)])]
        } else {
            v.body(&o, rows)
        };
        for l in body.into_iter().take(rows) {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            out.push(r);
        }
        self.spinning = v.sess.live().is_some() || a.is_some_and(|a| a.job.live());
    }

    /// The bottom row: a flash for a few seconds, else the keys.
    fn status_line(&self) -> Line {
        let w = self.w;
        if let Some((s, at)) = &self.flash
            && self.now - at < 6000
        {
            return fit(line([plain("  "), sp(s.clone(), TEXT)]), w);
        }
        let keys = if self.session.is_some() {
            keys_fit(
                w.saturating_sub(2),
                &[
                    ("↑↓", "pick a row"),
                    ("enter", "open or close it"),
                    ("ctrl+o", "more detail"),
                    ("esc", "back to the list"),
                ],
            )
        } else {
            keys_fit(
                w.saturating_sub(2),
                &[
                    ("enter", "open"),
                    ("↑↓", "choose"),
                    ("ctrl+n", "next that needs you"),
                    ("ctrl+c", "quit"),
                    ("?", "more"),
                ],
            )
        };
        let mut l = line([plain("  ")]);
        join(&mut l, keys);
        fit(l, w)
    }

    /// The keys, in a box over the middle of the screen.
    fn help_over(&self, out: &mut [Line]) {
        let pairs: &[(&str, &str)] = &[
            ("↑↓ j k", "move"),
            ("pgup pgdn", "a page"),
            ("home end", "first, last"),
            ("enter →", "open the agent, or a section"),
            ("←", "fold a section"),
            ("ctrl+n", "the next that needs you"),
            ("esc esc · q", "quit"),
            ("", ""),
            ("in a session", ""),
            ("↑↓", "pick a row"),
            ("enter space", "open or close it"),
            ("pgup pgdn", "scroll"),
            ("end", "follow the conversation"),
            ("ctrl+o", "more detail"),
            ("esc", "back to the list"),
            ("", ""),
            ("ctrl+c", "quit"),
        ];
        let bw = 52.min(self.w.saturating_sub(4));
        let inner = bw.saturating_sub(4);
        let mut rows = vec![line([sp(
            format!("╭─ keys {}╮", "─".repeat(bw.saturating_sub(9))),
            DIM,
        )])];
        for (k, what) in pairs {
            let body = if what.is_empty() {
                line([spb(*k, SUB)])
            } else {
                line([spb(fit_str(k, 14), TEXT), sp(*what, DIM)])
            };
            let mut l = line([sp("│ ", DIM)]);
            join(&mut l, on_ground(body, inner, PANEL_BG));
            l.spans.push(sp(" │", DIM));
            rows.push(l);
        }
        rows.push(line([sp(
            format!("╰{}╯", "─".repeat(bw.saturating_sub(2))),
            DIM,
        )]));
        let top = out.len().saturating_sub(rows.len()) / 2;
        let left = self.w.saturating_sub(bw) / 2;
        for (i, r) in rows.into_iter().enumerate() {
            let Some(base) = out.get_mut(top + i) else {
                break;
            };
            *base = overlay(base, r, left, self.w);
        }
    }

    /// The focused row's card: what it's doing in the title, its latest
    /// words inside, its numbers in a footer.
    fn card_lines(&self, i: usize, w: usize, body_lines: usize) -> Vec<Line> {
        let a = &self.agents[i];
        let empty = Preview::default();
        let p = self.previews.get(&a.key).unwrap_or(&empty);
        let inner = w.saturating_sub(6);
        let edge = |s: &str| sp(s, DIM);
        let row = |l: Line| {
            let mut out = line([edge("│")]);
            let mut body = line([plain("  ")]);
            join(&mut body, fit(l, inner));
            body.spans.push(plain("  "));
            join(&mut out, on_ground(body, inner + 4, PANEL_BG));
            out.spans.push(edge("│"));
            out
        };
        let live = a.job.live();
        let title = if live && !p.tool.is_empty() {
            let arg = one_line(&tildify(&p.tool_arg));
            line([
                sp("● ", ORANGE),
                spb(p.tool.clone(), TEXT),
                plain("  "),
                sp(
                    rush_draw::truncate(&arg, 10.max(inner.saturating_sub(p.tool.len() + 8)), "…")
                        .into_owned(),
                    SUB,
                ),
            ])
        } else if a.job.state == State::Blocked {
            line([spb("waiting on you", YELLOW)])
        } else if a.busy() {
            line([sp("◌ ", ORANGE), sp("background work still running", TEXT)])
        } else if live {
            line([sp("● ", ORANGE), sp("working…", TEXT)])
        } else if a.pid != 0 {
            line([sp("idle · still in memory", DIM)])
        } else {
            line([sp(format!("finished {} ago", age(a.age(self.now))), DIM)])
        };
        let tw = title.width();
        let mut top = line([edge("╭─ ")]);
        join(&mut top, title);
        top.spans
            .push(edge(&format!(" {}╮", "─".repeat(w.saturating_sub(tw + 5)))));
        let mut text = p.text.clone();
        if !live && !a.job.detail.trim().is_empty() && a.job.detail != "stopped" {
            text = a.job.detail.clone();
        }
        if a.job.state == State::Blocked && !a.job.needs.is_empty() {
            text = a.job.needs.clone();
        }
        if text.is_empty() {
            text = "…".into();
        }
        let text = unmark(&one_line(&text));
        let wrapped = wrap(&text, inner);
        let mut body = Vec::new();
        for (n, l) in wrapped.iter().enumerate() {
            if n == body_lines - 1 && wrapped.len() > body_lines {
                body.push(line([
                    sp(
                        rush_draw::truncate(l, inner.saturating_sub(24), "…").into_owned(),
                        TEXT,
                    ),
                    sp("   tab for the full preview", FAINT),
                ]));
                break;
            }
            body.push(line([sp(l.clone(), TEXT)]));
        }
        let mut cells: Vec<Line> = Vec::new();
        if p.context > 0 {
            let model = if p.model.is_empty() {
                &a.spend.model
            } else {
                &p.model
            };
            let win = rush_claude::pricing::context_window(model);
            let pct = if win > 0 {
                p.context as f64 * 100.0 / win as f64
            } else {
                0.0
            };
            let mut c = line([sp("context ", DIM)]);
            join(&mut c, ctx_bar(pct));
            c.spans.push(plain(" "));
            c.spans.push(sp(format!("{pct:.0}%"), TEXT));
            c.spans.push(sp(format!(" of {}", tokens(win)), DIM));
            cells.push(c);
        }
        let u = a.spend.usage;
        if a.spend.cost > 0.0 {
            cells.push(line([sp("spent ", DIM), sp(money(a.spend.cost), TEXT)]));
            cells.push(line([
                sp("out ", DIM),
                sp(tokens(u.output), SUB),
                sp("  cache ", DIM),
                sp(tokens(u.cache_read), SUB),
            ]));
        }
        // Its processes (view.go:1894).
        if a.pid != 0 && a.procs > 0 {
            cells.push(line([
                sp(format!("{} procs ", a.procs), DIM),
                sp(crate::style::mem(a.mem), SUB),
            ]));
        }
        if !live {
            let el = a.elapsed(self.now);
            if el > 0 {
                cells.push(line([sp("ran ", DIM), sp(dur(el), SUB)]));
            }
        }
        let c = crate::list::location_summary(a, &self.roots[i]);
        if !c.is_empty() {
            cells.push(line([sp(c, FAINT)]));
        }
        while body.len() < body_lines {
            body.push(Line::new());
        }
        let mut out = vec![top];
        for l in body.into_iter().take(body_lines) {
            out.push(row(l));
        }
        out.push(line([edge(&format!(
            "├{}┤",
            "─".repeat(w.saturating_sub(2))
        ))]));
        let mut foot = Line::new();
        for (n, c) in cells.into_iter().enumerate() {
            if n > 0 {
                foot.spans.push(edge("   │   "));
            }
            join(&mut foot, c);
        }
        out.push(row(foot));
        out.push(line([edge(&format!(
            "╰{}╯",
            "─".repeat(w.saturating_sub(2))
        ))]));
        out
    }

    /// Whether anything on screen moves with the clock.
    pub fn spinning(&self) -> bool {
        self.spinning
    }
}

fn ctx_bar(pct: f64) -> Line {
    let n = ((pct / 12.5 + 0.5) as usize).min(8);
    let c = if pct >= 80.0 {
        RED
    } else if pct >= 50.0 {
        YELLOW
    } else {
        SUB
    };
    line([sp("━".repeat(n), c), sp("━".repeat(8 - n), FAINT)])
}

/// Greedy word wrap to w cells; a word longer than w is cut.
fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(8);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ').filter(|x| !x.is_empty()) {
        let ww = width(word);
        if !cur.is_empty() && width(&cur) + 1 + ww > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        if ww > w {
            cur.push_str(&rush_draw::truncate(word, w, "…"));
        } else {
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `top` laid over `base` from column x.
fn overlay(base: &Line, top: Line, x: usize, w: usize) -> Line {
    let base = fit(base.clone(), w);
    let left = rush_draw::truncate_line(&base, x, "");
    let tw = top.width();
    let mut out = pad(left, x);
    join(&mut out, top);
    // What shows right of the box: the base's cells past it.
    let skip = x + tw;
    let mut col = 0;
    for s in base.spans {
        let sw = s.width();
        if col + sw <= skip {
            col += sw;
            continue;
        }
        if col >= skip {
            out.spans.push(s);
        } else {
            let mut text = String::new();
            let mut c = col;
            for ch in s.text.chars() {
                let cw = width(ch.encode_utf8(&mut [0; 4]));
                if c >= skip {
                    text.push(ch);
                } else if c + cw > skip {
                    text.push(' ');
                }
                c += cw;
            }
            out.spans.push(Span::new(text, s.style));
        }
        col += sw;
    }
    out
}

/// The local hour at ms.
pub fn local_hour(ms: Ms) -> i32 {
    local_tm(ms).map_or(12, |t| t.tm_hour)
}

/// Seconds east of UTC here, at ms.
pub fn local_zone(ms: Ms) -> i64 {
    local_tm(ms).map_or(0, |t| t.tm_gmtoff)
}

fn local_tm(ms: Ms) -> Option<libc::tm> {
    let t = (ms / 1000) as libc::time_t;
    // SAFETY: localtime_r writes into the tm we own.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            None
        } else {
            Some(tm)
        }
    }
}

impl App<Note> for Model {
    fn update(&mut self, msg: Msg<Note>) -> Step {
        match msg {
            Msg::Input(ev) => match ev {
                Event::Key { code, mods } => self.key(code, mods),
                Event::Mouse { kind, x, y, .. } => self.mouse(kind, x, y),
                Event::Resize(w, h) => {
                    self.w = w as usize;
                    self.h = h as usize;
                    Step::Draw
                }
                Event::FocusGained => {
                    self.term_focused = true;
                    Step::Draw
                }
                Event::FocusLost => {
                    self.term_focused = false;
                    Step::Idle
                }
                Event::Paste(_) => Step::Idle,
            },
            Msg::App(note) => match note {
                Note::Fleet(f) => {
                    self.set_fleet(f);
                    Step::Draw
                }
                Note::Preview(k, p) => {
                    self.previews.insert(k, *p);
                    Step::Draw
                }
                Note::Tail { r#gen, fresh } => match &mut self.session {
                    Some(v) if v.r#gen == r#gen => {
                        v.take(*fresh);
                        Step::Draw
                    }
                    _ => Step::Idle,
                },
                Note::TailFailed { r#gen, err } => match &mut self.session {
                    Some(v) if v.r#gen == r#gen => {
                        v.read = true;
                        v.err = err;
                        Step::Draw
                    }
                    _ => Step::Idle,
                },
            },
        }
    }

    fn draw(&mut self, c: &mut Canvas) {
        self.w = c.width() as usize;
        self.h = c.height() as usize;
        let rows = self.frame();
        for (y, l) in rows.iter().enumerate() {
            c.put_line(0, y as u16, l, self.w as u16);
        }
        self.title = self.make_title();
    }

    fn wake_at(&self) -> Option<Instant> {
        let mut at = None;
        if self.spinning && self.term_focused {
            let fast = self.session.as_ref().is_some_and(|v| v.r.fast());
            let every = if fast {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(1)
            };
            at = Some(self.ticked + every);
        }
        if let Some((_, t)) = &self.flash {
            let left = 6000 - (now() - t);
            if left > 0 {
                let f = Instant::now() + Duration::from_millis(left as u64);
                at = Some(at.map_or(f, |a: Instant| a.min(f)));
            }
        }
        at
    }

    fn wake(&mut self) -> Step {
        if self.ticked.elapsed() >= Duration::from_millis(950) {
            self.tick += 1;
            self.ticked = Instant::now();
        } else if self.session.as_ref().is_some_and(|v| v.r.fast()) {
            self.ticked = Instant::now();
        }
        if self.flash.as_ref().is_some_and(|(_, t)| now() - t >= 6000) {
            self.flash = None;
        }
        Step::Draw
    }

    fn title(&self) -> Option<&str> {
        Some(&self.title)
    }
}
