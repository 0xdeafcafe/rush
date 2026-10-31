//! The command bar (ctrl+k, or cmd+k where the terminal passes it on) goes
//! anywhere from anywhere: a place or a page, an agent, a turn of the
//! conversation open, or a match in any agent's transcript. What's typed
//! is matched loosely against names; words also search the open
//! conversation and, after a pause, every transcript in the background.
//! ctrl+f is the same bar found where you are; each ctrl+f widens it.
//!
//! Port of internal/ui/cmdbar.go:37-1408: barScope :38, barScopes :52,
//! barToggle :131, openBar :158, canCut :167, barKey :186, nextSection
//! :255, barChanged :278, searchNow :297, searchable :322, barMsg :350,
//! searchTranscripts :526, splitIn :580, barRebuild :595, matchItem
//! :722, barPlaces :740 (the open Session's views :783-793),
//! barCommands :878, barStart :902, toPrompt :911,
//! barAgents :932, barAgentItem :957, matchWords :971, barSortedGroups
//! :993, barAgentGroups :1034, agentGlyph :1062, agentState :1074,
//! goAgent :1089, barTurns :1101, whoGlyph :1166, withSaid :1179,
//! barTranscripts :1214, sortFound :1254, whenAgo :1263, goFound :1283,
//! jumpInPane :1297, applyJump :1318, here :1359, where :1376, goSpot
//! :1406, litWords :1449. The search itself is search.rs; drawing is
//! draw.rs.

use std::time::{Duration, Instant};

use rush_convo::search::{self as cs, Hit};
use rush_core::agent::Agent;
use rush_core::text::Span;
use rush_core::time::Ms;
use rush_fleet::Predicates;

use super::fleetslash::{available_fleet_commands, completed, needs_agent, needs_arg, round_move};
use super::fuzzy::fuzzy_match;
use super::search::{BarFound, Search};
use crate::frame::editor::{edit, line_end};
use crate::frame::{Out, PlaceId, Press};
use crate::model::Model;
use crate::session::{Pos, harness_name};
use crate::style::*;

/// How far the bar looks (barScope).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Chat,
    Group,
    Agents,
    Place,
    /// Everywhere.
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub kind: ScopeKind,
    /// The chat's agent.
    pub key: String,
    /// The list section.
    pub group: String,
    pub place: PlaceId,
    pub label: String,
}

impl Scope {
    fn new(kind: ScopeKind, label: impl Into<String>) -> Scope {
        Scope {
            kind,
            key: String::new(),
            group: String::new(),
            place: PlaceId::Agents,
            label: label.into(),
        }
    }

    /// Whether the scope searches agents' transcripts.
    pub fn transcripts(&self) -> bool {
        matches!(
            self.kind,
            ScopeKind::All | ScopeKind::Group | ScopeKind::Agents
        )
    }
}

/// What picking a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Go {
    Back(Spot),
    /// Agents' box, to start one; with text, started with it.
    Start(String),
    Agents,
    /// Agents' page by name ("Wall").
    AgentsPage(usize),
    Place(PlaceId, Option<usize>),
    FindHere,
    /// The open session's view by name ("changes").
    View(String),
    /// The agent with this key, full screen in its agent's program.
    Full(String),
    Keys,
    Agent(String),
    Turn(String),
    Found {
        key: String,
        name: String,
        r#ref: String,
        snip: String,
        q: String,
    },
    Command {
        name: String,
        needs_arg: bool,
        hint: String,
    },
}

/// One row of results (barItem).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub section: String,
    /// The agent's, on an agent's row.
    pub key: String,
    /// More it's found by: a place's keywords.
    pub words: String,
    pub glyph: Span,
    /// Plain; lit where the query matched.
    pub title: String,
    /// Char indexes of title to light.
    pub lit: Vec<usize>,
    /// Plain, dim, in a column after the title.
    pub meta: String,
    /// Plain, dim, short, at the right edge.
    pub tail: String,
    pub score: i32,
    pub go: Go,
}

impl Item {
    fn new(glyph: Span, title: impl Into<String>, go: Go) -> Item {
        Item {
            section: String::new(),
            key: String::new(),
            words: String::new(),
            glyph,
            title: title.into(),
            lit: Vec::new(),
            meta: String::new(),
            tail: String::new(),
            score: 0,
            go,
        }
    }
}

/// Somewhere the bar jumped from, for "Back" (spot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spot {
    pub place: PlaceId,
    pub agents_page: usize,
    pub page: usize,
    pub page_name: String,
    pub key: String,
    pub name: String,
    pub r#ref: String,
    pub open: bool,
}

/// A jump into an agent's conversation waiting for it to open: the hit is
/// found again as the conversation reads (barJump).
#[derive(Clone, Debug)]
pub struct Jump {
    pub key: String,
    pub name: String,
    pub r#ref: String,
    pub query: String,
    pub snip: String,
    pub at: Instant,
}

/// The open bar (cmdBar).
pub struct Bar {
    pub scopes: Vec<Scope>,
    pub at: usize,
    pub query: Vec<char>,
    pub pos: usize,
    pub cursor: usize,
    pub top: usize,
    pub items: Vec<Item>,
    pub r#gen: u64,
    pub search: Option<Search>,
    pub found: Vec<BarFound>,
    pub searched: usize,
    pub to_search: usize,
    pub done: bool,
    /// The screen row of each item as last drawn, for the mouse.
    pub rows_at: Vec<(usize, usize)>,
    /// The hint row's screen row and first cell.
    pub hint_at: Option<(usize, usize)>,
}

impl Bar {
    pub fn scope(&self) -> &Scope {
        &self.scopes[self.at]
    }

    pub fn query_text(&self) -> String {
        self.query.iter().collect()
    }

    /// The first row of the next (or previous) section, round.
    pub fn next_section(&self, d: isize) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        let starts: Vec<usize> = (0..self.items.len())
            .filter(|&i| i == 0 || self.items[i].section != self.items[i - 1].section)
            .collect();
        let cur = starts.iter().rposition(|&s| s <= self.cursor).unwrap_or(0);
        let n = starts.len() as isize;
        starts[((cur as isize + d) % n + n) as usize % starts.len()]
    }
}

/// The bar's state, kept between calls.
#[derive(Default)]
pub struct BarState {
    pub bar: Option<Bar>,
    /// Where the last jump came from (barBack).
    pub back: Option<Spot>,
    /// The agents' order (barSort): "" is "status".
    pub sort: &'static str,
    pub jump: Option<Jump>,
    pub r#gen: u64,
}

