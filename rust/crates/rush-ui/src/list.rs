//! The agent list, in the rail look and the classic: sections, projects,
//! rows, columns. After Go's
//! internal/ui/model.go (rebuild :2215, sections :2150, stale :2612,
//! asks :2620, waitedLess/stoppedLess/runLess :2537-2620, as of a4c94f59,
//! noteRuns :2572, sectionMeta :2644, move :2069, items :2090),
//! view.go (listLines :1619, listRowLine :1749, rowWash :1785,
//! sectionLine :1803, colWidths :1937, columnHeader :1965, nameColumn
//! :2041, widestName :2053, stacked :2077, agentSub :2091, agentLine
//! :2199, saysWhy :2389, cacheWheel :2396, rowSummary :2415, activity
//! :2524, backgroundText :2548, costCell :2589, badges :2597; as of
//! main's latest the row's badges and pluginStatus :2334, badges :2621),
//! rail.go
//! (railColor :28, railSection :43, railLast :59, agentWhere :76, To
//! land and its tag as of 2e4662ff: model.go:2282, rail.go:39 and :96;
//! what a finished turn wants as of 779c3ea7 and 3c9343b1: model.go:2285,
//! rail.go:101, view.go:1787 rowWash, :2209 agentLine, :2439 rowSummary;
//! railRows :150), folders.go (folderKey :68, folderTitles :85, treeOf
//! :122), wall.go:646 (wallProject, projectTint), location.go:47
//! (locationSummary) and host/tmp.go:39 (IsTemp). As of main's latest:
//! rebuild model.go:2221 (Done and Pinned, Scratch, a section split by
//! project :2346), rail.go (agentWhere :80, startedBy :130 as of
//! df3a73ef, whereGit :145, prShort :155, railProject :178, nested :184,
//! statusShort :227), the classic look's rows (view.go:1578
//! drawAgentRow, :1624 listLines, :1754 listRowLine, :1810 sectionLine,
//! :2195 soloTree, :2336 the row's end tag as of 2e4662ff; folders.go:197
//! projectLine, :309 treeLine, :319 treeTag) and subrows.go's rows.
//! Rooms in the list: a room one row with its members under it, one
//! opened from a chat under that chat (model.go:2251 roomize's rows,
//! :2459 roomMembers' under them; view.go:1593 drawAgentRow's room and
//! member rows, :1678; rail.go:63 railLast, :192 railRows; roomrows.go:50
//! roomize, :125 roomMembers).

use std::collections::HashMap;

use rush_core::agent::{Agent, State};
use rush_core::text::{Color, Line, width};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use crate::model::Model;
use crate::screens::list_extras::landing::{land_of, tree_of};
use crate::screens::session_core::turnwants::{
    turn_label, wants_mark, wants_of, wants_say, wants_you,
};
use crate::screens::social::rooms::{ROOM_KEY_PREFIX, RoomRows, Under, grouped};
use crate::style::*;

pub const NEEDS: &str = "Needs you";
pub const FINISHED: &str = "Finished";
pub const STUCK: &str = "Stuck?";
pub const WORKING: &str = "Working";
pub const IDLE: &str = "Idle";
pub const JUST_LEFT: &str = "Just left";
/// What's stopped with its work done but not landed, and only waits on
/// you to merge it (model.go toLandSection, landing.go toLand).
pub const TO_LAND: &str = "To land";
pub const PINNED: &str = "Pinned";
pub const TODAY: &str = "Today";
pub const EARLIER: &str = "Earlier";
/// Temp-folder runs: they finish on their own, not on your turn; folded.
pub const SCRATCH_SECTION: &str = "Scratch";
pub const SCRATCH: &str = "scratch";
pub const NO_FOLDER: &str = "No folder";
/// rush's own advisor's rows sit under this (folders.go:63 rushSection).
pub const RUSH_SECTION: &str = "rush's advisor · Haiku looks for savings, Opus checks";
/// Temp work Earlier names only past this much (tempwork.go:16).
const TEMP_SHOWN: i64 = 1 << 20;

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
    /// A project's heading in a section split by project: its title, and
    /// its folder (model.go lineProject).
    Project {
        title: String,
        root: String,
    },
    /// A linked worktree's heading under its project, in the classic look
    /// (model.go lineTree): the project's folder, and the worktree's.
    Tree {
        root: String,
        tree: String,
    },
    /// An agent, by its place in Model::agents.
    Agent(usize),
    /// A room's own row, by its id; joined when it's under the chat it
    /// was opened from (roomrows.go:50 roomize).
    Room {
        id: String,
        joined: bool,
    },
    /// A room's member under it, by its place in Model::agents; joined
    /// when its room is under a chat (model.go:2459).
    Member {
        i: usize,
        joined: bool,
    },
}

impl ListLine {
    /// Whether it's a row ↑↓ stop on that isn't a heading: an agent's, a
    /// room's or a member's (Go's lineAgent).
    pub fn is_row(&self) -> bool {
        matches!(
            self,
            ListLine::Agent(_) | ListLine::Room { .. } | ListLine::Member { .. }
        )
    }

    /// Steps in a row sits from its section's edge in the rail look: a
    /// room and its members under the chat it was opened from sit one in
    /// (model.go:2462 under).
    fn inset(&self) -> usize {
        match self {
            ListLine::Room { joined: true, .. } | ListLine::Member { joined: true, .. } => {
                ROW_INSET
            }
            _ => 0,
        }
    }
}

/// What the list lays out: an agent, or a room's row.
#[derive(Clone, Debug)]
enum Item {
    Agent(usize),
    Room(String),
}

