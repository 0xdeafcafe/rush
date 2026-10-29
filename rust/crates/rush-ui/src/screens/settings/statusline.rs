//! /statusline: builds status lines. Pick what shows and in what order,
//! see it drawn with this session's numbers, save. Three, one per tab:
//! the agent header at the top of an agent's Session, the top bar at the
//! top right of rush, and Claude Code's line under its prompt, which
//! saving points at `rush statusline`. After Go's
//! internal/ui/statuslinesheet.go (statusSheet :32, tabs :78, tabNames
//! :86, openStatusLine :100, adopt :135, openTopBar :164, lay :184,
//! maxLines :194, pad :206, segs :217, previewInput :247, slots :283,
//! place :301, rows :328, mouse :353, key :430, trimmed :528, save :539,
//! write :574, turnOff :601, sample :617, body :629, cutout :811).
//!
//! The header's and top bar's own segments, how they draw and what had
//! no room are the header's module's (Go's bars.go): it hands them over
//! through [`provide`]. Until it does, those tabs list bars.go's segments
//! by name, with nothing drawn.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rush_bundled::statusline::{self as sl, Bars, Input, Layout, Renderer};
use rush_core::adapter::{Feature, Profile};
use rush_core::text::{Color, Line};

use super::common::{harness_name, reg};
use super::config;
use super::paint::{from_ansi, trunc};
use crate::frame::overlayclick::MouseEv;
use crate::frame::picker::round_move;
use crate::frame::sheet::{Sheet, sheet_row, sheet_tabs, sheet_title, window};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// The header's two bars, as bars.go names them.
pub const BAR_TOP: usize = 0;
pub const BAR_AGENT: usize = 1;

/// A segment as the builder lists it: id, name, what it is (segInfo).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegInfo {
    pub id: String,
    pub name: String,
    pub about: String,
}

/// What the header's module hands the builder.
#[derive(Clone, Copy)]
pub struct Header {
    /// A bar's segments (barSegs).
    pub segs: fn(usize) -> Vec<SegInfo>,
    /// One segment on its own, as the bar would draw it now.
    pub sample: fn(&Model, usize, &str) -> Line,
    /// The bar as it's drawn now, w wide: rush's top, or an agent's
    /// header with its rows (agent key).
    pub preview: fn(&Model, usize, &str, usize) -> Vec<Line>,
    /// What the bar drawn last had no room for (barDropped).
    pub dropped: fn(&Model, usize) -> HashSet<String>,
    /// The bars as kept, and saved: drawn from now on.
    pub bars: fn(&Model) -> Option<Bars>,
    pub saved: fn(&mut Model, Bars),
}

static PROVIDED: OnceLock<Header> = OnceLock::new();

/// The header module's part, once.
pub fn provide(h: Header) {
    let _ = PROVIDED.set(h);
}

/// The top bar's segments, until the header's module provides them
/// (bars.go:49 topSegs: id, name, about).
const TOP_SEGS: &[(&str, &str, &str)] = &[
    (
        "plan",
        "Plan summary",
        "the default provider’s usage, with fixed window order; add Plan usage for all accounts, meters and reset times",
    ),
    (
        "memory",
        "Agent memory",
        "memory held by agents and their processes, with a fixed-width reading",
    ),
    (
        "system",
        "System alerts",
        "network trouble, low disk or low battery; quiet while healthy",
    ),
    (
        "statushelp",
        "Status customization",
        "opens the status-line editor: full CPU, tokens, storage, battery and detailed usage remain available",
    ),
    ("today", "Spend today", "what every account has spent today"),
    (
        "usage",
        "Plan usage",
        "the account in use: its 5-hour and weekly limits, and when they reset; then every other provider's, the least room first",
    ),
    (
        "accounts",
        "Every account",
        "each account's shortest limit, of every agent, when there are several",
    ),
    (
        "agents",
        "Agents",
        "how many agents there are, and how many are running",
    ),
    ("ram", "Memory", "what agents and their processes hold"),
    (
        "tokens",
        "Tokens",
        "context tokens held by every agent still working",
    ),
    ("cpu", "CPU", "what agents and their processes use"),
    (
        "net",
        "Network",
        "whether the network has been steady: yellow while the API has lately failed, been slow, or the network changed, offline in red while it can't be reached · #network says more",
    ),
    (
        "tmp",
        "Temp files",
        "agents' scratch left on disk, when there's much",
    ),
    (
        "disk",
        "Free disk",
        "space left on the disk agents work on: yellow under 20G, red under 5G",
    ),
    (
        "battery",
        "Battery",
        "the battery's charge, ⚡ while plugged in: yellow under 30%, red under 15% off power",
    ),
    (
        "account",
        "Account",
        "the agent and account new agents start on, and their profile",
    ),
    ("clock", "Clock", "the time of day"),
    (
        "version",
        "rush version",
        "the rush this window runs: yellow once a newer one is installed (#reload), orange while a newer one is out (#update)",
    ),
];