/// The orders ctrl+s steps the bar's agents through.
pub const BAR_SORTS: [&str; 6] = [
    "status", "latest", "project", "worktree", "model", "harness",
];

/// An agent whose name has the words comes before one where they're
/// only in its project or branch.
pub const NAME_FIRST: i32 = 1 << 12;

fn st(m: &mut Model) -> &mut BarState {
    m.kept::<BarState>()
}

/// Whether the bar is open.
pub fn is_open(m: &Model) -> bool {
    m.kept.peek::<BarState>().is_some_and(|s| s.bar.is_some())
}

/// The scopes ctrl+f steps through from here, narrowest first; the last
/// is always everywhere (barScopes).
pub fn bar_scopes(m: &Model) -> Vec<Scope> {
    let mut out = Vec::new();
    if m.place != PlaceId::Agents {
        let mut s = Scope::new(ScopeKind::Place, m.place.name());
        s.place = m.place;
        out.push(s);
    } else {
        if let Some(v) = &m.session {
            let mut s = Scope::new(ScopeKind::Chat, "this chat");
            s.key = v.key.clone();
            out.push(s);
        }
        let group = match m.sel.strip_prefix('§') {
            Some(g) => g.to_string(),
            None => m.group_of.get(&m.sel).cloned().unwrap_or_default(),
        };
        if !group.is_empty() {
            let mut s = Scope::new(ScopeKind::Group, group.clone());
            s.group = group;
            out.push(s);
        }
        out.push(Scope::new(ScopeKind::Agents, "all agents"));
    }
    out.push(Scope::new(ScopeKind::All, "everywhere"));
    out
}

/// ctrl+k, cmd+k and ctrl+f open the bar and, while it's open, change its
/// scope: ctrl+f widens it, ctrl+k goes to everywhere and, there already,
/// closes it. ctrl+k is still "cut to the end of the line" while there's
/// something after the cursor to cut (barToggle).
pub fn toggle(m: &mut Model, s: &str) -> Out {
    if s != "super+k" && s != "ctrl+k" && s != "ctrl+f" {
        return Out::Pass;
    }
    if let Some(b) = st(m).bar.as_mut() {
        let last = b.scopes.len() - 1;
        if s == "ctrl+f" {
            b.at = (b.at + 1) % b.scopes.len();
        } else if b.at != last {
            b.at = last;
        } else {
            close(m);
            return Out::Draw;
        }
        changed(m);
        return Out::Draw;
    }
    if m.confirm.is_some() || m.place_asking() {
        return Out::Pass;
    }
    if s == "ctrl+k" && can_cut(m) {
        return Out::Pass;
    }
    open(m, s == "ctrl+f");
    Out::Draw
}

/// Opens the bar where you are (here) or everywhere (openBar).
pub fn open(m: &mut Model, here: bool) {
    let scopes = bar_scopes(m);
    let s = st(m);
    s.r#gen += 1;
    let at = if here { 0 } else { scopes.len() - 1 };
    s.bar = Some(Bar {
        scopes,
        at,
        query: Vec::new(),
        pos: 0,
        cursor: 0,
        top: 0,
        items: Vec::new(),
        r#gen: s.r#gen,
        search: None,
        found: Vec::new(),
        searched: 0,
        to_search: 0,
        done: false,
        rows_at: Vec::new(),
        hint_at: None,
    });
    rebuild(m);
}

/// Whether ctrl+k would cut text in the Agents box (canCut).
fn can_cut(m: &Model) -> bool {
    if m.on_place() {
        return false;
    }
    // The Session's box, when it has the keys (canCut's paneFocus).
    if let Some(v) = m.session.as_ref() {
        let core = m.kept.peek::<crate::screens::session_core::Core>();
        return core.and_then(|c| c.conns.get(&v.key)).is_some_and(|c| {
            let pos = c.input.pos;
            line_end(&c.input.text, pos) > pos
        });
    }
    if !m.ext.agents_box {
        return false;
    }
    let pos = m.input.pos;
    line_end(&m.input.text, pos) > pos
}

/// Closes the bar, its search with it (closeBar).
pub fn close(m: &mut Model) {
    st(m).bar = None; // the search stops as it's dropped
}

/// A key while the bar is open (barKey).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    let Some(b) = st(m).bar.as_mut() else {
        return Out::Pass;
    };
    let n = b.items.len();
    match s {
        "esc" | "ctrl+c" => {
            close(m);
            return Out::Draw;
        }
        "up" | "ctrl+p" => b.cursor = round_move(b.cursor, -1, n),
        "down" | "ctrl+n" => b.cursor = round_move(b.cursor, 1, n),
        "pgup" => b.cursor = b.cursor.saturating_sub(10),
        "pgdown" => b.cursor = (b.cursor + 10).min(n.saturating_sub(1)),
        "tab" | "shift+tab" => b.cursor = b.next_section(if s == "tab" { 1 } else { -1 }),
        "ctrl+s" => {
            // The agents' order: by what they need of you, the latest, or
            // in sections by project, worktree, model or harness.
            let sort = st(m).sort;
            let i = BAR_SORTS.iter().position(|x| *x == sort).unwrap_or(0);
            st(m).sort = BAR_SORTS[(i + 1) % BAR_SORTS.len()];
            if let Some(b) = st(m).bar.as_mut() {
                b.cursor = 0;
            }
            changed(m);
        }
        "ctrl+enter" | "ctrl+j" => {
            // ctrl+j is the same key where the terminal can't tell
            // ctrl+enter from enter.
            search_now(m);
        }
        "enter" => {
            if b.cursor >= n {
                return Out::Idle;
            }
            let it = b.items[b.cursor].clone();
            let from = here(m);
            close(m);
            let o = run(m, &it.go);
            if !it.section.is_empty() && it.title != "Back" {
                st(m).back = Some(from);
            }
            return if o == Out::Quit { o } else { Out::Draw };
        }
        _ => {
            // Backspace with nothing typed widens the scope, as ctrl+f does.
            if (s == "backspace" || s == "ctrl+h")
                && b.query.is_empty()
                && b.at < b.scopes.len() - 1
            {
                b.at += 1;
                changed(m);
                return Out::Draw;
            }
            let Some((buf, pos)) = edit(&b.query, b.pos, k) else {
                return Out::Idle;
            };
            if buf.contains(&'\n') {
                return Out::Idle;
            }
            if buf == b.query {
                b.pos = pos;
                return Out::Draw;
            }
            b.query = buf;
            b.pos = pos;
            changed(m);
        }
    }
    Out::Draw
}