/// A row as drawn: which agent or section it acts on, and its part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Drawn,
    Agent,
    Where,
    Sub,
    /// The i'th of its running subagents (subrows.go).
    Subagent(usize),
    /// A room's own row, and a member's under it (roomrows.go roomLine,
    /// roomMemberLine).
    Room,
    Member,
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
    /// The classic look's cells in from the edge, and a worktree's tag
    /// beside the name for its one agent.
    pad: usize,
    tag: Option<Line>,
    /// Steps in from the section's edge in the rail look (a room joined
    /// under its chat, and its members).
    inset: usize,
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
        TO_LAND => QUEUE,
        _ => FAINT,
    }
}

/// An agent waiting on an answer from you, not just a next task
/// (model.go:2629).
pub fn asks(a: &Agent) -> bool {
    a.needs_you() || a.waiting() || a.halted()
}

/// Left a day without a turn, neither open, at work nor pinned: Earlier
/// has it unless it asks you something (model.go:2621).
pub(crate) fn stale(a: &Agent, now: Ms) -> bool {
    !a.job.open() && !a.busy() && !a.pinned && a.age(now) >= 24 * HOUR
}

/// Whether a section lists what finished purely by when, the most recent
/// first, its project a tag rather than a heading (model.go:2179 byTime).
pub fn by_time(title: &str) -> bool {
    matches!(title, JUST_LEFT | TODAY | EARLIER)
}

