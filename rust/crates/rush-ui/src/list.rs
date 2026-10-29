//! The agent list in the rail look: sections, rows, columns. After Go's
//! internal/ui/model.go (rebuild :2215, sections :2150, stale :2612,
//! asks :2620, waitedLess/stoppedLess/runLess :2537-2620, as of a4c94f59,
//! noteRuns :2572, sectionMeta :2644, move :2069, items :2090),
//! view.go (listLines :1619, listRowLine :1749, rowWash :1785,
//! sectionLine :1803, colWidths :1937, columnHeader :1965, nameColumn
//! :2041, widestName :2053, stacked :2077, agentSub :2091, agentLine
//! :2199, saysWhy :2389, cacheWheel :2396, rowSummary :2415, activity
//! :2524, backgroundText :2548, costCell :2589, badges :2597), rail.go
//! (railColor :28, railSection :43, railLast :59, agentWhere :76,
//! railRows :150), folders.go (folderKey :68, folderTitles :85, treeOf
//! :122), wall.go:646 (wallProject, projectTint), location.go:47
//! (locationSummary) and host/tmp.go:39 (IsTemp).

use std::collections::HashMap;

use rush_core::agent::{Agent, State};
use rush_core::text::{Color, Line, width};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use crate::model::Model;
use crate::style::*;

pub const NEEDS: &str = "Needs you";
pub const FINISHED: &str = "Finished";
pub const STUCK: &str = "Stuck?";
pub const WORKING: &str = "Working";
pub const IDLE: &str = "Idle";
pub const JUST_LEFT: &str = "Just left";
pub const SCRATCH: &str = "scratch";
pub const NO_FOLDER: &str = "No folder";

// Column widths on the right of a row (view.go:1507).
const W_ACT: usize = 10;
const W_CPU: usize = 6;
const W_RAM: usize = 7;
const W_TOK: usize = 7;
const W_COST: usize = 8;
const W_AGE: usize = 5;

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;
const CACHE_LIFE: Ms = HOUR;

/// One of the list's lines before it's drawn.
#[derive(Clone, Debug)]
pub enum ListLine {
    Blank,
    Section {
        title: String,
        meta: String,
        folded: bool,
        peek: String,
    },
    /// An agent, by its place in Model::agents.
    Agent(usize),
}

/// A row as drawn: which agent or section it acts on, and its part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Drawn,
    Agent,
    Where,
    Sub,
}

struct Row {
    line: Line,
    key: String,
    part: Part,
    agent: usize,
    edge: Color,
    last: bool,
    wash: Option<Color>,
    sel: bool,
    cont: bool,
}

pub fn section_key(title: &str) -> String {
    format!("§{title}")
}

/// The colour of a section's edge: what it wants of you.
fn rail_color(section: &str) -> Color {
    match section {
        NEEDS => GREEN,
        FINISHED => BLUE,
        STUCK => YELLOW,
        WORKING => ORANGE,
        _ => FAINT,
    }
}

/// An agent waiting on an answer from you, not just a next task.
pub fn asks(a: &Agent) -> bool {
    a.needs_you() || a.halted()
}

/// Left a day without a turn, neither open nor at work.
fn stale(a: &Agent, now: Ms) -> bool {
    !a.job.open() && !a.busy() && a.age(now) >= 24 * HOUR
}

/// Whether p is in a temp folder, a session's or the system's.
pub fn is_temp(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    let p = p.trim_end_matches('/');
    if p.contains("/var/folders/") || p.starts_with("/tmp/") || p.starts_with("/private/tmp/") {
        return true;
    }
    let d = crate::config::dir();
    let inside = |root: &std::path::Path| {
        let r = root.to_string_lossy();
        p.starts_with(r.as_ref()) && (p.len() == r.len() || p.as_bytes()[r.len()] == b'/')
    };
    if inside(&d.join("tmp")) {
        return true;
    }
    let root = d.join("sessions");
    let r = root.to_string_lossy();
    if !inside(&root) || p.len() == r.len() {
        return false;
    }
    let rest = &p[r.len() + 1..];
    let rest = rest.split_once('/').map_or("", |(_, r)| r);
    rest == "tmp" || rest.starts_with("tmp/")
}

/// The folder an agent's row sits under.
pub fn folder_key<'a>(a: &'a Agent, root: &'a str) -> &'a str {
    if !root.is_empty() {
        root
    } else if is_temp(&a.job.cwd) {
        SCRATCH
    } else if a.job.cwd.is_empty() {
        NO_FOLDER
    } else {
        &a.job.cwd
    }
}