/// Rebuilds the rows for a new query and, once typing pauses, searches the
/// transcripts for its words (barChanged).
pub fn changed(m: &mut Model) {
    let on_key = crate::screens::settings::store(m)
        .config
        .search_transcripts_on_key;
    let s = st(m);
    s.r#gen += 1;
    let g = s.r#gen;
    let Some(b) = s.bar.as_mut() else { return };
    b.cursor = 0;
    b.top = 0;
    b.search = None;
    b.r#gen = g;
    b.found.clear();
    b.searched = 0;
    b.to_search = 0;
    b.done = false;
    // With searchTranscriptsOnKey, typing matches only names, places
    // and commands; ctrl+enter searches the transcripts (cmdbar.go:285).
    let go = searchable(&b.query_text()) && b.scope().transcripts() && !on_key;
    rebuild(m);
    if go {
        search_transcripts(m, true);
    }
}

/// Searches the transcripts for what's typed at once (searchNow).
fn search_now(m: &mut Model) {
    let Some(b) = st(m).bar.as_ref() else { return };
    if b.search.is_some() || !b.scope().transcripts() {
        return;
    }
    if !searchable(&b.query_text()) {
        m.flash("type at least 3 letters to search conversations");
        return;
    }
    search_transcripts(m, false);
}

/// A query with enough to it to be worth reading every transcript for.
pub fn searchable(q: &str) -> bool {
    cs::words(&strip_in(q))
        .iter()
        .map(String::len)
        .sum::<usize>()
        >= 3
}