/// The agent header's (bars.go:169 agentSegs).
const AGENT_SEGS: &[(&str, &str, &str)] = &[
    (
        "context",
        "Context",
        "context fullness; click the header readout for token counts and breakdown",
    ),
    ("cost", "Cost", "what the agent has cost so far"),
    (
        "billing",
        "Billing",
        "how it's paid for, when not out of a plan: extra usage in yellow, an API key in orange",
    ),
    ("folder", "Folder", "the folder it works in"),
    ("branch", "Git branch", "the branch checked out there"),
    ("model", "Model", "the model it runs"),
    (
        "rush",
        "Older rush",
        "shown when its host runs an older rush than the one installed",
    ),
    ("effort", "Effort", "the effort level"),
    (
        "mode",
        "Permissions",
        "the permission mode: plan in blue, the ones that don't ask in orange",
    ),
    (
        "tmp",
        "Temp files",
        "its scratch left on disk, when there's much",
    ),
    ("lines", "Lines changed", "lines it added and removed"),
    ("files", "Files changed", "how many files it changed"),
    ("turns", "Turns", "how many of your messages it has had"),
    ("time", "Time", "how long since its first turn"),
    (
        "cache",
        "Prompt cache",
        "whether the cache is warm, and for how long: a message after that re-reads everything",
    ),
    ("queue", "Queue", "messages waiting for the turn to end"),
    (
        "account",
        "Account",
        "the agent it runs, and the login it runs on",
    ),
    ("session", "Session id", "the conversation's short id"),
    ("clock", "Clock", "the time of day"),
];

fn header() -> Header {
    PROVIDED.get().copied().unwrap_or(Header {
        segs: |which| {
            let all = if which == BAR_TOP {
                TOP_SEGS
            } else {
                AGENT_SEGS
            };
            all.iter()
                .map(|&(id, name, about)| SegInfo {
                    id: id.into(),
                    name: name.into(),
                    about: about.into(),
                })
                .collect()
        },
        sample: |_, _, _| Line::new(),
        preview: |_, _, _, _| Vec::new(),
        dropped: |_, _| HashSet::new(),
        bars: |_| None,
        saved: |_, _| {},
    })
}

/// The tabs, in order.
pub const ST_AGENT: usize = 0;
pub const ST_TOP: usize = 1;
const ST_CLAUDE: usize = 2;
const STATUS_TABS: usize = 3;
const TAB_NAMES: [&str; 3] = ["Agent header", "Top bar", "Claude Code"];

/// A row of the builder: a segment on a line, or not shown (line None)
/// (slot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub line: Option<usize>,
    pub id: String,
}

/// A row of the builder's list: a line's heading, the note on an empty
/// line, or a segment (its index in slots); line lines.len() is Not shown
/// (listRow).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListRow {
    pub line: Option<usize>,
    pub head: bool,
    pub empty: bool,
    pub slot: Option<usize>,
}

/// The builder (statusSheet).
pub struct StatusSheet {
    pub prof: Profile,
    pub kind: String,
    /// The session's agent has a line of its own: its tab shows.
    pub line: bool,
    /// The agent whose header the previews are of.
    pub agent: String,
    pub tab: usize,
    /// The agent's own line under its prompt.
    pub agent_lay: Layout,
    pub bars: Bars,
    /// The agent's layout as opened, to tell if it changed.
    was: String,
    pub cur: usize,
    input: Input,
    /// Its settings' statusLine command now.
    pub current: String,
    pub err: String,
    rnd: Renderer,
    /// The agent's layout and settings have been read.
    pub loaded: bool,
    /// A save is being written.
    pub saving: bool,
    tabs_y: usize,
    list_y: usize,
    from: usize,
    tab_ends: Vec<usize>,
    /// The segment being dragged.
    pub drag: String,
}

/// This session as Claude Code would describe it to the status line, with
/// made-up numbers where there aren't any yet (previewInput).
pub fn preview_input(session_id: &str, cwd: &str, model: &str, cost: f64) -> Input {
    let mut i = Input::default();
    i.session_id = if session_id.is_empty() {
        "3f2a9c1e-0000".into()
    } else {
        session_id.into()
    };
    i.cwd = cwd.into();
    i.version = "2.1".into();
    i.workspace.current_dir = cwd.into();
    i.model.display_name = if model.is_empty() { "Opus" } else { model }.into();
    i.effort.level = "high".into();
    i.cost.usd = if cost == 0.0 { 1.27 } else { cost };
    i.cost.duration_ms = 23.0 * 60.0 * 1000.0;
    i.cost.lines_added = 48;
    i.cost.lines_removed = 12;
    i.context.size = if i.model.display_name.to_lowercase().contains("1m") {
        1_000_000
    } else {
        200_000
    };
    i.context.input = i.context.size * 34 / 100;
    i
}