/// Each folder by its last element; where two share it, by as many of
/// their last elements as tells them apart, else the whole path.
pub fn folder_titles<'a>(keys: impl Iterator<Item = &'a str>) -> HashMap<String, String> {
    let keys: Vec<&str> = {
        let mut k: Vec<&str> = keys.collect();
        k.sort_unstable();
        k.dedup();
        k
    };
    let tail = |k: &str, n: usize| {
        let parts: Vec<&str> = k.trim_matches('/').split('/').collect();
        parts[parts.len().saturating_sub(n)..].join("/")
    };
    let mut out: HashMap<String, String> = HashMap::with_capacity(keys.len());
    for n in 1..=3 {
        let mut count: HashMap<String, usize> = HashMap::new();
        for k in &keys {
            if !out.contains_key(*k) {
                *count.entry(tail(k, n)).or_default() += 1;
            }
        }
        for k in &keys {
            if out.contains_key(*k) {
                continue;
            }
            if *k == SCRATCH || *k == NO_FOLDER {
                out.insert(k.to_string(), k.to_string());
            } else if !k.starts_with('/') {
                out.insert(k.to_string(), tildify(k));
            } else if count.get(&tail(k, n)) == Some(&1) {
                out.insert(k.to_string(), tail(k, n));
            }
        }
    }
    for k in &keys {
        if !out.contains_key(*k) {
            out.insert(k.to_string(), tildify(k));
        }
    }
    out
}

/// A project's own colour, from its folder (FNV-1a).
pub fn project_tint(key: &str) -> Color {
    const PALETTE: [Color; 5] = [BLUE, GREEN, QUEUE, YELLOW, TEXT];
    let mut h: u32 = 0x811c_9dc5;
    for b in key.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    PALETTE[(h % PALETTE.len() as u32) as usize]
}

/// The base name of a path.
fn base(p: &str) -> &str {
    let p = p.trim_end_matches('/');
    p.rsplit('/').next().unwrap_or(p)
}

/// Where a live agent works: repository and branch.
pub fn location_summary(a: &Agent, root: &str) -> String {
    if a.repo.is_empty() {
        if is_temp(&a.job.cwd) {
            return format!("tmp/{}", base(&a.job.cwd));
        }
        return tildify(&a.job.cwd);
    }
    let mut s = base(if root.is_empty() { &a.repo } else { root }).to_string();
    if !a.branch.is_empty() {
        s.push_str(" · ");
        s.push_str(&a.branch);
    }
    if let Some(rel) = a.job.cwd.strip_prefix(&a.repo)
        && let Some(rel) = rel.strip_prefix('/')
        && !rel.is_empty()
    {
        s.push_str(" · ./");
        s.push_str(rel);
    }
    s
}

fn section_meta(n: usize, cost: f64) -> String {
    if cost > 0.0 {
        format!("{n} · {}", money(cost))
    } else {
        n.to_string()
    }
}

/// How much of the prompt cache's hour is left since it last wrote.
fn cache_wheel(quiet: Ms) -> (&'static str, Color) {
    let left = 1.0 - quiet as f64 / CACHE_LIFE as f64;
    if left > 0.8 {
        ("●", GREEN)
    } else if left > 0.6 {
        ("◕", GREEN)
    } else if left > 0.4 {
        ("◑", YELLOW)
    } else if left > 0.2 {
        ("◔", ORANGE)
    } else if left > 0.0 {
        ("○", RED)
    } else {
        ("⊘", FAINT)
    }
}

/// What a finished agent is still waiting on.
fn background_text(a: &Agent) -> String {
    let mut kinds: HashMap<&str, usize> = HashMap::new();
    let mut first = String::new();
    for b in &a.job.background {
        let (k, label) = b.split_once('\0').unwrap_or((b, ""));
        *kinds.entry(k).or_default() += 1;
        if first.is_empty() {
            first = one_line(label);
        }
    }
    let mut parts = Vec::new();
    for (k, name) in [
        ("shell", "shell"),
        ("agent", "subagent"),
        ("monitor", "monitor"),
        ("task", "task"),
    ] {
        if let Some(&n) = kinds.get(k)
            && n > 0
        {
            parts.push(format!("{n} {name}{}", if n > 1 { "s" } else { "" }));
        }
    }
    let n = a.subs.direct + a.subs.nested;
    if parts.is_empty() && n > 0 {
        parts.push(format!("{n} subagent{}", if n > 1 { "s" } else { "" }));
    }
    if parts.is_empty() {
        parts.push(format!("{} tasks", a.job.in_flight));
    }
    let mut s = format!("background · {} running", parts.join(", "));
    if !first.is_empty() {
        s.push_str(" · ");
        s.push_str(&first);
    }
    s
}