/// Starts reading every agent's transcript for the query, the most
/// recently active first, leaving out the one open (its matches are under
/// This session); in:<name> narrows the agents (searchTranscripts). With
/// pause, it waits for typing to pause first.
fn search_transcripts(m: &mut Model, pause: bool) {
    if pause && m.sender().is_none() {
        // No loop (tests, --render): the pause never ends, as Go's tick
        // never fires in a single frame; ctrl+j searches here and now.
        return;
    }
    let Some(b) = st(m).bar.as_ref() else { return };
    let (q, inn) = split_in(&b.query_text());
    let sc = b.scope().clone();
    let r#gen = b.r#gen;
    let open_key = m.session.as_ref().map(|v| v.key.clone());
    let now = m.now;
    let mut agents: Vec<&Agent> = m
        .agents
        .iter()
        .filter(|a| {
            !(sc.kind == ScopeKind::All && open_key.as_deref() == Some(a.key.as_str()))
                && (sc.kind != ScopeKind::Group || m.group_of.get(&a.key) == Some(&sc.group))
                && (inn.is_empty() || a.display_name.to_lowercase().contains(&inn))
        })
        .collect();
    agents.sort_by_key(|a| a.age(now));
    let jobs: Vec<_> = agents
        .iter()
        .filter_map(|a| {
            super::search::conversation_of(&m.reg, a)
                .map(|s| (a.key.clone(), one_line(&a.display_name), s))
        })
        .collect();
    let n = jobs.len();
    let tx = m.sender();
    let b = st(m).bar.as_mut().expect("open");
    b.to_search = n;
    if n == 0 {
        b.done = true;
        return;
    }
    match tx {
        Some(tx) => b.search = Some(Search::start(tx, r#gen, jobs, q, pause)),
        None => {
            b.search = Some(Search::idle());
            let found = super::search::search_all(&jobs, &q);
            took(m, r#gen, found, true);
        }
    }
}

/// Takes in a batch of what the search found (barMsg's barFoundMsg).
pub fn took(m: &mut Model, r#gen: u64, found: Vec<BarFound>, done: bool) -> Out {
    let Some(b) = st(m).bar.as_mut() else {
        return Out::Idle;
    };
    if b.r#gen != r#gen || b.search.is_none() {
        return Out::Idle;
    }
    for f in found {
        b.searched += 1;
        if f.found.total > 0 {
            b.found.push(f);
        }
    }
    sort_found(&mut b.found);
    b.done = done;
    // Keep the cursor on the row it was on as rows arrive above it.
    let keep = (b.cursor > 0 && b.cursor < b.items.len()).then(|| {
        (
            b.items[b.cursor].title.clone() + &b.items[b.cursor].meta,
            b.items[b.cursor].section.clone(),
        )
    });
    rebuild(m);
    if let Some((t, sec)) = keep
        && let Some(b) = st(m).bar.as_mut()
        && let Some(i) = b
            .items
            .iter()
            .position(|it| it.section == sec && it.title.clone() + &it.meta == t)
    {
        b.cursor = i;
    }
    Out::Draw
}

/// The conversations that matched closest first; of those as close, the
/// agent most recently active (sortFound).
pub fn sort_found(found: &mut [BarFound]) {
    found.sort_by(|a, b| {
        b.found
            .best()
            .cmp(&a.found.best())
            .then(a.rank.cmp(&b.rank))
    });
}

/// Takes in:<name> out of a query (splitIn).
pub fn split_in(q: &str) -> (String, String) {
    let mut keep = Vec::new();
    let mut inn = String::new();
    for f in q.split_whitespace() {
        if let Some(v) = f.to_lowercase().strip_prefix("in:") {
            inn = v.to_string();
            continue;
        }
        keep.push(f);
    }
    (keep.join(" "), inn)
}

pub fn strip_in(q: &str) -> String {
    split_in(q).0
}

/// Lists what the query matches, section by section (barRebuild).
pub fn rebuild(m: &mut Model) {
    let Some(b) = m.kept.peek::<BarState>().and_then(|s| s.bar.as_ref()) else {
        return;
    };
    let raw = b.query_text().trim().to_string();
    let sc = b.scope().clone();
    let (q, inn) = split_in(&raw);
    let words = cs::words(&q);
    let name = words.join(" ");
    let filtered = cs::filtered(&q) || !inn.is_empty();
    let sort = m.kept.peek::<BarState>().map_or("", |s| s.sort);
    let mut out: Vec<Item> = Vec::new();
    let mut add = |section: &str, mut items: Vec<Item>, limit: usize| {
        if !name.is_empty() {
            items.sort_by_key(|it| std::cmp::Reverse(it.score));
        }
        for it in items.iter_mut() {
            it.section = section.to_string();
        }
        items.truncate(limit);
        out.extend(items);
    };
    let session_title = |m: &Model| {
        m.session
            .as_ref()
            .map(|v| one_line(&host_name(m, &v.key)))
            .unwrap_or_default()
    };
    match sc.kind {
        ScopeKind::Chat => {
            if m.session.as_ref().is_some_and(|v| v.key == sc.key) {
                add(
                    &format!("This chat · {}", session_title(m)),
                    bar_turns(m, &q),
                    300,
                );
            }
        }
        ScopeKind::Group | ScopeKind::Agents => {
            if !filtered {
                let title = if sc.kind == ScopeKind::Group {
                    sc.group.clone()
                } else {
                    "Agents".into()
                };
                add(
                    &title,
                    with_said(m, bar_agents(m, &name, &sc.group), &sc.group),
                    40,
                );
            }
            add("Conversations", bar_transcripts(m, &words), 60);
        }
        ScopeKind::Place => {
            let prefix = format!("{} › ", sc.label);
            let pages = bar_places(m, &name)
                .into_iter()
                .filter(|it| it.title.starts_with(&prefix))
                .collect();
            add(&sc.label, pages, 20);
        }
        ScopeKind::All => {
            let mut said = false;
            let turn = raw.strip_prefix('#').unwrap_or(&raw).parse::<u32>().is_ok();
            if filtered {
            } else if raw.starts_with('#') && !turn {
                // # is for commands: none of them clutter the bar until
                // asked for.
                let mut title = "Commands".to_string();
                if let Some(i) = m.focused() {
                    title += &format!(" · on {}", one_line(&m.agents[i].display_name));
                }
                add(&title, bar_commands(m, &name), 40);
            } else if name.is_empty() || sort != "status" && !sort.is_empty() {
                // Agents first, by what they need of you; places once
                // typed for, but for starting one and going back.
                let groups = bar_agent_groups(m, &name, sort);
                let n = groups.len();
                for (t, items) in groups {
                    add(&t, items, 3.max(12 / n.max(1)));
                }
                let places = bar_places(m, &name)
                    .into_iter()
                    .filter(|it| it.title == "Back" || it.title == "Start an agent")
                    .collect();
                add("Go to", places, 2);
            } else {
                // Typed: agents (by name, then by what they said), then
                // what was said; a place first only when it's what's
                // typed, word for word.
                let agents = with_said(m, bar_agents(m, &name, ""), "");
                let mut go_to = Vec::new();
                let mut best = 0;
                for mut it in bar_places(m, &name) {
                    if let Some(sc) =
                        cs::score(&format!("{} {} {}", it.title, it.meta, it.words), &name)
                    {
                        it.score = sc;
                        if let Some(t) = cs::score(&it.title, &name) {
                            it.score = t + NAME_FIRST;
                            best = best.max(it.score);
                        }
                        go_to.push(it);
                    }
                }
                let top = agents.iter().map(|it| it.score).max().unwrap_or(i32::MIN);
                if best > 0 && best >= top {
                    add("Go to", std::mem::take(&mut go_to), 4);
                }
                add("Agents", agents, 10);
                add("Conversations", bar_transcripts(m, &words), 60);
                said = true;
                add("Go to", go_to, 3);
            }
            if m.session.is_some() && inn.is_empty() && (!raw.starts_with('#') || turn) {
                let limit = if raw.is_empty() { 4 } else { 50 };
                add(
                    &format!("This session · {}", session_title(m)),
                    bar_turns(m, &q),
                    limit,
                );
            }
            // Anything else typed can be a new agent's task (#12 is a turn).
            if !raw.is_empty() && !filtered && !raw.starts_with('#') {
                add("Start", vec![bar_start(m, &raw)], 1);
            }
            if !said {
                add("Conversations", bar_transcripts(m, &words), 60);
            }
        }
    }
    let b = st(m).bar.as_mut().expect("open");
    b.cursor = b.cursor.min(out.len().saturating_sub(1));
    b.items = out;
}

fn host_name(m: &Model, key: &str) -> String {
    m.agents
        .iter()
        .find(|a| a.key == key)
        .map_or_else(|| "this agent".into(), |a| a.display_name.clone())
}

/// Scores an item against the words typed; one that doesn't match is
/// left out. Matches in the title are lit; keywords only count
/// (matchItem).
pub fn match_item(mut it: Item, q: &str, keywords: &str) -> Option<Item> {
    if q.is_empty() {
        return Some(it);
    }
    if let Some((s, at)) = fuzzy_match(q, &it.title) {
        it.score = s;
        it.lit = at;
        return Some(it);
    }
    let (s, _) = fuzzy_match(q, &format!("{} {keywords}", it.title))?;
    it.score = s - 20;
    Some(it)
}

/// The places, their pages and a few things to do (barPlaces).
pub fn bar_places(m: &Model, q: &str) -> Vec<Item> {
    let mut items = Vec::new();
    let mut add = |glyph: Span, title: String, meta: String, keywords: String, go: Go| {
        let mut it = Item::new(glyph, title, go);
        it.meta = meta;
        it.words = keywords.clone();
        if let Some(it) = match_item(it, q, &keywords) {
            items.push(it);
        }
    };
    let sub = |s: &str| sp(s, SUB);
    if let Some(bk) = m.kept.peek::<BarState>().and_then(|s| s.back.clone()) {
        let w = bk.where_();
        add(
            sp("↩", ORANGE),
            "Back".into(),
            w.clone(),
            format!("previous return {w}"),
            Go::Back(bk),
        );
    }
    let start = crate::screens::list_extras::prompt::start_dir(m);
    let dir = tildify(&start);
    add(
        sp("+", GREEN),
        "Start an agent".into(),
        format!(
            "in {dir} · {}",
            crate::screens::accounts::setup::start_with_words(m, &start)
        ),
        "new session spawn prompt task model effort profile".into(),
        Go::Start(String::new()),
    );
    add(
        sub("◇"),
        "Agents".into(),
        "the list".into(),
        "home list".into(),
        Go::Agents,
    );
    for (i, p) in m.ext.agents_pages.iter().enumerate() {
        let Some(p) = p else { continue };
        let name = p.name().to_string();
        let (meta, words) = if name == "Wall" {
            (
                "every agent at once, streaming",
                "agents wall grid tiles all agents live stream dashboard",
            )
        } else {
            ("", "agents")
        };
        add(
            sub("◇"),
            format!("Agents › {name}"),
            meta.into(),
            words.into(),
            Go::AgentsPage(i + 1),
        );
    }
    // Projects and its pages are always offered, as Go names them; a page
    // is found by its name once the Projects place has it.
    const PROJECTS_WORDS: &str = "projects repositories repos folders git branches worktrees commits prs cleanup clean disk temporary scratch processes orphans machine system ";
    let projects_pages = m.ext.places[PlaceId::Projects as usize]
        .as_ref()
        .map(|p| p.pages(m))
        .unwrap_or_default();
    for (name, page, what) in [
        (
            "Projects",
            "",
            "each repository whole: branch, changes, worktrees and their sizes, agents' scratch, commits, PRs and its agents",
        ),
        (
            "Projects › Temporary",
            "Temporary",
            "/tmp and finished agents' temp work",
        ),
        ("Projects › System", "System", "processes no project owns"),
        (
            "Projects › Pull requests",
            "Pull requests",
            "yours and asking for your review, by project",
        ),
    ] {
        let at = (!page.is_empty())
            .then(|| projects_pages.iter().position(|p| p == page))
            .flatten();
        add(
            sub("◇"),
            name.into(),
            what.into(),
            format!("{PROJECTS_WORDS}{name}"),
            Go::Place(PlaceId::Projects, at),
        );
    }
    for id in [PlaceId::Harnesses, PlaceId::Settings] {
        let Some(p) = m.ext.places[id as usize].as_ref() else {
            continue;
        };
        let pages = p.pages(m);
        if pages.is_empty() {
            add(
                sub("◇"),
                id.name().into(),
                String::new(),
                format!("settings preferences harnesses agents {}", id.name()),
                Go::Place(id, None),
            );
        }
        for (i, page) in pages.iter().enumerate() {
            add(
                sub("◇"),
                format!("{} › {page}", id.name()),
                String::new(),
                format!("settings preferences harnesses agents {page}"),
                Go::Place(id, Some(i)),
            );
        }
    }
    // The Session's views, opened or beside the list (Go's m.host stays
    // the one shown when the list has the keys; cmdbar.go:783-793).
    let shown = m
        .session
        .as_ref()
        .or(m.pane.as_ref())
        .map(|s| s.key.clone());
    if let Some(key) = shown {
        let name = one_line(&host_name(m, &key));
        for v in crate::screens::session_core::views::views(m, &key) {
            let mut c = v.chars();
            let title = c
                .next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default();
            add(
                sp("▤", BLUE),
                format!("Session › {title}"),
                name.clone(),
                format!("session view {v}"),
                Go::View(v.into()),
            );
        }
        add(
            sp("⌕", BLUE),
            "Find in this chat".into(),
            "ctrl+f".into(),
            "search find session".into(),
            Go::FindHere,
        );
    }
    if let Some(a) = m.focused().map(|i| &m.agents[i])
        && super::attach::offers_full(a)
    {
        add(
            sub("◇"),
            "Open full screen".into(),
            format!("{} in Claude Code", one_line(&a.display_name)),
            "attach claude code full screen native".into(),
            Go::Full(a.key.clone()),
        );
    }
    add(
        sub("◇"),
        "All keys".into(),
        "?".into(),
        "help shortcuts keys".into(),
        Go::Keys,
    );
    items
}

/// rush's # commands and others', runnable from the bar: picking one drops
/// it into the Agents box, the way typing it there would, and runs it at
/// once when it needs nothing more (barCommands).
pub fn bar_commands(m: &Model, q: &str) -> Vec<Item> {
    let focused = m.focused().is_some();
    let mut cmds = available_fleet_commands(m);
    for c in &m.ext.commands {
        if !cmds.iter().any(|x| x.name == c.name) {
            cmds.push(rush_core::event::Command {
                name: c.name.into(),
                description: c.about.into(),
                ..Default::default()
            });
        }
    }
    let mut items = Vec::new();
    for c in cmds {
        if needs_agent(&c.name) && !focused {
            continue;
        }
        let mut it = Item::new(
            sp("›", SUB),
            format!("#{}", c.name),
            Go::Command {
                name: c.name.clone(),
                needs_arg: needs_arg(&c),
                hint: c.argument_hint.clone(),
            },
        );
        it.meta = rush_convo::render::key_word(&c.description);
        if let Some(it) = match_item(it, q, &format!("{} {}", c.name, c.description)) {
            items.push(it);
        }
    }
    items
}

/// Starts a new agent with what's typed as its task (barStart).
fn bar_start(m: &Model, task: &str) -> Item {
    let mut it = Item::new(sp("+", GREEN), task, Go::Start(task.into()));
    it.meta = format!(
        "new agent in {}",
        tildify(&crate::screens::list_extras::prompt::start_dir(m))
    );
    it
}

/// The agents, those needing you first, then the latest (barAgents).
pub fn bar_agents(m: &Model, q: &str, group: &str) -> Vec<Item> {
    let now = m.now;
    let mut agents: Vec<&Agent> = m.agents.iter().collect();
    agents.sort_by(|a, b| {
        b.needs_you()
            .cmp(&a.needs_you())
            .then(a.age(now).cmp(&b.age(now)))
    });
    agents
        .into_iter()
        .filter(|a| group.is_empty() || m.group_of.get(&a.key).map(String::as_str) == Some(group))
        .filter_map(|a| agent_item(m, a, q))
        .collect()
}

/// a's row in the bar, if it matches q (barAgentItem).
pub fn agent_item(m: &Model, a: &Agent, q: &str) -> Option<Item> {
    let repo = base_name(&a.repo);
    let where_ = format!("{repo} · {}", a.branch)
        .trim_matches(|c| c == ' ' || c == '·')
        .to_string();
    let mut it = Item::new(
        agent_glyph(a),
        one_line(&a.display_name),
        Go::Agent(a.key.clone()),
    );
    it.key = a.key.clone();
    it.meta = where_;
    it.tail = agent_state(a, m.now);
    let group = m.group_of.get(&a.key).cloned().unwrap_or_default();
    let kw = format!(
        "{} {} {group} {} {}",
        a.repo,
        a.branch,
        a.job.state.as_str(),
        harness_name(&m.reg, &a.kind)
    );
    match_words(it, q, &kw)
}

fn base_name(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    std::path::Path::new(p)
        .file_name()
        .map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Scores an agent's row by the words typed, as conversations are: those
/// in the name before those only in its project, branch or state; the
/// words lit (matchWords).
pub fn match_words(mut it: Item, q: &str, keywords: &str) -> Option<Item> {
    if q.trim().is_empty() {
        return Some(it);
    }
    if let Some(sc) = cs::score(&it.title, q) {
        it.score = sc + NAME_FIRST;
        it.lit = lit_words(&it.title, &cs::words(q));
        return Some(it);
    }
    it.score = cs::score(&format!("{} {keywords}", it.title), q)?;
    Some(it)
}

/// The agents matching q, latest first, in one section or a section each
/// by the bar's sort (barSortedGroups); by status, in sections by what
/// they need of you (barAgentGroups).
pub fn bar_agent_groups(m: &Model, q: &str, sort: &str) -> Vec<(String, Vec<Item>)> {
    let mut out: Vec<(String, Vec<Item>)> = Vec::new();
    let mut push = |title: String, it: Item| match out.iter_mut().find(|(t, _)| *t == title) {
        Some((_, v)) => v.push(it),
        None => out.push((title, vec![it])),
    };
    if sort.is_empty() || sort == "status" {
        let mut by: Vec<(String, Vec<Item>)> = ["Needs you", "Working", "Idle", "Earlier"]
            .iter()
            .map(|t| (t.to_string(), Vec::new()))
            .collect();
        for it in bar_agents(m, "", "") {
            let g = match it.tail.as_str() {
                "needs you" | "waiting" => 0,
                "working" => 1,
                "idle" => 2,
                _ => 3,
            };
            by[g].1.push(it);
        }
        return by.into_iter().filter(|(_, v)| !v.is_empty()).collect();
    }
    let now = m.now;
    let mut agents: Vec<&Agent> = m.agents.iter().collect();
    agents.sort_by_key(|a| a.age(now));
    for a in agents {
        let Some(it) = agent_item(m, a, q) else {
            continue;
        };
        let title = match sort {
            "project" => base_name(if a.repo.is_empty() {
                &a.job.cwd
            } else {
                &a.repo
            }),
            "worktree" => format!("{} · {}", tildify(&a.job.cwd), a.branch)
                .trim_matches(|c| c == ' ' || c == '·')
                .to_string(),
            "model" => {
                if a.spend.model.is_empty() {
                    "model unknown".into()
                } else {
                    a.spend.model.clone()
                }
            }
            "harness" => harness_name(&m.reg, &a.kind),
            _ => "Latest".into(),
        };
        push(title, it);
    }
    out
}

/// An agent's dot: yellow when it needs you, orange at work, hollow idle.
pub fn agent_glyph(a: &Agent) -> Span {
    if a.needs_you() {
        sp("●", YELLOW)
    } else if a.job.live() || a.busy() {
        sp("●", ORANGE)
    } else if a.pid != 0 {
        sp("○", SUB)
    } else {
        sp("·", FAINT)
    }
}

/// An agent's state in a word, or how long ago it finished.
pub fn agent_state(a: &Agent, now: Ms) -> String {
    if a.needs_you() {
        "needs you".into()
    } else if a.job.live() || a.busy() {
        "working".into()
    } else if a.pid != 0 {
        "idle".into()
    } else {
        format!("{} ago", age(a.age(now)))
    }
}

/// The open conversation's turns, the latest first, or its matches for
/// the query (barTurns).
pub fn bar_turns(m: &Model, q: &str) -> Vec<Item> {
    let Some(v) = &m.session else {
        return Vec::new();
    };
    let s = &v.sess;
    let mut items = Vec::new();
    let tq = q.trim();
    // #12 is turn 12.
    if let Some(n) = tq.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()) {
        for t in s.turns.iter().filter(|t| t.n == n) {
            let mut it = Item::new(sp("›", SUB), one_line(&t.prompt), Go::Turn(format!("t{n}")));
            it.tail = format!("#{n}");
            it.score = 1000;
            items.push(it);
        }
        return items;
    }
    if cs::words(q).is_empty() && !cs::filtered(q) {
        for t in s.turns.iter().rev() {
            let text = one_line(&t.prompt);
            if text.is_empty() {
                continue;
            }
            let mut meta = if s.partial {
                String::new()
            } else {
                format!("#{}", t.n)
            };
            if t.start > 0 {
                meta = format!("{} ago  {meta}", age(m.now - t.start))
                    .trim()
                    .to_string();
            }
            let mut it = Item::new(sp("›", SUB), text, Go::Turn(format!("t{}", t.n)));
            it.tail = meta;
            items.push(it);
        }
        return items;
    }
    let words = cs::words(q);
    let mut hits = s.search(q);
    hits.reverse(); // of those as close, the latest first
    for h in hits {
        let tail = match when_ago(h.at, m.now) {
            at if !at.is_empty() => format!("#{} · sent {at}", h.turn),
            _ => format!("#{} {}", h.turn, h.who),
        };
        let title = homeless(&h.snippet);
        let mut it = Item::new(who_glyph(&h.who), title.clone(), Go::Turn(h.r#ref.clone()));
        it.lit = lit_words(&title, &words);
        it.tail = tail;
        it.score = h.score;
        items.push(it);
    }
    items
}

/// Text with the home folder in it written ~ (homeless).
pub fn homeless(s: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => s.replace(&h, "~"),
        _ => s.to_string(),
    }
}

/// Who said a hit: you, the agent, or a tool (whoGlyph).
pub fn who_glyph(who: &str) -> Span {
    match who {
        "you" => sp("›", TEXT),
        cs::WHO_AGENT => sp("✻", ORANGE),
        _ => sp("·", SUB),
    }
}

/// items, then every agent whose conversation has the words (in group,
/// when one's given), saying how often and where (withSaid).
pub fn with_said(m: &Model, mut items: Vec<Item>, group: &str) -> Vec<Item> {
    let Some(b) = m.kept.peek::<BarState>().and_then(|s| s.bar.as_ref()) else {
        return items;
    };
    let now = m.now;
    for f in &b.found {
        let Some(a) = m.agents.iter().find(|a| a.key == f.key) else {
            continue;
        };
        if items.iter().any(|it| it.key == a.key)
            || f.found.hits.is_empty()
            || !group.is_empty() && m.group_of.get(&a.key).map(String::as_str) != Some(group)
        {
            continue;
        }
        let Some(mut it) = agent_item(m, a, "") else {
            continue;
        };
        let h = &f.found.hits[0];
        it.meta = format!("said {}", quoted(&homeless(&h.snippet)));
        let at = when_ago(h.at, now);
        if !at.is_empty() {
            it.meta += &format!(" · {at}");
        }
        it.tail = format!("{} found", f.found.total);
        it.score = -(1 << 20) + f.found.best(); // after every name that matched
        items.push(it);
    }
    items
}

/// A snippet in quotes, as said.
pub fn quoted(s: &str) -> String {
    format!("“{}”", one_line(s).trim())
}

/// The matches found in other agents' transcripts so far
/// (barTranscripts).
pub fn bar_transcripts(m: &Model, words: &[String]) -> Vec<Item> {
    let Some(b) = m.kept.peek::<BarState>().and_then(|s| s.bar.as_ref()) else {
        return Vec::new();
    };
    let q = strip_in(&b.query_text());
    let now = m.now;
    let mut all: Vec<(&BarFound, &Hit)> = b
        .found
        .iter()
        .flat_map(|f| f.found.hits.iter().map(move |h| (f, h)))
        .collect();
    // The closest first; of those as close, the latest said.
    all.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(b.1.at.cmp(&a.1.at)));
    all.into_iter()
        .map(|(f, h)| {
            let mut meta = one_line(&f.name);
            if f.found.first > 0 {
                meta += &format!(" · started {}", when_ago(f.found.first, now));
            }
            if f.found.last > 0 {
                meta += &format!(", updated {}", when_ago(f.found.last, now));
            }
            let at = when_ago(h.at, now);
            let tail = if at.is_empty() {
                format!("#{}", h.turn)
            } else {
                format!("sent {at}")
            };
            let title = homeless(&h.snippet);
            let mut it = Item::new(
                who_glyph(&h.who),
                title.clone(),
                Go::Found {
                    key: f.key.clone(),
                    name: f.name.clone(),
                    r#ref: h.r#ref.clone(),
                    snip: h.snippet.clone(),
                    q: q.clone(),
                },
            );
            it.lit = lit_words(&title, words);
            it.meta = meta;
            it.tail = tail;
            it
        })
        .collect()
}

/// About how long d is: 5m, 3h, 2d (view.go:479 roughly).
pub fn roughly(d: Ms) -> String {
    const MIN: Ms = 60_000;
    const HOUR: Ms = 60 * MIN;
    if d < HOUR {
        format!("{}m", ((d + MIN / 2) / MIN).max(1))
    } else if d < 48 * HOUR {
        format!("{}h", (d + HOUR / 2) / HOUR)
    } else {
        format!("{}d", (d + 12 * HOUR) / (24 * HOUR))
    }
}

/// When t was, as people say it: 5m ago, yesterday, or the date past a
/// week; "" when it isn't known (whenAgo).
pub fn when_ago(t: Ms, now: Ms) -> String {
    const HOUR: Ms = 3_600_000;
    let d = now - t;
    if t <= 0 {
        String::new()
    } else if d < 60_000 {
        "just now".into()
    } else if d < 24 * HOUR {
        format!("{} ago", roughly(d))
    } else if d < 48 * HOUR {
        "yesterday".into()
    } else if d < 7 * 24 * HOUR {
        format!("{} ago", roughly(d))
    } else {
        let (y, mo, day) = local_date(t);
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let mon = MONTHS[(mo - 1) as usize];
        if y == local_date(now).0 {
            format!("{day} {mon}")
        } else {
            format!("{day} {mon} {y}")
        }
    }
}

/// The local date at ms: year, month (1-12), day.
fn local_date(ms: Ms) -> (i64, u32, u32) {
    let secs = ms / 1000 + crate::model::local_zone(ms);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(mo <= 2);
    (y, mo, d)
}

/// The chars of s where the query's words are found (litWords).
pub fn lit_words(s: &str, words: &[String]) -> Vec<usize> {
    let mut at = Vec::new();
    for w in words {
        let Some((i, j)) = cs::match_span(s, w) else {
            continue;
        };
        let from = s[..i].chars().count();
        at.extend(from..from + s[i..j].chars().count());
    }
    at
}

/// Does what a picked row says.
pub fn run(m: &mut Model, go: &Go) -> Out {
    match go {
        Go::Back(s) => return go_spot(m, s.clone()),
        Go::Start(task) => {
            to_prompt(m);
            if task.is_empty() {
                return Out::Draw;
            }
            // What was typed in the box is one undo away once it's sent.
            let was = (m.input.text.clone(), m.input.pos);
            m.input.clear();
            m.input.insert_str(task);
            return submit_keeping(m, was);
        }
        Go::Agents => {
            let from = m.place;
            go_place(m, PlaceId::Agents);
            m.agents_page = 0;
            m.land_on_needs(from);
        }
        Go::AgentsPage(i) => {
            go_place(m, PlaceId::Agents);
            m.set_page(*i);
        }
        Go::Place(id, page) => {
            go_place(m, *id);
            if let Some(p) = page {
                m.set_page(*p);
            }
        }
        Go::FindHere => {
            go_place(m, PlaceId::Agents);
            focus_session(m);
            open(m, true);
        }
        Go::View(v) => {
            go_place(m, PlaceId::Agents);
            if let Some(key) = focus_session(m) {
                crate::screens::session_core::views::show_view(m, &key, v);
            }
        }
        Go::Full(key) => {
            let a = m.agents.iter().find(|a| a.key == *key).cloned();
            return super::attach::attach(m, a);
        }
        Go::Keys => super::keyhelp::open(m),
        Go::Agent(key) => go_agent(m, key),
        Go::Turn(r) => jump_in_pane(m, r),
        Go::Found {
            key,
            name,
            r#ref,
            snip,
            q,
        } => {
            if !m.agents.iter().any(|a| a.key == *key) {
                m.flash_err(&format!("{name} isn't in the list any more"));
                return Out::Draw;
            }
            st(m).jump = Some(Jump {
                key: key.clone(),
                name: name.clone(),
                r#ref: r#ref.clone(),
                query: q.clone(),
                snip: snip.clone(),
                at: Instant::now(),
            });
            go_agent(m, key);
            apply_jump(m);
        }
        Go::Command {
            name,
            needs_arg: need,
            hint,
        } => {
            to_prompt(m);
            let c = rush_core::event::Command {
                name: name.clone(),
                argument_hint: hint.clone(),
                ..Default::default()
            };
            let was = (m.input.text.clone(), m.input.pos);
            m.input.clear();
            m.input
                .insert_str(&completed("#", &c, false).into_iter().collect::<String>());
            if !need {
                return submit_keeping(m, was);
            }
        }
    }
    Out::Draw
}

/// Sends the Agents box, then keeps what it held before (was) as the step
/// undo brings back: a bar's pick put in place of what was typed loses
/// nothing.
fn submit_keeping(m: &mut Model, was: (Vec<char>, usize)) -> Out {
    let o = m.submit();
    if !was.0.is_empty() && m.input.is_empty() {
        m.input.undo.save(&was.0, was.1, false);
    }
    o
}

/// Moves to a place unless it's already there (goView).
fn go_place(m: &mut Model, id: PlaceId) {
    if m.place != id {
        m.set_place(id);
    }
}

/// The Session beside the list given the keys, shown (m.preview,
/// m.paneFocus = true): its key, or the opened one's.
fn focus_session(m: &mut Model) -> Option<String> {
    if m.session.is_none()
        && let Some(key) = m.pane.as_ref().map(|v| v.key.clone())
        && let Some(i) = m.agents.iter().position(|a| a.key == key)
    {
        m.open(i);
    }
    m.session.as_ref().map(|v| v.key.clone())
}

/// The keys to Agents' box, where new sessions start (toPrompt). Only
/// the keys move: the Session stays open beside the list as it is, still
/// on its host (Go's paneFocus = false keeps m.host), pushed off the
/// whole screen when there's room for both, so the Prompt shows (Go
/// leaves m.full, the Prompt then out of sight); with no room the list
/// takes the screen (leavePane).
pub fn to_prompt(m: &mut Model) {
    use crate::screens::list_extras::split;
    go_place(m, PlaceId::Agents);
    m.agents_page = 0;
    if m.session.is_some() {
        let full = std::mem::replace(&mut m.full, false);
        if !split::side_by_side(m) {
            m.full = full; // leave_chat says how it was left
        }
        split::leave_pane(m);
    }
}

/// Selects an agent and opens its conversation (goAgent).
pub fn go_agent(m: &mut Model, key: &str) {
    go_place(m, PlaceId::Agents);
    m.agents_page = 0;
    if !m.sel.is_empty() && !m.sel.starts_with('§') {
        m.shown = m.sel.clone();
    }
    m.sel = key.to_string();
    if m.session.as_ref().is_some_and(|v| v.key == key) {
        return;
    }
    if let Some(i) = m.agents.iter().position(|a| a.key == key) {
        m.open(i);
    }
}

/// Opens a turn or step of the open conversation and selects it
/// (jumpInPane).
pub fn jump_in_pane(m: &mut Model, r: &str) {
    go_place(m, PlaceId::Agents);
    m.agents_page = 0;
    let Some(v) = m.session.as_mut() else { return };
    let turn = r.split(':').next().unwrap_or(r).to_string();
    v.o.open.insert(turn.clone(), true);
    if r.contains(':') {
        v.o.open.insert(r.to_string(), true);
        let p = v.sess.parent_ref(r);
        if !p.is_empty() {
            v.o.open.insert(p, true); // a subagent's step shows under its opened parent
        }
    }
    v.sel = r.to_string();
    if let Some(n) = turn.strip_prefix('t').and_then(|n| n.parse().ok()) {
        v.pos = Pos::At { n, off: 0 };
    }
}

/// Finishes a jump into a conversation once it's open: the match is looked
/// for again as the conversation has it, until found or a few seconds
/// have gone by (applyJump).
pub fn apply_jump(m: &mut Model) -> bool {
    let Some(j) = m.kept.peek::<BarState>().and_then(|s| s.jump.clone()) else {
        return false;
    };
    if j.at.elapsed() > Duration::from_secs(6) {
        st(m).jump = None;
        m.flash_err(&format!("couldn't find that in {}'s conversation", j.name));
        return true;
    }
    let Some(v) = m.session.as_ref().filter(|v| v.key == j.key && v.read) else {
        return false;
    };
    let mut r = String::new();
    if j.query.is_empty() {
        let turn = j.r#ref.split(':').next().unwrap_or("");
        if v.sess.turns.iter().any(|t| format!("t{}", t.n) == turn) {
            r = j.r#ref.clone();
        }
    } else {
        for h in v.sess.search(&j.query) {
            if h.snippet != j.snip {
                continue;
            }
            if r.is_empty() || h.r#ref == j.r#ref {
                r = h.r#ref;
            }
        }
    }
    if r.is_empty() {
        return false; // still reading
    }
    st(m).jump = None;
    jump_in_pane(m, &r);
    true
}

/// Where the screen is now, to come back to (here).
pub fn here(m: &mut Model) -> Spot {
    let name = m
        .agents
        .iter()
        .find(|a| a.key == m.sel)
        .map(|a| one_line(&a.display_name))
        .unwrap_or_default();
    let (page, page_name) = match m.page_tabs() {
        Some(t) if m.place != PlaceId::Agents => {
            (t.cur, t.names.get(t.cur).cloned().unwrap_or_default())
        }
        Some(t) if m.agents_page > 0 => (t.cur, t.names.get(t.cur).cloned().unwrap_or_default()),
        _ => (0, String::new()),
    };
    let (open, r) = match &m.session {
        Some(v) if v.key == m.sel => (true, v.sel.clone()),
        _ => (false, String::new()),
    };
    Spot {
        place: m.place,
        agents_page: m.agents_page,
        page,
        page_name,
        key: m.sel.clone(),
        name,
        r#ref: r,
        open,
    }
}

impl Spot {
    /// Where it is, in words (spot.where).
    pub fn where_(&self) -> String {
        match self.place {
            PlaceId::Agents if self.agents_page > 0 => format!("Agents › {}", self.page_name),
            PlaceId::Agents if self.name.is_empty() => "Agents".into(),
            PlaceId::Agents => self.name.clone(),
            p if self.page_name.is_empty() => p.name().into(),
            p => format!("{} › {}", p.name(), self.page_name),
        }
    }
}

/// Goes back to where the last jump came from, and makes where it went the
/// new way back (goSpot).
fn go_spot(m: &mut Model, s: Spot) -> Out {
    let back = here(m);
    st(m).back = Some(back);
    if s.place != PlaceId::Agents {
        go_place(m, s.place);
        m.set_page(s.page);
        return Out::Draw;
    }
    go_place(m, PlaceId::Agents);
    m.set_page(s.agents_page);
    if s.agents_page > 0 || !m.agents.iter().any(|a| a.key == s.key) {
        return Out::Draw;
    }
    if !s.open {
        m.sel = s.key.clone();
        m.session = None;
        return Out::Draw;
    }
    if !s.r#ref.is_empty() {
        st(m).jump = Some(Jump {
            key: s.key.clone(),
            name: s.name.clone(),
            r#ref: s.r#ref.clone(),
            query: String::new(),
            snip: String::new(),
            at: Instant::now(),
        });
    }
    go_agent(m, &s.key);
    apply_jump(m);
    Out::Draw
}