/// Opens the builder for agent kind's session on prof (openStatusLine).
pub fn open_status_line(m: &mut Model, kind: &str, prof: Profile, agent: &str, input: Input) {
    let bars = (header().bars)(m).unwrap_or_else(sl::load_bars);
    let mut bars = bars;
    if bars.top.lines.is_empty() {
        bars.top = sl::default_top();
    }
    if bars.agent.lines.is_empty() {
        bars.agent = sl::default_agent();
    }
    let line = reg().supports(kind, Feature::StatusLine)
        && reg().get(kind).is_some_and(|a| a.status_liner().is_some());
    let mut st = StatusSheet {
        prof: prof.clone(),
        kind: kind.into(),
        line,
        agent: agent.into(),
        tab: ST_AGENT,
        agent_lay: Layout::default(),
        bars,
        was: String::new(),
        cur: 0,
        input,
        current: String::new(),
        err: String::new(),
        rnd: Renderer::default(),
        loaded: false,
        saving: false,
        tabs_y: 2,
        list_y: 0,
        from: 0,
        tab_ends: Vec::new(),
        drag: String::new(),
    };
    for t in [ST_AGENT, ST_TOP] {
        st.tab = t;
        st.pad();
    }
    st.tab = ST_AGENT;
    m.open_sheet(Box::new(st));
    let k = kind.to_string();
    m.later(
        move || {
            let lay = sl::load();
            let mut current = String::new();
            if line
                && let Some(a) = reg().get(&k)
                && let Some(l) = a.status_liner()
                && let Ok(c) = l.status_line(&prof)
            {
                current = c;
            }
            (lay, current)
        },
        |m, (lay, current)| {
            if let Some(st) = m.sheet_as::<StatusSheet>() {
                st.adopt(lay, current);
            }
            Out::Draw
        },
    );
}

/// #statusline from the list: the same sheet, on the top bar, with the
/// selected agent (if any) for the other tabs' previews (openTopBar).
pub fn open_top_bar(m: &mut Model) -> Out {
    let a = m.focused().map(|i| m.agents[i].clone());
    let cfg = config(m);
    let (kind, prof, key, input) = match &a {
        Some(a) => (
            a.kind.0.clone(),
            Profile {
                kind: a.kind.clone(),
                name: a.job.account.clone(),
                dir: String::new(),
            },
            a.key.clone(),
            preview_input(&a.job.session_id, &a.job.cwd, &a.spend.model, a.spend.cost),
        ),
        None => {
            let acct = cfg.active_account();
            let kind = cfg.default_agent();
            (
                kind.clone(),
                Profile {
                    kind: rush_core::adapter::Kind::new(&kind),
                    name: acct.name,
                    dir: acct.config_dir,
                },
                String::new(),
                preview_input("", &m.launch_dir, "", 0.0),
            )
        }
    };
    open_status_line(m, &kind, prof, &key, input);
    if let Some(st) = m.sheet_as::<StatusSheet>() {
        st.tab = ST_TOP;
    }
    Out::Draw
}

/// A layout as saved: no empty lines at the end (trimmed).
pub fn trimmed(l: &Layout) -> Layout {
    let mut l = l.clone();
    while l.lines.len() > 1 && l.lines.last().is_some_and(Vec::is_empty) {
        l.lines.pop();
    }
    l
}

fn as_json(l: &Layout) -> String {
    serde_json::to_string(&trimmed(l)).unwrap_or_default()
}

impl StatusSheet {
    /// Takes in the agent's layout and settings once read (adopt).
    pub fn adopt(&mut self, lay: Layout, current: String) {
        self.loaded = true;
        self.agent_lay = lay;
        self.current = current;
        // Taken before your own line is folded in, so saving adopts it.
        self.was = as_json(&self.agent_lay);
        // A status line of your own isn't lost: it becomes a segment, kept
        // where it was, on the first line.
        if !self.current.is_empty()
            && !sl::ours(&self.current)
            && self.agent_lay.custom != self.current
        {
            self.agent_lay.custom = self.current.clone();
            if !self.agent_lay.shown("custom") {
                self.agent_lay.lines.insert(0, vec!["custom".into()]);
            }
        }
        let tab = self.tab;
        self.tab = ST_CLAUDE;
        self.pad();
        self.tab = tab;
    }

    /// How many tabs show: the agent's own only if it has a line (tabs).
    pub fn tabs(&self) -> usize {
        if self.line {
            STATUS_TABS
        } else {
            STATUS_TABS - 1
        }
    }