impl Model {
    /// Groups the agents into sections, what needs you first.
    pub fn rebuild(&mut self) {
        let now = self.now;
        struct Group {
            name: String,
            agents: Vec<usize>,
            rank: u8,
            recent: Ms,
        }
        let mut groups: Vec<Group> = Vec::new();
        let add = |groups: &mut Vec<Group>, name: &str, rank: u8, i: usize, at: Ms| {
            let g = match groups.iter().position(|g| g.name == name) {
                Some(j) => &mut groups[j],
                None => {
                    groups.push(Group {
                        name: name.to_string(),
                        agents: Vec::new(),
                        rank,
                        recent: 0,
                    });
                    groups.last_mut().unwrap()
                }
            };
            g.agents.push(i);
            g.recent = g.recent.max(at);
        };
        let mut left = Vec::new();
        let active_for = self.cfg.active_for();
        let just_left_for = self.cfg.just_left_for();
        let filters = self.ext.list_filter.clone();
        let shown: Vec<bool> = self
            .agents
            .iter()
            .map(|a| filters.iter().all(|f| f(self, a)))
            .collect();
        for (i, a) in self.agents.iter().enumerate() {
            if !shown[i] {
                continue;
            }
            let fresh = !stale(a, now);
            let at = a.job.updated_at;
            let fk = folder_key(a, &self.roots[i]);
            if asks(a) {
                add(&mut groups, NEEDS, 0, i, at);
            } else if fk == SCRATCH {
                add(&mut groups, "Scratch", 10, i, at);
            } else if fresh && (a.your_turn(now) || a.just_finished(now) && !a.checking) {
                add(&mut groups, FINISHED, 3, i, at);
            } else if a.stuck(now) {
                add(&mut groups, STUCK, 2, i, at);
            } else if a.checking || a.job.live() || a.busy() {
                add(&mut groups, WORKING, 1, i, at);
            } else if a.pid != 0 || a.age(now) < active_for {
                add(&mut groups, IDLE, 4, i, at);
            } else if fresh && a.age(now) < just_left_for {
                left.push(i);
            } else if !fresh {
                add(&mut groups, "Earlier", 9, i, at);
            } else {
                add(&mut groups, "Today", 7, i, at);
            }
        }
        left.sort_by_key(|&i| self.agents[i].age(now));
        let count = self.cfg.just_left_count();
        for (n, i) in left.into_iter().enumerate() {
            let at = self.agents[i].job.updated_at;
            if n < count {
                add(&mut groups, JUST_LEFT, 5, i, at);
            } else {
                add(&mut groups, "Today", 7, i, at);
            }
        }
        // When each agent at work began this run.
        let mut next = HashMap::new();
        for g in groups
            .iter()
            .filter(|g| g.name == WORKING || g.name == STUCK)
        {
            for &i in &g.agents {
                let k = &self.agents[i].key;
                let t = match self.run_since.get(k) {
                    Some(&t) => t,
                    None if !self.run_init => 0,
                    None => now,
                };
                next.insert(k.clone(), t);
            }
        }
        self.run_since = next;
        self.run_init = self.run_init || !self.agents.is_empty();

        groups.sort_by(|a, b| a.rank.cmp(&b.rank).then(b.recent.cmp(&a.recent)));
        let agents = &self.agents;
        let run_since = &self.run_since;
        let run_for = |a: &Agent| match run_since.get(&a.key) {
            Some(&t) if t != 0 => now - t,
            _ => a.elapsed(now),
        };
        for g in &mut groups {
            let key = |i: &usize| &agents[*i].key;
            match g.name.as_str() {
                NEEDS | STUCK => g.agents.sort_by(|x, y| {
                    let (a, b) = (&agents[*x], &agents[*y]);
                    asks(b)
                        .cmp(&asks(a))
                        .then(b.quiet(now).cmp(&a.quiet(now)))
                        .then(key(x).cmp(key(y)))
                }),
                WORKING => g.agents.sort_by(|x, y| {
                    let (a, b) = (&agents[*x], &agents[*y]);
                    run_for(b).cmp(&run_for(a)).then(key(x).cmp(key(y)))
                }),
                // Earlier and every stopped section: by the time its rows
                // show, the latest first, so the column reads in order
                // (stoppedLess).
                _ => g.agents.sort_by(|x, y| {
                    let (a, b) = (&agents[*x], &agents[*y]);
                    asks(b)
                        .cmp(&asks(a))
                        .then(a.age(now).cmp(&b.age(now)))
                        .then(key(x).cmp(key(y)))
                }),
            }
        }
        self.titles = folder_titles(
            self.agents
                .iter()
                .enumerate()
                .map(|(i, a)| folder_key(a, &self.roots[i])),
        );
        self.lines.clear();
        self.order.clear();
        self.group_of.clear();
        for g in &groups {
            let mut cost = 0.0;
            let mut names = Vec::new();
            for &i in &g.agents {
                let a = &self.agents[i];
                cost += a.spend.cost;
                if names.len() < 4 {
                    names.push(one_line(&a.display_name));
                }
            }
            let fold = self.folded(&g.name);
            let mut meta = section_meta(g.agents.len(), cost);
            if g.name == IDLE {
                let held: u64 = g
                    .agents
                    .iter()
                    .map(|&i| &self.agents[i])
                    .filter(|a| !a.job.live() && !a.busy())
                    .map(|a| a.mem)
                    .sum();
                if held > 0 {
                    meta.push_str(&format!(" · {} idle ram", mem(held)));
                }
            }
            self.lines.push(ListLine::Section {
                title: g.name.clone(),
                meta,
                folded: fold,
                peek: names.join(", "),
            });
            for &i in &g.agents {
                self.order.push(i);
                self.group_of
                    .insert(self.agents[i].key.clone(), g.name.clone());
                if !fold {
                    self.lines.push(ListLine::Agent(i));
                }
            }
            if !fold {
                self.lines.push(ListLine::Blank);
            }
        }
        let items = self.items();
        if !items.contains(&self.sel) {
            self.sel = items
                .iter()
                .find(|k| !k.starts_with('§'))
                .or(items.first())
                .cloned()
                .unwrap_or_default();
        }
    }