/// Compares strings as their lower-case forms would (model.go:2539).
fn cmp_lower(x: &str, y: &str) -> std::cmp::Ordering {
    let mut a = x.chars().flat_map(char::to_lowercase);
    let mut b = y.chars().flat_map(char::to_lowercase);
    loop {
        match (a.next(), b.next()) {
            (Some(p), Some(q)) if p == q => continue,
            (Some(p), Some(q)) => return p.cmp(&q),
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
        }
    }
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
    if a.advisor {
        RUSH_SECTION
    } else if !root.is_empty() {
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
            if *k == SCRATCH || *k == NO_FOLDER || *k == RUSH_SECTION {
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
    /// Groups the agents into sections, what needs you first; what's put
    /// away, or finished more than a day ago, goes to Earlier
    /// (model.go:2221 rebuild). A room is one row in place of its members,
    /// which go under it, and a room opened from a chat goes under that
    /// chat (model.go:2251, :2459; roomrows.go:50 roomize).
    pub fn rebuild(&mut self) {
        let now = self.now;
        let rooms = match self.kept.peek::<RoomRows>() {
            Some(rr) => grouped(&rr.list, &self.agents, &rr.hidden),
            None => grouped(&[], &self.agents, &Default::default()),
        };
        let agents = &self.agents;
        let roots = &self.roots;
        let of = |it: &Item| -> &Agent {
            match it {
                Item::Agent(i) => &agents[*i],
                Item::Room(id) => &rooms.row[id],
            }
        };
        let root = |it: &Item| -> &str {
            match it {
                Item::Agent(i) => &roots[*i],
                Item::Room(_) => "",
            }
        };
        struct Group {
            name: String,
            items: Vec<Item>,
            rank: u8,
            recent: Ms,
        }
        let mut groups: Vec<Group> = Vec::new();
        let add = |groups: &mut Vec<Group>, name: &str, rank: u8, it: Item, at: Ms| {
            let g = match groups.iter().position(|g| g.name == name) {
                Some(j) => &mut groups[j],
                None => {
                    groups.push(Group {
                        name: name.to_string(),
                        items: Vec::new(),
                        rank,
                        recent: 0,
                    });
                    groups.last_mut().unwrap()
                }
            };
            g.items.push(it);
            g.recent = g.recent.max(at);
        };
        let mut left: Vec<Item> = Vec::new();
        let active_for = self.cfg.active_for();
        let just_left_for = self.cfg.just_left_for();
        let by = self.cfg.group_by.clone();
        let filters = self.ext.list_filter.clone();
        let laid: Vec<Item> = rooms
            .rows
            .iter()
            .map(|&i| Item::Agent(i))
            .chain(rooms.rooms.iter().map(|id| Item::Room(id.clone())))
            .collect();
        for it in laid {
            let a = of(&it);
            if !filters.iter().all(|f| f(self, a)) {
                continue;
            }
            let fresh = !stale(a, now);
            let at = a.job.updated_at;
            let fk = folder_key(a, root(&it));
            if a.done {
                // Put away stays put away: what it left running doesn't
                // bring it back, a message does.
                add(&mut groups, EARLIER, 9, it, at);
            } else if asks(a) {
                add(&mut groups, NEEDS, 0, it, at);
            } else if fresh && wants_you(self, a) {
                // Its last message hands you a step, offers one, asks or
                // says it couldn't.
                add(&mut groups, NEEDS, 0, it, at);
            } else if fk == SCRATCH {
                // Temp-folder runs finish on their own, not on your turn.
                add(&mut groups, SCRATCH_SECTION, 10, it, at);
            } else if fresh && (a.your_turn(now) || a.just_finished(now) && !a.seen && !a.checking)
            {
                add(&mut groups, FINISHED, 3, it, at);
            } else if a.stuck(now) {
                add(&mut groups, STUCK, 2, it, at);
            } else if a.checking || a.job.live() || a.busy() {
                add(&mut groups, WORKING, 1, it, at);
            } else if fresh && land_of(self, a).to_land() {
                // Done, but its work isn't on its target yet: yours to merge.
                add(&mut groups, TO_LAND, 4, it, at);
            } else if a.pinned {
                add(&mut groups, PINNED, 6, it, at);
            } else if a.pid != 0 || a.age(now) < active_for {
                // Open, or stopped a moment ago: in play a while before
                // Today has it.
                add(&mut groups, IDLE, 5, it, at);
            } else if fresh && a.age(now) < just_left_for {
                left.push(it);
            } else if !fresh {
                // A day without a turn, a finish you never looked at too.
                add(&mut groups, EARLIER, 9, it, at);
            } else if by == "agent" {
                let name = self.reg.label(a.kind.as_str());
                add(&mut groups, &name, 6, it, at);
            } else if by == "group" && !a.group.is_empty() {
                let g = a.group.clone();
                add(&mut groups, &g, 6, it, at);
            } else {
                add(&mut groups, TODAY, 7, it, at);
            }
        }
        // The most recent stay in Just left, up to its count; the rest go
        // on to Today.
        left.sort_by_key(|it| of(it).age(now));
        let count = self.cfg.just_left_count();
        for (n, it) in left.into_iter().enumerate() {
            let at = of(&it).job.updated_at;
            if n < count {
                add(&mut groups, JUST_LEFT, 6, it, at);
            } else {
                add(&mut groups, TODAY, 7, it, at);
            }
        }
        // Split by project, each section's rows sit together by the
        // repository they work in, its worktrees' after its own.
        let split = self.cfg.split_projects();
        let titles = folder_titles(
            groups
                .iter()
                .flat_map(|g| g.items.iter())
                .map(|it| folder_key(of(it), root(it))),
        );
        // When each agent at work began this run (noteRuns).
        let mut next = HashMap::new();
        for g in groups
            .iter()
            .filter(|g| g.name == WORKING || g.name == STUCK)
        {
            for it in &g.items {
                let k = &of(it).key;
                let t = match self.run_since.get(k) {
                    Some(&t) => t,
                    None if !self.run_init => 0,
                    None => now,
                };
                next.insert(k.clone(), t);
            }
        }
        let run_init = self.run_init || !self.agents.is_empty();

        groups.sort_by(|a, b| a.rank.cmp(&b.rank).then(b.recent.cmp(&a.recent)));
        let run_since = &next;
        let run_for = |a: &Agent| match run_since.get(&a.key) {
            Some(&t) if t != 0 => now - t,
            _ => a.elapsed(now),
        };
        for g in &mut groups {
            let within = |x: &Item, y: &Item| {
                let (a, b) = (of(x), of(y));
                match g.name.as_str() {
                    // What you'd look at first: a question, then the
                    // longest waiting or silent (waitedLess).
                    NEEDS | STUCK => asks(b)
                        .cmp(&asks(a))
                        .then(b.quiet(now).cmp(&a.quiet(now)))
                        .then(a.key.cmp(&b.key)),
                    // The longest at it this run first (runLess).
                    WORKING => run_for(b).cmp(&run_for(a)).then(a.key.cmp(&b.key)),
                    // Earlier and every stopped section: by the time its
                    // rows show, the latest first, so the column reads in
                    // order (stoppedLess).
                    _ => asks(b)
                        .cmp(&asks(a))
                        .then(a.age(now).cmp(&b.age(now)))
                        .then(a.key.cmp(&b.key)),
                }
            };
            if split && !by_time(&g.name) {
                // What finished goes by time, its project a tag.
                let title = |it: &Item| {
                    titles
                        .get(folder_key(of(it), root(it)))
                        .map_or("", String::as_str)
                };
                g.items.sort_by(|x, y| {
                    cmp_lower(title(x), title(y))
                        .then_with(|| tree_of(of(x)).cmp(tree_of(of(y))))
                        .then_with(|| within(x, y))
                });
            } else {
                g.items.sort_by(within);
            }
        }
        let mut lines = Vec::new();
        let mut order = Vec::new();
        let mut group_of = HashMap::new();
        // The rail look's rows name their branch under them, so no
        // worktree headings.
        let rail = self.cfg.rail();
        for g in &groups {
            let mut cost = 0.0;
            let mut names = Vec::new();
            for it in &g.items {
                let a = of(it);
                cost += a.spend.cost;
                if names.len() < 4 {
                    names.push(one_line(&a.display_name));
                }
            }
            let fold = self.folded(&g.name);
            let mut meta = section_meta(g.items.len(), cost);
            // One extra figure at most, and only one you can act on: temp
            // work where /clean all reaches it, memory where agents rest.
            if g.name == EARLIER {
                let temp: i64 = g
                    .items
                    .iter()
                    .map(of)
                    .filter(|a| a.pid == 0)
                    .map(|a| a.temp)
                    .sum();
                if temp >= TEMP_SHOWN {
                    meta.push_str(&format!(
                        " · {} tmp",
                        crate::screens::social::parts::disk(temp)
                    ));
                }
            } else if g.name == IDLE {
                let held: u64 = g
                    .items
                    .iter()
                    .map(of)
                    .filter(|a| !a.job.live() && !a.busy())
                    .map(|a| a.mem)
                    .sum();
                if held > 0 {
                    meta.push_str(&format!(" · {} idle ram", mem(held)));
                }
            }
            lines.push(ListLine::Section {
                title: g.name.clone(),
                meta,
                folded: fold,
                peek: names.join(", "),
            });
            let split = split && !by_time(&g.name);
            let mut project: Option<&str> = None;
            let mut tree: &str = "";
            for it in &g.items {
                let a = of(it);
                if let Item::Agent(i) = it {
                    order.push(*i);
                }
                group_of.insert(a.key.clone(), g.name.clone());
                if fold {
                    continue;
                }
                if split {
                    let p = folder_key(a, root(it));
                    if project != Some(p) {
                        project = Some(p);
                        tree = "";
                        lines.push(ListLine::Project {
                            title: titles.get(p).cloned().unwrap_or_default(),
                            root: p.to_string(),
                        });
                    }
                    let t = tree_of(a);
                    if t != tree && !rail {
                        tree = t;
                        lines.push(ListLine::Tree {
                            root: p.to_string(),
                            tree: t.to_string(),
                        });
                    }
                }
                let joined = match it {
                    Item::Agent(i) => {
                        lines.push(ListLine::Agent(*i));
                        true // a room opened from a chat, joined under it
                    }
                    Item::Room(id) => {
                        lines.push(ListLine::Room {
                            id: id.clone(),
                            joined: false,
                        });
                        false
                    }
                };
                for u in rooms.under(&a.key) {
                    match u {
                        Under::Room(id) => {
                            group_of.insert(rooms.row[&id].key.clone(), g.name.clone());
                            lines.push(ListLine::Room { id, joined });
                        }
                        Under::Member(i) => {
                            order.push(i);
                            group_of.insert(self.agents[i].key.clone(), g.name.clone());
                            lines.push(ListLine::Member { i, joined });
                        }
                    }
                }
            }
            if !fold {
                // Folded history stacks tight.
                lines.push(ListLine::Blank);
            }
        }
        self.titles = titles;
        self.run_since = next;
        self.run_init = run_init;
        self.lines = lines;
        self.memo.forget_widest_name();
        self.order = order;
        self.group_of = group_of;
        self.kept::<RoomRows>().grouped = rooms;
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
                ListLine::Agent(i) | ListLine::Member { i, .. } => {
                    Some(self.agents[*i].key.clone())
                }
                ListLine::Room { id, .. } => Some(format!("{ROOM_KEY_PREFIX}{id}")),
                ListLine::Blank | ListLine::Project { .. } | ListLine::Tree { .. } => None,
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
        // The agent left keeps the preview while a heading is picked
        // (model.go:2088 move).
        if !self.sel.is_empty() && !self.sel.starts_with('§') {
            self.shown = self.sel.clone();
        }
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

    /// The widest of the list's names with its badges (view.go:2060
    /// widestName).
    fn widest_name(&self) -> usize {
        // Asked by the header and the rows: measured once a frame.
        self.memo.widest_name(|| self.measure_widest_name())
    }

    fn measure_widest_name(&self) -> usize {
        // Each name made one line in the same string, not a new one.
        let mut name = String::new();
        self.lines
            .iter()
            .filter_map(|l| match l {
                ListLine::Agent(i) | ListLine::Member { i, .. } => Some(&self.agents[*i]),
                ListLine::Room { id, .. } => self.rooms_grouped().and_then(|g| g.row.get(id)),
                _ => None,
            })
            .map(|a| {
                let n = if plain_one_line(&a.display_name) {
                    a.display_name.len()
                } else {
                    one_line_into(&mut name, &a.display_name);
                    width(&name)
                };
                match self.badges(a, false).width() {
                    0 => n,
                    b => n + b + 1,
                }
            })
            .max()
            .unwrap_or(0)
    }

    /// What goes by an agent's name: its harness's glyph (named in full
    /// when full), the harness its open session's host says it runs in
    /// over the one the list knows, and the pull requests it linked, two
    /// at most, unless its project's heading names them (view.go:2621
    /// badges, caps.go:29 sessionAgent, providers.go:134 rowBadge, :2649
    /// prColor).
    pub fn badges(&self, a: &Agent, full: bool) -> Line {
        let kind = self
            .session
            .as_ref()
            .filter(|v| v.key == a.key && !v.sess.info.kind.is_empty())
            .map_or(a.kind.as_str(), |v| v.sess.info.kind.as_str());
        let mut l = line([crate::screens::accounts::look::row_badge(
            &self.reg, kind, full,
        )]);
        let headed = !a.prs.is_empty()
            && self.cfg.split_projects()
            && !self.group_of.get(&a.key).is_some_and(|g| by_time(g));
        if headed {
            return l;
        }
        for (i, pr) in a.prs.iter().enumerate() {
            l.spans.push(plain(" "));
            if i == 2 {
                l.spans.push(sp(format!("+{}", a.prs.len() - 2), DIM));
                break;
            }
            let c = if pr.state == "MERGED" {
                BLUE
            } else if pr.state == "CLOSED" {
                DIM
            } else if pr.checks.failed > 0 {
                RED
            } else if pr.state == "DRAFT" {
                SUB
            } else {
                GREEN
            };
            l.spans.push(sp(format!("#{}", pr.number), c));
        }
        l
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
        let rail = self.cfg.rail();
        // The classic look: whether this section is split by project, the
        // worktree heading the rows are under, and a worktree's tag beside
        // its one agent's name instead of heading it.
        let (mut split_here, mut in_tree) = (false, false);
        let mut tag: Option<Line> = None;
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
                    split_here = false;
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
                ListLine::Project { title, root } => {
                    split_here = true;
                    in_tree = false;
                    let l = if rail {
                        // A project's heading on the section's edge, in the
                        // colour its rows name it in (rail.go:178
                        // railProject).
                        line([
                            sp("┃", edge),
                            plain(" "),
                            spb(title.clone(), project_tint(root)),
                        ])
                    } else {
                        project_line(title, w)
                    };
                    rows.push(Row::drawn(fit(l, w), String::new(), false));
                }
                ListLine::Tree { tree, .. } => {
                    in_tree = !tree.is_empty();
                    if solo_tree(&lines, li) {
                        // One agent: it stays under its project, the
                        // worktree a tag (view.go:2195 soloTree).
                        tag = Some(tree_tag(tree));
                        continue;
                    }
                    rows.push(Row::drawn(tree_line(tree, w), String::new(), false));
                }
                ListLine::Agent(i) => {
                    let a = &self.agents[*i];
                    let sel = a.key == self.sel;
                    let last = rail_last(&lines, li);
                    let mut wash = self.row_wash(a, sel);
                    let mut pad = 0;
                    let row_tag = tag.take();
                    if rail {
                        band = !band;
                        if wash.is_none() && band && !sel {
                            wash = Some(BAND_BG);
                        }
                    } else if split_here {
                        // Under its project, a worktree's under that, one
                        // tagged with its worktree under the project as
                        // its heading would have been.
                        let mut inset = if in_tree { 2 * ROW_INSET } else { ROW_INSET };
                        if row_tag.is_some() {
                            inset -= ROW_INSET;
                        }
                        pad = inset.saturating_sub(1); // a column in from the ▍
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
                            pad,
                            tag: if part == Part::Agent {
                                row_tag.clone()
                            } else {
                                None
                            },
                            inset: 0,
                        });
                    };
                    push(Part::Agent, false);
                    if rail && w > 7 {
                        push(Part::Where, true);
                    }
                    if two && self.says_why(a) {
                        push(Part::Sub, true);
                    }
                    // Its running subagents, a row each under it.
                    let n =
                        crate::screens::list_extras::subs::rows(self, &a.key, a.subagents.len());
                    for s in 0..n {
                        push(Part::Subagent(s), true);
                    }
                }
                ListLine::Room { .. } | ListLine::Member { .. } => {
                    // A room's row and its members' under it, as an
                    // agent's row is laid out (view.go:1678, rail.go:192
                    // railRows), with no reason or subagents under a room.
                    let (key, agent, part, joined) = match l {
                        ListLine::Room { id, joined } => (
                            format!("{ROOM_KEY_PREFIX}{id}"),
                            usize::MAX,
                            Part::Room,
                            *joined,
                        ),
                        ListLine::Member { i, joined } => {
                            (self.agents[*i].key.clone(), *i, Part::Member, *joined)
                        }
                        _ => continue,
                    };
                    let a = self.row_agent(&key, agent);
                    let sel = key == self.sel;
                    let inset = l.inset();
                    let last = rail_last(&lines, li);
                    let mut wash = None;
                    let mut pad = 0;
                    if rail {
                        if inset == 0 {
                            band = !band;
                        }
                        wash = a.and_then(|a| self.row_wash(a, sel));
                        if wash.is_none() && band && !sel {
                            wash = Some(BAND_BG);
                        }
                    } else {
                        let mut ins = match (split_here, in_tree) {
                            (false, _) => 0,
                            (true, false) => ROW_INSET,
                            (true, true) => 2 * ROW_INSET,
                        };
                        if joined {
                            ins += ROW_INSET;
                        }
                        if tag.take().is_some() {
                            ins = ins.saturating_sub(ROW_INSET);
                        }
                        pad = ins.saturating_sub(1); // a column in from the ▍
                    }
                    let says = part == Part::Member && two && a.is_some_and(|a| self.says_why(a));
                    let subs = if part == Part::Member && rail {
                        crate::screens::list_extras::subs::rows(
                            self,
                            &key,
                            self.agents[agent].subagents.len(),
                        )
                    } else {
                        0
                    };
                    let mut push = |part: Part, cont: bool| {
                        if sel && (wash.is_none() || !key.is_empty()) {
                            sel_top.get_or_insert(rows.len());
                            sel_bottom = rows.len();
                        }
                        rows.push(Row {
                            line: Line::new(),
                            key: key.clone(),
                            part,
                            agent,
                            edge,
                            last,
                            wash,
                            sel,
                            cont,
                            pad,
                            tag: None,
                            inset,
                        });
                    };
                    push(part, false);
                    if rail {
                        if w > 7 + inset {
                            push(Part::Where, true);
                        }
                        if says {
                            push(Part::Sub, true);
                        }
                        for s in 0..subs {
                            push(Part::Subagent(s), true);
                        }
                    }
                }
            }
        }
        self.lines = lines;
        self.memo.forget_widest_name();
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
            end -= 1; // its last line says what's below instead
            more = Some(rows.len() - end - 1);
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
        if !self.cfg.rail() {
            return self.classic_row(r, w, name_col, two);
        }
        let mut l = match r.part {
            Part::Drawn => r.line.clone(),
            _ => {
                let Some(a) = self.row_agent(&r.key, r.agent) else {
                    return fit(Line::new(), w);
                };
                let edge = if r.sel {
                    spb("▌", BRIGHT)
                } else {
                    sp("┃", r.edge)
                };
                let pad_w = 4 + r.inset;
                let mut l = Line::new();
                l.spans.push(edge);
                l.spans.push(plain(" "));
                match r.part {
                    Part::Room | Part::Member => {
                        if r.inset > 0 {
                            // Under the row that opened it.
                            l.spans.push(sp("│ ", FAINT));
                            l.spans.push(plain(blanks(r.inset)));
                        } else {
                            l.spans.push(sp(if r.last { "└─" } else { "├─" }, FAINT));
                        }
                        join(&mut l, self.room_row(r, a, w.saturating_sub(pad_w)));
                    }
                    Part::Agent => {
                        l.spans.push(sp(if r.last { "└─" } else { "├─" }, FAINT));
                        let row = self.agent_line(
                            r.agent,
                            w.saturating_sub(pad_w),
                            w,
                            r.sel,
                            name_col.saturating_sub(pad_w),
                            two,
                            None,
                        );
                        join(&mut l, row);
                    }
                    _ => {
                        l.spans.push(sp(if r.last { " " } else { "│" }, FAINT));
                        l.spans.push(plain("    "));
                        let rw = w.saturating_sub(pad_w + 3);
                        match r.part {
                            Part::Where => {
                                join(&mut l, self.agent_where_in(a, self.row_root(r), rw))
                            }
                            Part::Subagent(s) => join(
                                &mut l,
                                crate::screens::list_extras::subs::row_of(self, a, s, w - 7, w),
                            ),
                            _ => join(&mut l, self.agent_sub(r.agent, rw)),
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

    /// A row in the classic look: in from the edge as its project and
    /// worktree put it, the picked one marked ▍ on the selection's ground
    /// (view.go:1578 drawAgentRow, :1754 listRowLine).
    fn classic_row(&self, r: &Row, w: usize, name_col: usize, two: bool) -> Line {
        let lead = plain(blanks(r.pad));
        let rw = w.saturating_sub(r.pad);
        let mut l = match r.part {
            Part::Drawn => r.line.clone(),
            Part::Agent => {
                let mut l = line([lead]);
                let row = self.agent_line(
                    r.agent,
                    rw,
                    w,
                    r.sel,
                    name_col.saturating_sub(r.pad),
                    two,
                    r.tag.as_ref(),
                );
                join(&mut l, row);
                l
            }
            Part::Subagent(s) => {
                // Past the marker, under the name.
                let mut l = line([lead, plain("   ")]);
                let a = &self.agents[r.agent];
                join(
                    &mut l,
                    crate::screens::list_extras::subs::row_of(self, a, s, rw.saturating_sub(3), w),
                );
                l
            }
            Part::Sub | Part::Where => {
                let mut l = line([lead]);
                join(&mut l, self.agent_sub(r.agent, rw));
                l
            }
            Part::Room | Part::Member => {
                let mut l = line([lead]);
                if let Some(a) = self.row_agent(&r.key, r.agent) {
                    join(&mut l, self.room_row(r, a, rw));
                }
                l
            }
        };
        let mark = |l: &mut Line| {
            // The picked row's marker takes the row's first cell, a blank.
            if let Some(s) = l.spans.iter_mut().find(|s| !s.text.is_empty()) {
                s.text.remove(0);
            }
            l.spans.insert(0, sp("▍", ORANGE));
        };
        if r.part != Part::Drawn
            && let Some(bg) = r.wash
        {
            if r.sel {
                mark(&mut l);
            }
            return on_ground(l, w, bg);
        }
        if r.sel {
            mark(&mut l);
            return on_ground(l, w, SEL_BG);
        }
        if !r.key.is_empty() && r.key == self.hover {
            return on_ground(l, w, HOVER_BG);
        }
        fit(l, w)
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
        if !self.cfg.rail() {
            // The classic look: the title with a hairline to the edge
            // (view.go:1810 sectionLine, style.go:401 rule).
            let mut head = line([spb(title, SUB)]);
            if !meta.is_empty() {
                head.spans.push(plain("  "));
                head.spans.push(sp(meta, DIM));
            }
            let n = w.saturating_sub(5).saturating_sub(head.width() + 2);
            head.spans.push(plain(" "));
            head.spans.push(sp("─".repeat(n), FAINT));
            let mut l = line([plain(" "), sp("▾ ", FAINT)]);
            join(&mut l, head);
            return fit(l, w);
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
        } else if wants_you(self, a) {
            (ASK_BG, ASK_SEL_BG) // its last message wants you: drawn like a question
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
        } else if wants_you(self, a) {
            summary = wants_say(&wants_of(self, a));
            col = YELLOW;
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
        // Right-aligned in its column; with no column (w 0), as it is, for
        // the badges by the name (view.go:2548 activity).
        let lw = l.width();
        if w > lw {
            l.spans.insert(0, plain(blanks(w - lw)));
        } else if w > 0 && w < lw {
            l = rush_draw::truncate_line(&l, w, "…");
        }
        l
    }

    /// The first line of a row: marker, name, badge, figures, and the
    /// summary too unless the row is stacked.
    #[allow(clippy::too_many_arguments)] // as Go's agentLine takes them
    pub fn agent_line(
        &self,
        i: usize,
        w: usize,
        list_w: usize,
        sel: bool,
        name_col: usize,
        stacked: bool,
        tag: Option<&Line>,
    ) -> Line {
        let a = &self.agents[i];
        let [w_act, w_cpu, w_ram, w_tok, w_cost, w_time] = self.col_widths(list_w);
        let now = self.now;
        let live = a.job.live();
        let marker = if a.checking {
            sp("◔", SUB)
        } else if a.halted() {
            sp("✗", RED)
        } else if wants_you(self, a) {
            wants_mark(&wants_of(self, a))
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
        let short = self.badges(a, false);
        let mut badges = short.clone();
        // What plugins put on its row, after its badges (view.go:2336).
        let ps = crate::screens::plugin_ui::hooks::plugin_status(self, &a.key);
        if !ps.spans.is_empty() {
            if badges.width() > 0 {
                badges.spans.push(plain(" "));
            }
            join(&mut badges, ps);
        }
        if w_act == 0 {
            // At width 0 it has no padding; the space between its
            // subagents and shells stays (view.go:2341).
            let act = self.activity(a, 0);
            if !act.spans.is_empty() {
                badges.spans.push(plain(" "));
                join(&mut badges, act);
            }
        }
        let (summary, sum_color, just) = self.row_summary(i);
        let mut room = w as isize - 3 - rt.width() as isize;
        // What's left before Earlier has it, at the row's end: the rail
        // look says it under the name (view.go:2336, as of 2e4662ff).
        let mut land = None;
        if !self.cfg.rail()
            && let Some(t) = land_of(self, a).tag()
            && room - width(&t.text) as isize - 2 >= 16
        {
            room -= width(&t.text) as isize + 2;
            land = Some(t);
        }
        let mut name_room = room;
        let with_summary = !summary.is_empty() && self.says_why(a) && !stacked;
        let mut name_col = name_col as isize;
        let (full, extra) = if sel || a.key == self.hover {
            let f = self.badges(a, true);
            let extra = f.width() as isize - short.width() as isize;
            (Some(f), extra)
        } else {
            (None, 0)
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
        if let Some(full) = full
            && (grew || name_w > name_room || name_w <= name_room - extra)
        {
            if !grew {
                name_room -= extra;
            }
            // The short badges replaced by the full, what follows kept.
            let rest = badges.spans.split_off(short.spans.len());
            badges = full;
            badges.spans.extend(rest);
        }
        let mut left = line([rush_core::text::Span::new(
            rush_draw::truncate(&name, name_room.max(0) as usize, "…").into_owned(),
            name_style,
        )]);
        left.spans.push(plain(" "));
        join(&mut left, badges);
        if let Some(t) = tag {
            // Its worktree, which has no heading of its own.
            left.spans.push(plain("  "));
            join(&mut left, t.clone());
        }
        if !self.cfg.rail()
            && group.is_some_and(by_time)
            && let Some(p) = self.titles.get(folder_key(a, &self.roots[i]))
            && !p.is_empty()
        {
            // What finished goes by time, without project headings.
            left.spans.push(plain("  "));
            left.spans.push(sp(p.clone(), FAINT));
        }
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
        if let Some(t) = land {
            out.spans.push(plain("  "));
            out.spans.push(t);
        }
        join(&mut out, rt);
        out
    }

    /// How long a has been at its current run.
    /// The agent a row draws: a room's row as the list placed it, else
    /// the agent at i.
    fn row_agent(&self, key: &str, i: usize) -> Option<&Agent> {
        match key.strip_prefix(ROOM_KEY_PREFIX) {
            Some(id) => self.rooms_grouped()?.row.get(id),
            None => self.agents.get(i),
        }
    }

    /// The main checkout of a row's repository; "" for a room's.
    fn row_root(&self, r: &Row) -> &str {
        if r.key.starts_with(ROOM_KEY_PREFIX) {
            ""
        } else {
            self.roots.get(r.agent).map_or("", String::as_str)
        }
    }

    /// A room's row, or a member's under it (roomrows.go:190 roomLine,
    /// :210 roomMemberLine).
    fn room_row(&self, r: &Row, a: &Agent, w: usize) -> Line {
        use crate::screens::social::rooms::{room_line, room_member_line};
        let tick = self.tick;
        if r.part == Part::Room {
            return self
                .room_of(&r.key)
                .map(|s| room_line(s, w, r.sel, tick))
                .unwrap_or_default();
        }
        room_member_line(self.room_of(&r.key), a, w, r.sel, tick)
    }

    fn run_for(&self, a: &Agent) -> Ms {
        match self.run_since.get(&a.key) {
            Some(&t) if t != 0 => self.now - t,
            _ => a.elapsed(self.now),
        }
    }

    /// An agent's second line in the rail look: its project, branch, how
    /// far it is from pushed, its uncommitted files, its PR, what's left
    /// before Earlier has it, what its finished turn wants of you and
    /// what its own view says of where it stands (rail.go:80 agentWhere).
    #[cfg(test)]
    pub(crate) fn agent_where(&self, a: &Agent, i: usize, w: usize) -> Line {
        self.agent_where_in(a, &self.roots[i], w)
    }

    /// agent_where with the main checkout of a's repository (root), ""
    /// for a room's row.
    fn agent_where_in(&self, a: &Agent, root: &str, w: usize) -> Line {
        if let Some(mut by) = started_by(a) {
            // A job rush or a plugin runs, in your home folder: who runs
            // it, and why, say more than where.
            if let Some(v) = crate::screens::plugin_ui::views::read::summary_of(self, a) {
                by.spans.push(sp(" · ", FAINT));
                join(
                    &mut by,
                    crate::screens::plugin_ui::views::view_summary_short(&v),
                );
            }
            return fit(by, w);
        }
        let mut parts: Vec<Line> = Vec::new();
        let fk = folder_key(a, root);
        if let Some(t) = self.titles.get(fk)
            && !t.is_empty()
            && !self.nested(a)
        {
            // The colour the Wall gives it, so it's found at a glance.
            let proj = if root.is_empty() && !a.repo.is_empty() {
                a.repo.as_str()
            } else {
                fk
            };
            parts.push(line([spb(t.clone(), project_tint(proj))]));
        }
        let st = self.where_git(a, fk);
        let branch = st
            .map(|s| s.branch.as_str())
            .filter(|b| !b.is_empty())
            .unwrap_or(&a.branch);
        if !branch.is_empty() {
            parts.push(line([sp(branch.to_string(), DIM)]));
        }
        if let Some(st) = st {
            if st.ahead > 0 {
                parts.push(line([sp(format!("↑{}", st.ahead), ORANGE)]));
            }
            if st.behind > 0 {
                parts.push(line([sp(format!("↓{}", st.behind), YELLOW)]));
            }
            if st.changed > 0 {
                parts.push(line([sp(format!("±{}", st.changed), DIM)]));
            }
        }
        let land = land_of(self, a);
        if land.kind != crate::screens::list_extras::landing::LandKind::None && land.pr > 0 {
            // What's left of its pull request says its state.
        } else if let Some(pr) = &a.pr {
            parts.push(line([status_short(pr)])); // found from the branch, for every harness
        } else if let Some(pr) = a.prs.last() {
            parts.push(line([pr_short(pr)]));
        }
        if let Some(t) = land.tag() {
            parts.push(line([t])); // what's left before Earlier has it
        }
        let t = turn_label(self, a);
        if !t.is_empty() {
            // What its finished turn wants of you, or that it's done.
            parts.push(line(t));
        }
        let mut l = Line::new();
        for (n, p) in parts.into_iter().enumerate() {
            if n > 0 {
                l.spans.push(sp(" · ", FAINT));
            }
            join(&mut l, p);
        }
        if let Some(v) = crate::screens::plugin_ui::views::read::summary_of(self, a) {
            // Its own view says where it stands (rail.go:121).
            if !l.spans.is_empty() {
                l.spans.push(sp(" · ", FAINT));
            }
            join(
                &mut l,
                crate::screens::plugin_ui::views::view_summary_short(&v),
            );
        }
        fit(l, w)
    }

    /// Whether a's row sits under its project's heading, which then names
    /// the project for it (rail.go:184 nested).
    fn nested(&self, a: &Agent) -> bool {
        self.cfg.split_projects() && !self.group_of.get(&a.key).is_some_and(|g| by_time(g))
    }

    /// What git last said of the checkout a works in (rail.go:145
    /// whereGit); None till it has.
    fn where_git(&self, a: &Agent, fk: &str) -> Option<&rush_fleet::GitState> {
        let f = self
            .kept
            .peek::<crate::screens::list_extras::landing::ListFolders>()?
            .by_root
            .get(fk)?;
        let t = tree_of(a);
        if t.is_empty() {
            Some(&f.git)
        } else {
            f.trees.get(t)
        }
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

/// Each step a row sits in under a project, then a worktree (view.go
/// rowInset).
const ROW_INSET: usize = 2;

/// Whether the row on line i is the last at its depth before its section
/// ends, so its branch closes (└─) rather than runs on (├─) (rail.go:63
/// railLast).
fn rail_last(lines: &[ListLine], i: usize) -> bool {
    let at = lines[i].inset();
    for l in &lines[i + 1..] {
        match l {
            l if l.is_row() => {
                if l.inset() <= at {
                    return false;
                }
            }
            ListLine::Section { .. } | ListLine::Blank | ListLine::Project { .. } => return true,
            _ => {}
        }
    }
    true
}

/// Whether the worktree heading on line i has one agent under it, which
/// then stays under its project with the worktree as a tag (view.go:2195
/// soloTree).
fn solo_tree(lines: &[ListLine], i: usize) -> bool {
    lines[i + 1..].iter().take_while(|l| l.is_row()).count() == 1
}

/// A project's heading in the classic look: its name, then a dotted rule
/// to the edge (folders.go:197 projectLine).
fn project_line(title: &str, w: usize) -> Line {
    let mut l = line([plain("  "), sp(title.to_string(), SUB)]);
    let n = w.saturating_sub(l.width() + 3);
    l.spans.push(plain(" "));
    l.spans.push(sp("┄".repeat(n), FAINT));
    l
}

/// A worktree's name, for its heading or beside its one agent's name
/// (folders.go:319 treeTag).
fn tree_tag(tree: &str) -> Line {
    line([sp("↳ ", FAINT), sp(base(tree).to_string(), BLUE)])
}

/// A worktree's heading under its project (folders.go:309 treeLine).
fn tree_line(tree: &str, w: usize) -> Line {
    let mut l = line([plain("    ")]);
    join(&mut l, tree_tag(tree));
    if l.width() + 2 > w {
        return fit(l, w);
    }
    let n = w.saturating_sub(l.width() + 3);
    l.spans.push(plain(" "));
    l.spans.push(sp("┄".repeat(n), FAINT));
    l
}

/// The second line of a session a plugin started: rush's name and the
/// job for one of rush's own (the cleanup agent), or the plugin's name
/// for one you installed; then why it was started. None for any other
/// session (rail.go:130 startedBy, as of df3a73ef).
pub fn started_by(a: &Agent) -> Option<Line> {
    if a.started_by.is_empty() {
        return None;
    }
    let mut l = if rush_plugin::bundled::bundle_named(&a.started_by).is_some() {
        line([
            rush_core::text::Span::new("rush", bold(TEXT).italic()),
            sp(" · ", FAINT),
            sp(a.started_by.clone(), TEXT),
        ])
    } else {
        line([spb(a.started_by.clone(), TEXT)])
    };
    if !a.why.is_empty() {
        l.spans.push(sp(" · ", FAINT));
        l.spans.push(sp(a.why.clone(), DIM));
    }
    Some(l)
}

/// A PR its agent linked, in a few words, coloured by what it needs:
/// failing checks first, then where it stands (rail.go:155 prShort).
pub fn pr_short(pr: &rush_core::adapter::Pr) -> rush_core::text::Span {
    let n = format!("PR #{}", pr.number);
    let c = &pr.checks;
    if pr.state == "MERGED" {
        sp(format!("{n} merged"), BLUE)
    } else if pr.state == "CLOSED" {
        sp(format!("{n} closed"), DIM)
    } else if c.failed > 0 {
        sp(format!("{n} ✗ {} failed", c.failed), RED)
    } else if c.pending > 0 {
        sp(
            format!(
                "{n} · checks {} of {}",
                c.passed,
                c.passed + c.failed + c.pending
            ),
            DIM,
        )
    } else if pr.review.eq_ignore_ascii_case("APPROVED") {
        sp(format!("{n} ✓ approved"), GREEN)
    } else if pr.state == "DRAFT" {
        sp(format!("{n} draft"), DIM)
    } else if c.passed > 0 {
        sp(format!("{n} ✓"), GREEN)
    } else {
        sp(n, BLUE)
    }
}

/// A branch's pull request in a few words, as GitHub last said it: its
/// first failing check by name (rail.go:227 statusShort).
pub fn status_short(pr: &rush_core::agent::PrStatus) -> rush_core::text::Span {
    let n = format!("PR #{}", pr.number);
    if pr.merged() {
        sp(format!("{n} merged"), BLUE)
    } else if pr.state == "closed" {
        sp(format!("{n} closed"), DIM)
    } else if pr.checks == "fail" {
        sp(format!("{n} ✗ {}", pr.failing).trim().to_string(), RED)
    } else if pr.checks == "pending" {
        sp(format!("{n} · checks running"), DIM)
    } else if pr.review == "approved" {
        sp(format!("{n} ✓ approved"), GREEN)
    } else if pr.review == "changes" {
        sp(format!("{n} · changes asked for"), YELLOW)
    } else if pr.state == "draft" {
        sp(format!("{n} draft"), DIM)
    } else if pr.checks == "pass" {
        sp(format!("{n} ✓"), GREEN)
    } else {
        sp(n, BLUE)
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
            pad: 0,
            tag: None,
            inset: 0,
        }
    }
}