    /// The tabs showing, the last named for the agent (tabNames).
    pub fn tab_names(&self) -> Vec<String> {
        let mut names: Vec<String> = TAB_NAMES[..self.tabs()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if self.line {
            names[ST_CLAUDE] = harness_name(&self.kind);
        }
        names
    }

    /// Whether the tab showing is the agent's, not read yet (waiting).
    fn waiting(&self) -> bool {
        self.tab == ST_CLAUDE && !self.loaded
    }

    /// The layout of the tab showing (lay).
    pub fn lay(&mut self) -> &mut Layout {
        match self.tab {
            ST_TOP => &mut self.bars.top,
            ST_AGENT => &mut self.bars.agent,
            _ => &mut self.agent_lay,
        }
    }

    fn lay_ref(&self) -> &Layout {
        match self.tab {
            ST_TOP => &self.bars.top,
            ST_AGENT => &self.bars.agent,
            _ => &self.agent_lay,
        }
    }

    fn max_lines(&self) -> usize {
        match self.tab {
            ST_CLAUDE => sl::MAX_LINES,
            ST_TOP => sl::TOP_LINES,
            _ => sl::BAR_LINES,
        }
    }

    /// Every line the layout can have, empty ones included (pad).
    fn pad(&mut self) {
        let n = self.max_lines();
        let l = self.lay();
        l.lines.resize(n, Vec::new());
    }

    fn which(&self) -> usize {
        if self.tab == ST_TOP {
            BAR_TOP
        } else {
            BAR_AGENT
        }
    }

    /// The tab's segments (segs).
    pub fn segs(&self) -> Vec<SegInfo> {
        if self.tab == ST_CLAUDE {
            return sl::SEGMENTS
                .iter()
                .filter(|s| s.id != "custom" || !self.agent_lay.custom.is_empty())
                .map(|s| SegInfo {
                    id: s.id.into(),
                    name: s.name.into(),
                    about: s.about.into(),
                })
                .collect();
        }
        (header().segs)(self.which())
    }

    /// The segments line by line, then the ones not shown (slots).
    pub fn slots(&self) -> Vec<Slot> {
        let l = self.lay_ref();
        let mut out: Vec<Slot> = Vec::new();
        for (i, ln) in l.lines.iter().enumerate() {
            for id in ln {
                out.push(Slot {
                    line: Some(i),
                    id: id.clone(),
                });
            }
        }
        for s in self.segs() {
            if !l.shown(&s.id) {
                out.push(Slot {
                    line: None,
                    id: s.id,
                });
            }
        }
        out
    }

    /// Puts id at pos on line (taking it from wherever it was) and moves
    /// the cursor with it (place).
    pub fn place(&mut self, id: &str, line: Option<usize>, pos: usize) {
        let l = self.lay();
        for ln in &mut l.lines {
            ln.retain(|s| s != id);
        }
        if let Some(n) = line {
            let ln = &mut l.lines[n];
            ln.insert(pos.min(ln.len()), id.to_string());
        }
        if let Some(i) = self.slots().iter().position(|s| s.id == id) {
            self.cur = i;
        }
    }

    /// The list's rows top to bottom, as body draws them (rows).
    pub fn rows(&self) -> Vec<ListRow> {
        let l = self.lay_ref();
        let mut out = Vec::new();
        let mut i = 0;
        for (n, ln) in l.lines.iter().enumerate() {
            out.push(ListRow {
                line: Some(n),
                head: true,
                empty: false,
                slot: None,
            });
            if ln.is_empty() {
                out.push(ListRow {
                    line: Some(n),
                    head: false,
                    empty: true,
                    slot: None,
                });
            }
            for _ in ln {
                out.push(ListRow {
                    line: Some(n),
                    head: false,
                    empty: false,
                    slot: Some(i),
                });
                i += 1;
            }
        }
        out.push(ListRow {
            line: Some(l.lines.len()),
            head: true,
            empty: false,
            slot: None,
        });
        let n = self.slots().len();
        while i < n {
            out.push(ListRow {
                line: None,
                head: false,
                empty: false,
                slot: Some(i),
            });
            i += 1;
        }
        out
    }

    /// Keeps all three: rush's own lines at once, and the agent's, in its
    /// settings too, only when it was changed. The files are written off
    /// the UI; the sheet stays, saying so, until they are (save).
    fn save(&mut self, m: &mut Model) -> Out {
        let bars = Bars {
            top: trimmed(&self.bars.top),
            agent: trimmed(&self.bars.agent),
        };
        let mut msg = "status lines saved".to_string();
        let mut l = trimmed(&self.agent_lay);
        if !l.shown("custom") {
            l.custom.clear(); // let go of it only when it's taken out
        }
        let claude = self.loaded && self.line && as_json(&self.agent_lay) != self.was;
        if claude {
            msg.push_str(&format!(
                " · {} sessions for {} show theirs from their next redraw",
                harness_name(&self.kind),
                self.prof.name
            ));
        }
        let (kind, prof, b2) = (self.kind.clone(), self.prof.clone(), bars.clone());
        self.write(
            m,
            move || {
                sl::save_bars(&b2)?;
                if !claude {
                    return Ok(());
                }
                sl::save(&l)?;
                let a = reg().get(&kind).ok_or("its agent has gone")?;
                let liner = a.status_liner().ok_or("it has no status line")?;
                liner
                    .set_status_line(&prof, &sl::command())
                    .map_err(|e| format!("couldn't turn it on for {}: {e}", prof.name))
            },
            move |m| {
                (header().saved)(m, bars);
                m.flash(&msg);
            },
        )
    }

    /// Runs f off the UI; if it worked, closes the sheet and does done; if
    /// not, the sheet stays open and says why (write, landed).
    fn write(
        &mut self,
        m: &mut Model,
        f: impl FnOnce() -> Result<(), String> + Send + 'static,
        done: impl FnOnce(&mut Model) + Send + 'static,
    ) -> Out {
        type Done = Arc<Mutex<Option<Box<dyn FnOnce(&mut Model) + Send>>>>;
        self.err.clear();
        self.saving = true;
        let done: Done = Arc::new(Mutex::new(Some(Box::new(done))));
        // With no loop the write lands at once, while this sheet is still
        // the one the key came to: it's handed back here.
        let at_once = Arc::new(AtomicBool::new(true));
        let landed: Arc<Mutex<Option<Result<(), String>>>> = Arc::default();
        let (d2, a2, l2) = (done.clone(), at_once.clone(), landed.clone());
        m.later(f, move |m, r| {
            if a2.load(Ordering::Relaxed) {
                *l2.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                return Out::Draw;
            }
            match r {
                Err(e) => {
                    if let Some(st) = m.sheet_as::<StatusSheet>() {
                        st.saving = false;
                        st.err = e;
                    }
                }
                Ok(()) => {
                    if m.sheet_as::<StatusSheet>().is_some() {
                        m.sheet = None;
                    }
                    let f = d2.lock().unwrap_or_else(|e| e.into_inner()).take();
                    if let Some(f) = f {
                        f(m);
                    }
                }
            }
            Out::Draw
        });
        at_once.store(false, Ordering::Relaxed);
        let r = landed.lock().unwrap_or_else(|e| e.into_inner()).take();
        match r {
            Some(Ok(())) => {
                self.saving = false;
                let f = done.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(f) = f {
                    f(m);
                }
                Out::Close
            }
            Some(Err(e)) => {
                self.saving = false;
                self.err = e;
                Out::Draw
            }
            None => Out::Draw,
        }
    }

    /// d: turns the agent's line off, when it's rush's (turnOff).
    fn turn_off(&mut self, m: &mut Model) -> Out {
        if self.current.is_empty() {
            m.flash("there's no status line to turn off");
            return Out::Draw;
        }
        if !sl::ours(&self.current) {
            self.err = "that status line is your own command, not rush's: it's left alone".into();
            return Out::Draw;
        }
        let (kind, prof) = (self.kind.clone(), self.prof.clone());
        let name = prof.name.clone();
        self.write(
            m,
            move || {
                let a = reg().get(&kind).ok_or("its agent has gone")?;
                let liner = a.status_liner().ok_or("it has no status line")?;
                liner.set_status_line(&prof, "").map_err(|e| e.to_string())
            },
            move |m| m.flash(&format!("status line turned off for {name}")),
        )
    }

    /// One segment on its own, as the line would show it now (sample).
    fn sample(&self, m: &Model, id: &str) -> Line {
        if self.tab == ST_CLAUDE {
            let l = Layout {
                lines: vec![vec![id.to_string()]],
                plain: self.agent_lay.plain,
                custom: self.agent_lay.custom.clone(),
                ..Layout::default()
            };
            let s = self
                .rnd
                .render(&self.input, &l, &self.prof.dir, rush_state::Time::now());
            return from_ansi(s.split('\n').next().unwrap_or(""));
        }
        (header().sample)(m, self.which(), id)
    }

    fn change_tab(&mut self, t: usize) {
        if t != self.tab {
            self.tab = t;
            self.cur = 0;
            self.err.clear();
        }
    }
}

/// Lines set into a well of the terminal's own ground, cw wide and
/// indented to line up with the sheet's text (cutout).
fn cutout(lines: Vec<Line>, cw: usize) -> Vec<Line> {
    let ground: Color = crate::frame::ground::DARK.bg.color();
    let row = |l: Line| {
        let mut r = line([plain(" ")]);
        join(&mut r, l);
        let mut out = line([plain("  ")]);
        join(&mut out, on_ground(r, cw, ground));
        out
    };
    let mut out = vec![row(Line::new())];
    out.extend(lines.into_iter().map(row));
    out.push(row(Line::new()));
    out
}

impl Sheet for StatusSheet {
    /// The window's, on every tab, so the sheet doesn't change size as
    /// you go between them.
    fn width(&self, m: &Model) -> usize {
        m.w
    }