    pub fn folded(&self, title: &str) -> bool {
        self.folds
            .get(title)
            .copied()
            .unwrap_or_else(|| self.cfg.folded(title))
    }

    pub fn toggle_fold(&mut self, title: &str) {
        let v = !self.folded(title);
        self.folds.insert(title.to_string(), v);
        self.rebuild();
    }

    /// The rows ↑↓ stop on, in display order.
    pub fn items(&self) -> Vec<String> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                ListLine::Section { title, .. } => Some(section_key(title)),
                ListLine::Agent(i) => Some(self.agents[*i].key.clone()),
                ListLine::Blank => None,
            })
            .collect()
    }

    pub fn move_sel(&mut self, d: isize) {
        let items = self.items();
        if items.is_empty() {
            return;
        }
        let i = items.iter().position(|k| *k == self.sel).unwrap_or(0) as isize;
        let i = (i + d).clamp(0, items.len() as isize - 1) as usize;
        self.sel = items[i].clone();
        self.hover.clear();
    }

    /// The agent picked, if a row is.
    pub fn selected(&self) -> Option<usize> {
        self.agents.iter().position(|a| a.key == self.sel)
    }

    /// The list's right-hand columns at width w.
    pub fn col_widths(&self, w: usize) -> [usize; 6] {
        let mut c = [W_ACT, W_CPU, W_RAM, W_TOK, W_COST, W_AGE + 2];
        if w < 96 {
            c[0] = 0;
            c[1] = 0;
        }
        if w < 76 {
            c[3] = 0;
        }
        if w < 64 {
            c[2] = 0;
        }
        if w < 44 {
            c[4] = 0;
        }
        for (i, col) in ["running", "cpu", "ram", "tokens", "cost", "time"]
            .iter()
            .enumerate()
        {
            if !self.cfg.shows(col) {
                c[i] = 0;
            }
        }
        c
    }

    fn widest_name(&self) -> usize {
        self.lines
            .iter()
            .filter_map(|l| match l {
                ListLine::Agent(i) => Some(&self.agents[*i]),
                _ => None,
            })
            .map(|a| width(&one_line(&a.display_name)) + 2) // the ✻ badge and its space
            .max()
            .unwrap_or(0)
    }

    /// Where summaries start: wide enough for most names, never more than
    /// two fifths of the row.
    pub fn name_column(&self, w: usize) -> usize {
        let widest = self.widest_name();
        if w < 96 {
            let rest: usize = self.col_widths(w).iter().sum();
            return 12.max(widest.min((w as isize - 4 - rest as isize - 3).max(0) as usize));
        }
        20.max(widest.min(w.saturating_sub(30) * 2 / 5))
    }

    /// When rows take two lines, the summary hung under the name.
    pub fn stacked(&self, w: usize, name_col: usize) -> bool {
        let at = self.cfg.stack_percent();
        if at > 0 && w * 100 <= self.w * at {
            return true;
        }
        let rest: usize = self.col_widths(w).iter().sum::<usize>() + 1;
        (w as isize) - 3 - rest as isize - name_col as isize - 2 < 20
    }

    /// The column header; it stays put while the list scrolls.
    pub fn column_header(&self, w: usize) -> Line {
        if let Some(l) = self.ext.column_header.iter().find_map(|f| f(self, w)) {
            return l;
        }
        let [act, cpu, ram, tok, cost, time] = self.col_widths(w);
        let name_col = self.name_column(w);
        let sort = self.cfg.sort_by();
        let label = |text: &str, mode: &str| {
            if mode == sort {
                format!("{text}▾")
            } else {
                text.to_string()
            }
        };
        let col = |text: &str, mode: &str, width: usize| {
            let s = right(&label(text, mode), width);
            if width == 0 {
                sp("", DIM)
            } else if mode == sort {
                spb(s, SUB)
            } else {
                sp(s, DIM)
            }
        };
        let name = label("AGENTS", "name");
        let mut left = if sort == "name" {
            line([plain("  "), spb(fit_str(&name, name_col + 2), SUB)])
        } else {
            line([sp(format!("  {}", fit_str(&name, name_col + 2)), DIM)])
        };
        if !self.stacked(w, name_col) {
            left.spans.push(sp("LATEST", DIM));
        }
        let right_w = act + cpu + ram + tok + cost + time + 1;
        let gap = w as isize - left.width() as isize - right_w as isize;
        if gap < 1 {
            return fit(left, w);
        }
        left.spans.push(plain(blanks(gap as usize)));
        left.spans.push(if act > 0 {
            sp(right("RUNNING", act), DIM)
        } else {
            sp("", DIM)
        });
        left.spans.extend([
            col("CPU", "cpu", cpu),
            col("RAM", "ram", ram),
            col("TOKENS", "tokens", tok),
            col("COST", "cost", cost),
            col("TIME", "time", time),
            plain(" "),
        ]);
        line(left.spans)
    }

    /// The list's rows in view, h at most; notes which key each is.
    pub fn list_lines(&mut self, w: usize, h: usize) -> Vec<Line> {
        let name_col = self.name_column(w);
        let two = self.stacked(w, name_col);
        let mut rows: Vec<Row> = Vec::new();
        let (mut sel_top, mut sel_bottom) = (None::<usize>, 0usize);
        let mut edge = FAINT;
        let mut band = false;
        let lines = std::mem::take(&mut self.lines);
        for (li, l) in lines.iter().enumerate() {
            match l {
                ListLine::Section {
                    title,
                    meta,
                    folded,
                    peek,
                } => {
                    edge = rail_color(title);
                    band = false;
                    let key = section_key(title);
                    let sel = key == self.sel;
                    let line = self.section_line(title, meta, *folded, peek, w, sel);
                    if sel {
                        sel_top.get_or_insert(rows.len());
                        sel_bottom = rows.len();
                    }
                    rows.push(Row::drawn(line, key, sel));
                }
                ListLine::Blank => rows.push(Row::drawn(Line::new(), String::new(), false)),
                ListLine::Agent(i) => {
                    let a = &self.agents[*i];
                    let sel = a.key == self.sel;
                    band = !band;
                    let last = !matches!(lines.get(li + 1), Some(ListLine::Agent(_)));
                    let mut wash = self.row_wash(a, sel);
                    if wash.is_none() && band && !sel {
                        wash = Some(BAND_BG);
                    }
                    let mut push = |part: Part, cont: bool| {
                        if sel && (wash.is_none() || !a.key.is_empty()) {
                            sel_top.get_or_insert(rows.len());
                            sel_bottom = rows.len();
                        }
                        rows.push(Row {
                            line: Line::new(),
                            key: a.key.clone(),
                            part,
                            agent: *i,
                            edge,
                            last,
                            wash,
                            sel,
                            cont,
                        });
                    };
                    push(Part::Agent, false);
                    if w > 7 {
                        push(Part::Where, true);
                    }
                    if two && self.says_why(a) {
                        push(Part::Sub, true);
                    }
                }
            }
        }
        self.lines = lines;
        if self.lines.is_empty() {
            let msg = if let Some(m) = self.ext.list_empty.iter().find_map(|f| f(self)) {
                m
            } else if self.loaded {
                "  No agents yet. Describe a task below to start one.".to_string()
            } else {
                "  Reading your agents…".to_string()
            };
            rows.push(Row::drawn(Line::new(), String::new(), false));
            rows.push(Row::drawn(line([sp(msg, DIM)]), String::new(), false));
        }
        // Scrolling up shows the whole row above; down, a row cut in half
        // at the top gives up its second line instead.
        if let Some(top) = sel_top {
            if top < self.scroll + 1 {
                self.scroll = top.saturating_sub(1);
                if self.scroll > 0 && rows[self.scroll].cont {
                    self.scroll -= 1;
                }
            }
            if sel_bottom + 1 >= self.scroll + h {
                self.scroll = (sel_bottom + 2).saturating_sub(h);
            }
        }
        if self.scroll > rows.len().saturating_sub(h) {
            self.scroll = rows.len().saturating_sub(h);
        }
        if self.scroll < rows.len() && rows[self.scroll].cont {
            self.scroll += 1;
        }
        let mut end = rows.len().min(self.scroll + h);
        let mut more = None;
        if end < rows.len() && end > self.scroll {
            end -= 1;
            more = Some(rows.len() - end);
        }
        self.row_keys.clear();
        let mut out = Vec::with_capacity(h);
        for r in &rows[self.scroll..end] {
            self.row_keys.push(r.key.clone());
            out.push(self.row_line(r, w, name_col, two));
        }
        if let Some(n) = more {
            self.row_keys.push(String::new());
            out.push(line([sp(format!("  ↓ {n} more lines"), FAINT)]));
        }
        out
    }

    fn row_line(&self, r: &Row, w: usize, name_col: usize, two: bool) -> Line {
        let mut l = match r.part {
            Part::Drawn => r.line.clone(),
            _ => {
                let a = &self.agents[r.agent];
                let edge = if r.sel {
                    spb("▌", BRIGHT)
                } else {
                    sp("┃", r.edge)
                };
                let pad_w = 4;
                let mut l = Line::new();
                l.spans.push(edge);
                l.spans.push(plain(" "));
                match r.part {
                    Part::Agent => {
                        l.spans.push(sp(if r.last { "└─" } else { "├─" }, FAINT));
                        let row = self.agent_line(
                            r.agent,
                            w.saturating_sub(pad_w),
                            w,
                            r.sel,
                            name_col.saturating_sub(pad_w),
                            two,
                        );
                        join(&mut l, row);
                    }
                    _ => {
                        l.spans.push(sp(if r.last { " " } else { "│" }, FAINT));
                        l.spans.push(plain("    "));
                        let rw = w.saturating_sub(pad_w + 3);
                        if r.part == Part::Where {
                            join(&mut l, self.agent_where(a, r.agent, rw));
                        } else {
                            join(&mut l, self.agent_sub(r.agent, rw));
                        }
                    }
                }
                l
            }
        };
        if r.part != Part::Drawn
            && let Some(bg) = r.wash
        {
            return on_ground(l, w, bg);
        }
        if r.sel {
            return on_ground(l, w, SEL_BG);
        }
        if !r.key.is_empty() && r.key == self.hover {
            return on_ground(l, w, HOVER_BG);
        }
        l = fit(l, w);
        l
    }

    fn section_line(
        &self,
        title: &str,
        meta: &str,
        folded: bool,
        peek: &str,
        w: usize,
        sel: bool,
    ) -> Line {
        if folded {
            let mut head = line([
                plain(" "),
                sp("▸ ", FAINT),
                spb(title, SUB),
                plain("  "),
                sp(meta, DIM),
            ]);
            let room = w as isize - head.width() as isize + 1 - 6;
            if room >= 20 && !peek.is_empty() {
                head.spans.push(plain("   "));
                head.spans.push(sp(fit_str(peek, room as usize), FAINT));
            }
            return fit(head, w);
        }
        let edge = if sel {
            spb("▌", BRIGHT)
        } else {
            sp("┃", rail_color(title))
        };
        fit(
            line([
                edge,
                plain(" "),
                spb(title, BRIGHT),
                plain("  "),
                sp(meta, DIM),
            ]),
            w,
        )
    }

    /// The ground a row that wants you is drawn on.
    fn row_wash(&self, a: &Agent, sel: bool) -> Option<Color> {
        let (bg, on) = if a.halted() {
            (ERR_BG, ERR_SEL_BG)
        } else if a.your_turn(self.now) || a.just_finished(self.now) {
            return None;
        } else if a.needs_you() || a.job.state == State::Blocked {
            (ASK_BG, ASK_SEL_BG)
        } else {
            return None;
        };
        Some(if sel { on } else { bg })
    }

    /// Whether a row's summary earns its place.
    pub fn says_why(&self, a: &Agent) -> bool {
        asks(a) || a.stuck(self.now)
    }

    /// The latest thing an agent said or is doing, its colour, and whether
    /// it has only just finished.
    pub fn row_summary(&self, i: usize) -> (String, Color, bool) {
        let a = &self.agents[i];
        let now = self.now;
        let (mut summary, mut col, mut just) = (String::new(), DIM, false);
        if a.checking {
            summary = "turn ended · checking…".into();
        } else if a.halted() {
            summary = format!("stopped · {}", one_line(&a.halt_reason(now)));
            col = RED;
        } else if a.your_turn(now) && !a.just_finished(now) {
            summary = one_line(&a.job.detail);
            col = SUB;
            if summary.is_empty() {
                summary = "finished its turn".into();
            }
        } else if a.just_finished(now) {
            summary = one_line(&a.job.detail);
            just = true;
        } else if a.job.state == State::Blocked {
            summary = one_line(&a.job.needs);
            col = YELLOW;
            if summary.is_empty() {
                summary = one_line(&a.job.detail);
            }
        } else if a.job.live() {
            summary = one_line(&a.job.detail);
            if summary.is_empty()
                && let Some(p) = self.previews.get(&a.key)
            {
                summary = one_line(&p.doing);
            }
            if summary.is_empty() && a.interactive && !a.headless {
                summary = "working in a terminal".into();
            }
            if summary.is_empty() {
                summary = "working…".into();
            }
        } else if a.busy() {
            summary = background_text(a);
        } else {
            if self.focused() != Some(i) {
                summary = one_line(&a.job.detail);
            }
            col = FAINT;
        }
        if summary == "stopped" {
            summary.clear();
        }
        if a.stuck(now) {
            summary = format!("silent {} · ctrl+b asks why · {summary}", age(a.quiet(now)));
            col = YELLOW;
        }
        (unmark(&summary), col, just)
    }

    /// Subagents and background shells, the RUNNING column.
    fn activity(&self, a: &Agent, w: usize) -> Line {
        let mut l = Line::new();
        if a.subs.direct + a.subs.nested > 0 {
            let mut v = format!("↳{}", a.subs.direct);
            if a.subs.nested > 0 {
                v.push_str(&format!("+{}", a.subs.nested));
            }
            l.spans.push(sp(v, ORANGE));
        }
        let shells = if a.pid != 0 {
            a.job.running.iter().filter(|t| t.kind == "shell").count()
        } else {
            0
        };
        if shells > 0 {
            if !l.spans.is_empty() {
                l.spans.push(plain(" "));
            }
            l.spans.push(sp(format!("▸{shells}"), DIM));
        }
        let lw = l.width();
        if w > lw {
            l.spans.insert(0, plain(blanks(w - lw)));
        } else if w < lw {
            l = rush_draw::truncate_line(&l, w, "…");
        }
        l
    }

    /// The first line of a row: marker, name, badge, figures, and the
    /// summary too unless the row is stacked.
    pub fn agent_line(
        &self,
        i: usize,
        w: usize,
        list_w: usize,
        sel: bool,
        name_col: usize,
        stacked: bool,
    ) -> Line {
        let a = &self.agents[i];
        let [w_act, w_cpu, w_ram, w_tok, w_cost, w_time] = self.col_widths(list_w);
        let now = self.now;
        let live = a.job.live();
        let marker = if a.checking {
            sp("◔", SUB)
        } else if a.halted() {
            sp("✗", RED)
        } else if a.your_turn(now) {
            sp("◆", GREEN)
        } else if a.just_finished(now) {
            sp("✓", GREEN)
        } else if a.needs_you() {
            spb("?", YELLOW)
        } else if live {
            sp(
                SPINNER[(self.tick + a.job.id.len()) % SPINNER.len()],
                ORANGE,
            )
        } else if a.busy() {
            sp("◌", ORANGE)
        } else if a.pid != 0 {
            sp("◦", DIM)
        } else {
            sp("·", FAINT)
        };
        let busy = a.busy();
        let resident = !live && a.pid != 0;
        let act = if w_act == 0 {
            Line::new()
        } else {
            self.activity(a, w_act)
        };
        let cpu_cell = || {
            if a.pid == 0 {
                return plain(blanks(w_cpu));
            }
            let v = right(&format!("{:.0}%", a.cpu), w_cpu);
            if a.cpu >= 50.0 {
                sp(v, YELLOW)
            } else if a.cpu >= 5.0 {
                sp(v, SUB)
            } else {
                sp(v, FAINT)
            }
        };
        let ram_cell = |active: bool| {
            if a.pid == 0 {
                return plain(blanks(w_ram));
            }
            let v = right(&mem(a.mem), w_ram);
            if a.mem >= 4 << 30 {
                sp(v, YELLOW)
            } else if active {
                sp(v, DIM)
            } else {
                sp(v, FAINT)
            }
        };
        let tok_cell = |active: bool| {
            let c = a.spend.context;
            if c <= 0 {
                return plain(blanks(w_tok));
            }
            let v = right(&tokens(c), w_tok);
            let win = self.reg.context_window(a.kind.as_str(), &a.spend.model);
            if win > 0 && c as f64 * 100.0 / win as f64 >= 80.0 {
                sp(v, YELLOW)
            } else if active {
                sp(v, DIM)
            } else {
                sp(v, FAINT)
            }
        };
        let mut rt = act.clone();
        if live || busy {
            let v = right(&money_short(a.spend.cost), w_cost);
            rt.spans.extend([
                cpu_cell(),
                ram_cell(true),
                tok_cell(true),
                if a.spend.cost >= 100.0 {
                    sp(v, YELLOW)
                } else {
                    sp(v, DIM)
                },
            ]);
        } else if resident {
            rt.spans.extend([
                sp(right(&format!("{:.0}%", a.cpu), w_cpu), FAINT),
                ram_cell(false),
                tok_cell(false),
                sp(right(&money_short(a.spend.cost), w_cost), FAINT),
            ]);
        } else {
            let mut cost = money_short(a.spend.cost);
            if cost == "–" {
                cost.clear();
            }
            rt.spans.extend([
                plain(blanks(w_cpu + w_ram)),
                tok_cell(false),
                sp(right(&cost, w_cost), FAINT),
            ]);
        }
        let group = self.group_of.get(&a.key).map(String::as_str);
        if w_time == 0 {
            rt.spans.push(plain(" "));
        } else if a.stuck(now) {
            rt.spans.push(sp(right(&age(a.quiet(now)), w_time), YELLOW));
            rt.spans.push(plain(" "));
        } else if live && !a.needs_you() {
            rt.spans.push(sp(right(&dur(self.run_for(a)), w_time), DIM));
            rt.spans.push(plain(" "));
        } else if group == Some(NEEDS) || group == Some(FINISHED) {
            let waited = a.age(now);
            let c = if waited >= 30 * MINUTE { YELLOW } else { TEXT };
            rt.spans.push(sp(right(&age(waited), w_time - 2), c));
            rt.spans.push(plain(" "));
            let (g, gc) = cache_wheel(a.quiet(now));
            rt.spans.push(sp(g, gc));
            rt.spans.push(plain(" "));
        } else {
            rt.spans.push(sp(right(&age(a.age(now)), w_time), FAINT));
            rt.spans.push(plain(" "));
        }
        let rt = line(rt.spans);

        let name_color = if a.your_turn(now) && !sel {
            Some(GREEN)
        } else if sel || a.needs_you() || a.halted() {
            Some(TEXT)
        } else {
            None
        };
        let name_style = match name_color {
            Some(c) => bold(c),
            None if live || busy => st(TEXT),
            None => st(DIM),
        };
        let name = one_line(&a.display_name);
        let short = line([sp("✻", ORANGE)]);
        let mut badges = short.clone();
        if w_act == 0 {
            let act = self.activity(a, 0);
            let act = line(
                act.spans
                    .into_iter()
                    .filter(|s| !s.text.trim().is_empty())
                    .collect::<Vec<_>>(),
            );
            if !act.spans.is_empty() {
                badges.spans.push(plain(" "));
                join(&mut badges, act);
            }
        }
        let (summary, sum_color, just) = self.row_summary(i);
        let room = w as isize - 3 - rt.width() as isize;
        let mut name_room = room;
        let with_summary = !summary.is_empty() && self.says_why(a) && !stacked;
        let mut name_col = name_col as isize;
        let harness;
        let (full, extra) = if sel || a.key == self.hover {
            harness = format!("✻ {}", crate::session::harness_name(&self.reg, &a.kind));
            (harness.as_str(), width(&harness) as isize - 1)
        } else {
            ("", 0)
        };
        let grew = extra > 0 && with_summary && room - name_col - extra - 2 > 8;
        if grew {
            name_col += extra;
        }
        if with_summary && room - name_col - 2 > 8 {
            name_room = name_room.min(name_col);
        }
        name_room -= badges.width() as isize + 1;
        let name_w = width(&name) as isize;
        if !full.is_empty() && (grew || name_w > name_room || name_w <= name_room - extra) {
            if !grew {
                name_room -= extra;
            }
            badges.spans[0] = sp(full, ORANGE);
        }
        let mut left = line([rush_core::text::Span::new(
            rush_draw::truncate(&name, name_room.max(0) as usize, "…").into_owned(),
            name_style,
        )]);
        left.spans.push(plain(" "));
        join(&mut left, badges);
        if with_summary {
            left = fit(left, name_col.max(0) as usize);
            let sw = room - name_col - 2;
            if sw > 8 {
                let sw = sw as usize;
                left.spans.push(plain("  "));
                if just {
                    left.spans.push(sp("just finished", GREEN));
                    left.spans.push(sp(" · ", FAINT));
                    left.spans
                        .push(sp(fit_str(&summary, sw.saturating_sub(16)), sum_color));
                } else {
                    left.spans.push(sp(fit_str(&summary, sw), sum_color));
                }
            }
        }
        let mut out = line([plain(" "), marker, plain(" ")]);
        join(&mut out, fit(left, room.max(0) as usize));
        join(&mut out, rt);
        out
    }

    /// How long a has been at its current run.
    fn run_for(&self, a: &Agent) -> Ms {
        match self.run_since.get(&a.key) {
            Some(&t) if t != 0 => self.now - t,
            _ => a.elapsed(self.now),
        }
    }

    /// An agent's second line in the rail look: its project and branch.
    fn agent_where(&self, a: &Agent, i: usize, w: usize) -> Line {
        let mut parts: Vec<rush_core::text::Span> = Vec::new();
        let fk = folder_key(a, &self.roots[i]);
        if let Some(t) = self.titles.get(fk) {
            let proj = if self.roots[i].is_empty() && !a.repo.is_empty() {
                a.repo.as_str()
            } else {
                fk
            };
            parts.push(spb(t.clone(), project_tint(proj)));
        }
        if !a.branch.is_empty() {
            parts.push(sp(a.branch.clone(), DIM));
        }
        let mut l = Line::new();
        for (n, p) in parts.into_iter().enumerate() {
            if n > 0 {
                l.spans.push(sp(" · ", FAINT));
            }
            l.spans.push(p);
        }
        fit(l, w)
    }

    /// A stacked row's second line: its summary, or where it works.
    fn agent_sub(&self, i: usize, w: usize) -> Line {
        let room = w.saturating_sub(5);
        let (summary, col, just) = self.row_summary(i);
        let mut l = line([plain("  "), sp("╰ ", FAINT)]);
        if just {
            l.spans.push(sp("just finished", GREEN));
            l.spans.push(sp(" · ", FAINT));
            l.spans
                .push(sp(tidy_fit(&summary, room.saturating_sub(16)), col));
        } else if summary.is_empty() {
            let a = &self.agents[i];
            l.spans.push(sp(
                tidy_fit(&location_summary(a, &self.roots[i]), room),
                FAINT,
            ));
        } else {
            l.spans.push(sp(tidy_fit(&summary, room), col));
        }
        l
    }
}

impl Row {
    fn drawn(line: Line, key: String, sel: bool) -> Row {
        Row {
            line,
            key,
            part: Part::Drawn,
            agent: 0,
            edge: FAINT,
            last: false,
            wash: None,
            sel,
            cont: false,
        }
    }
}