    fn mouse(&mut self, m: &mut Model, ev: MouseEv, x: i32, y: i32) -> Out {
        if self.saving {
            return Out::Idle;
        }
        match ev {
            MouseEv::WheelUp => return self.key(m, &Press::of("up").expect("up")),
            MouseEv::WheelDown => return self.key(m, &Press::of("down").expect("down")),
            MouseEv::Release => {
                self.drag.clear();
                return Out::Draw;
            }
            _ => {}
        }
        let (x, y) = (x.max(0) as usize, y.max(0) as usize);
        if ev == MouseEv::Press && y == self.tabs_y {
            if let Some(t) = self.tab_ends.iter().position(|&end| x < end) {
                self.change_tab(t);
            }
            return Out::Draw;
        }
        if self.waiting() {
            return Out::Idle;
        }
        let (rows, slots) = (self.rows(), self.slots());
        let at = (self.from + y) as isize - self.list_y as isize;
        if y < self.list_y || at < 0 || at as usize >= rows.len() {
            return Out::Idle;
        }
        let at = at as usize;
        let r = rows[at];
        if ev == MouseEv::Press {
            self.drag.clear();
            if let Some(i) = r.slot {
                self.cur = i;
                self.drag = slots[i].id.clone();
            }
            return Out::Draw;
        }
        if self.drag.is_empty() {
            return Out::Idle;
        }
        let Some(was) = rows
            .iter()
            .position(|r| r.slot.is_some_and(|i| slots[i].id == self.drag))
        else {
            return Out::Idle;
        };
        if was == at {
            return Out::Idle;
        }
        let from = slots[rows[was].slot.unwrap_or(0)].clone();
        let down = at > was;
        let n_lines = self.lay_ref().lines.len();
        let drag = self.drag.clone();
        if r.empty {
            self.place(&drag, r.line, 0);
        } else if r.head && down && r.line == Some(n_lines) {
            self.place(&drag, None, 0);
        } else if r.head && down {
            self.place(&drag, r.line, 0);
        } else if r.head && r.line == Some(0) {
            self.place(&drag, Some(0), 0);
        } else if r.head {
            let above = r.line.unwrap_or(1) - 1;
            let len = self.lay_ref().lines[above].len();
            self.place(&drag, Some(above), len);
        } else if let Some(i) = r.slot {
            let to = slots[i].clone();
            match to.line {
                None => {
                    if from.line.is_some() {
                        self.place(&drag, None, 0);
                    }
                }
                Some(tl) => {
                    let mut pos = self.lay_ref().lines[tl]
                        .iter()
                        .position(|s| *s == to.id)
                        .unwrap_or(0);
                    if down && from.line != to.line {
                        pos += 1; // below it: within a line, taking it out already makes room
                    }
                    self.place(&drag, Some(tl), pos);
                }
            }
        }
        Out::Draw
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if self.saving {
            return Out::Idle; // until it's written
        }
        let s = k.s.as_str();
        match s {
            "esc" | "ctrl+c" | "q" => return Out::Close,
            "]" => {
                let t = (self.tab + 1) % self.tabs();
                self.tab = t;
                self.cur = 0;
                self.err.clear();
                return Out::Draw;
            }
            "[" => {
                let t = (self.tab + self.tabs() - 1) % self.tabs();
                self.tab = t;
                self.cur = 0;
                self.err.clear();
                return Out::Draw;
            }
            "enter" | "ctrl+s" => return self.save(m),
            _ => {}
        }
        if self.waiting() {
            return Out::Idle;
        }
        let slots = self.slots();
        if slots.is_empty() {
            return Out::Idle;
        }
        self.cur = self.cur.min(slots.len() - 1);
        let sl = slots[self.cur].clone();
        let at = sl
            .line
            .and_then(|n| self.lay_ref().lines[n].iter().position(|x| *x == sl.id));
        let max_lines = self.max_lines();
        match s {
            "up" | "k" => self.cur = round_move(self.cur, -1, slots.len()),
            "down" | "j" => self.cur = round_move(self.cur, 1, slots.len()),
            "space" | "x" => {
                if sl.line.is_some() {
                    self.place(&sl.id, None, 0);
                    return Out::Draw;
                }
                // Onto the last line in use.
                let lines = &self.lay_ref().lines;
                let line = lines.iter().rposition(|l| !l.is_empty()).unwrap_or(0);
                let len = lines[line].len();
                self.place(&sl.id, Some(line), len);
            }
            "shift+up" | "alt+up" | "K" => match (sl.line, at) {
                (Some(n), Some(a)) if a > 0 => self.place(&sl.id, Some(n), a - 1),
                (Some(n), _) if n > 0 => {
                    let len = self.lay_ref().lines[n - 1].len();
                    self.place(&sl.id, Some(n - 1), len);
                }
                _ => {}
            },
            "shift+down" | "alt+down" | "J" => match (sl.line, at) {
                (Some(n), Some(a)) if a + 1 < self.lay_ref().lines[n].len() => {
                    self.place(&sl.id, Some(n), a + 1)
                }
                (Some(n), _) if n + 1 < max_lines => self.place(&sl.id, Some(n + 1), 0),
                _ => {}
            },
            "1" | "2" | "3" => {
                let line = (s.as_bytes()[0] - b'1') as usize;
                if line < max_lines {
                    let len = self.lay_ref().lines[line].len();
                    self.place(&sl.id, Some(line), len);
                }
            }
            "s" => {
                let l = self.lay();
                let i = sl::SEPS.iter().position(|x| *x == l.sep);
                l.sep = sl::SEPS[i.map_or(0, |i| (i + 1) % sl::SEPS.len())].into();
            }
            "c" => {
                if self.tab == ST_CLAUDE {
                    let l = self.lay();
                    l.plain = !l.plain;
                }
            }
            "r" => {
                match self.tab {
                    ST_TOP => *self.lay() = sl::default_top(),
                    ST_AGENT => *self.lay() = sl::default_agent(),
                    _ => {
                        let custom = self.agent_lay.custom.clone();
                        self.agent_lay = sl::default_layout();
                        self.agent_lay.custom = custom;
                    }
                }
                self.pad();
            }
            "d" => {
                if self.tab == ST_CLAUDE {
                    return self.turn_off(m);
                }
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let name = harness_name(&self.kind);
        let about = match self.tab {
            ST_AGENT => {
                "the top of an agent's Session: right of its name, and under it".to_string()
            }
            ST_TOP => "the top right of rush, about every agent at once".to_string(),
            _ => format!("what {name} shows under its prompt · {}", self.prof.name),
        };
        let names = self.tab_names();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut tabs = line([plain("  ")]);
        join(&mut tabs, sheet_tabs(&refs, self.tab));
        let mut out = vec![
            sheet_title("Status lines", &about, w),
            Line::new(),
            tabs,
            Line::new(),
        ];
        self.tabs_y = 2;
        self.tab_ends.clear();
        let mut end = 2;
        for n in &names {
            end += rush_core::text::width(n) + 5; // and the "  ·  " after it
            self.tab_ends.push(end - 2);
        }
        if self.waiting() {
            out.extend([
                line([sp(format!("  reading {name}'s settings…"), DIM)]),
                Line::new(),
                keys_fit(w, &[("[ ]", "tab"), ("esc", "cancel")]),
            ]);
            return out;
        }
        let mut dropped = HashSet::new();
        if self.tab != ST_CLAUDE {
            dropped = (header().dropped)(m, self.which());
        }
        // The preview is the real thing, set into the screen's own ground
        // as it will sit there.
        let cw = w.saturating_sub(4);
        if self.tab == ST_CLAUDE {
            let drawn = self.rnd.render(
                &self.input,
                self.lay_ref(),
                &self.prof.dir,
                rush_state::Time::now(),
            );
            out.push(line([sp(
                format!("  preview, under {name}'s prompt, with this session's numbers"),
                DIM,
            )]));
            let mut well = vec![
                line([sp("─".repeat(cw.saturating_sub(2)), FAINT)]),
                line([sp("❯ ", TEXT)]),
            ];
            if drawn.is_empty() {
                well.push(line([sp("(nothing to show: space adds a segment)", FAINT)]));
            } else {
                for ln in drawn.split('\n') {
                    well.push(trunc(from_ansi(ln), cw.saturating_sub(2)));
                }
            }
            out.extend(cutout(well, cw));
            out.push(Line::new());
            let note = if self.current.is_empty() {
                format!("  {name} has no status line for this account yet.")
            } else if sl::ours(&self.current) {
                format!("  {name} draws this now: saving updates it.")
            } else {
                "  Your own status line is kept, as the segment Your own line: move it, or take it out.".into()
            };
            out.push(line([sp(note, DIM)]));
        } else {
            let pw = cw.saturating_sub(2);
            let (where_, mut well) = if self.tab == ST_TOP {
                (
                    "  preview · rush's top, live on the real one too",
                    (header().preview)(m, BAR_TOP, "", pw),
                )
            } else {
                let mut wl = (header().preview)(m, BAR_AGENT, &self.agent, pw);
                wl.push(line([sp("  ⏺ the conversation goes on here…", FAINT)]));
                ("  preview · the agent header, live on the real one too", wl)
            };
            if well.is_empty() {
                well.push(Line::new());
            }
            out.push(line([sp(where_, DIM)]));
            out.extend(cutout(well, cw));
            out.push(Line::new());
            if dropped.is_empty() {
                out.push(line([sp(
                    "  When there isn't room, the segments last on a line go first, and it ends ⋯. What's wrong always shows.",
                    DIM,
                )]));
            } else {
                out.push(line([
                    sp("  ■", YELLOW),
                    sp(
                        " hasn't room at this width: it's left out, and the line ends ⋯. What's wrong always shows.",
                        DIM,
                    ),
                ]));
            }
        }
        out.push(Line::new());

        let slots = self.slots();
        self.cur = self.cur.min(slots.len().saturating_sub(1));
        let list_h = 4.max(h as isize - out.len() as isize - 5) as usize;
        let infos = self.segs();
        let mut list: Vec<Line> = Vec::new();
        let mut sel_at = 0;
        let lines_n = self.lay_ref().lines.len();
        let line_name = |n: usize| -> String {
            if n == lines_n {
                "Not shown".into()
            } else if self.tab == ST_AGENT && n == 0 {
                "Line 1 · right of the name".into()
            } else if self.tab == ST_AGENT {
                "Line 2 · under the name".into()
            } else {
                format!("Line {}", n + 1)
            }
        };
        for r in self.rows() {
            if r.head {
                list.push(line([spb(
                    format!("  {}", line_name(r.line.unwrap_or(lines_n))),
                    SUB,
                )]));
                continue;
            }
            if r.empty {
                list.push(line([sp(
                    format!(
                        "      empty · drag a segment here, or shift+↓ or {}",
                        r.line.unwrap_or(0) + 1
                    ),
                    FAINT,
                )]));
                continue;
            }
            let Some(i) = r.slot else {
                continue;
            };
            let slot = &slots[i];
            let seg = infos
                .iter()
                .find(|s| s.id == slot.id)
                .cloned()
                .unwrap_or_default();
            let no_room = slot.line.is_some() && dropped.contains(&slot.id);
            let mark = if slot.id == self.drag {
                sp("↕", ORANGE)
            } else if no_room {
                sp("■", YELLOW)
            } else if slot.line.is_none() {
                sp("□", DIM)
            } else {
                sp("■", GREEN)
            };
            if i == self.cur {
                sel_at = list.len();
            }
            let mut sample = self.sample(m, &slot.id);
            if sample.width() == 0 {
                sample = line([sp("(nothing right now)", FAINT)]);
            }
            let mut about = line([sp(seg.about.clone(), FAINT)]);
            if slot.id == "custom" && self.tab == ST_CLAUDE {
                about = line([sp(self.agent_lay.custom.clone(), FAINT)]);
            }
            if no_room {
                about = line([sp("no room ⋯ move it earlier, or widen the window", YELLOW)]);
            }
            let mut text = line([
                plain("  "),
                mark,
                plain(" "),
                sp(fit_str(&seg.name, 16), TEXT),
            ]);
            join(&mut text, fit(sample, 24));
            text.spans.push(plain("  "));
            join(&mut text, about);
            list.push(sheet_row(
                trunc(text, w.saturating_sub(3)),
                i == self.cur,
                w,
            ));
        }
        let (mut from, mut to) = window(list.len(), sel_at, list_h);
        // While dragging the list stays put unless the segment would leave
        // it, so what's under the pointer is what was.
        if !self.drag.is_empty()
            && self.from + list_h <= list.len()
            && sel_at >= self.from
            && sel_at < self.from + list_h
        {
            from = self.from;
            to = self.from + list_h;
        }
        self.list_y = out.len();
        self.from = from;
        out.extend(list[from..to].iter().cloned());
        out.push(Line::new());
        if self.saving {
            out.push(line([
                sp(format!("  {}", SPINNER[m.tick % SPINNER.len()]), ORANGE),
                sp(" saving…", DIM),
            ]));
        } else if !self.err.is_empty() {
            out.push(line([sp(format!("  {}", self.err), RED)]));
        }
        let l = self.lay_ref();
        let sep = l.sep.trim();
        let sep_name = format!("separator {}", if sep.is_empty() { "space" } else { sep });
        let lines = if self.max_lines() == 3 {
            "1 2 3"
        } else {
            "1 2"
        };
        // Most needed first: what doesn't fit is left off the end.
        let mut pairs: Vec<(&str, &str)> = vec![
            ("space", "show/hide"),
            ("shift+↑↓ or drag", "move"),
            ("[ ]", "tab"),
            ("enter", "save"),
            ("esc", "cancel"),
            (lines, "to line"),
            ("s", &sep_name),
        ];
        let colour = format!("colour {}", if l.plain { "off" } else { "on" });
        if self.tab == ST_CLAUDE {
            pairs.push(("c", &colour));
            pairs.push(("d", "off"));
        }
        pairs.push(("r", "reset"));
        out.push(keys_fit(w, &pairs));
        out
    }
}
