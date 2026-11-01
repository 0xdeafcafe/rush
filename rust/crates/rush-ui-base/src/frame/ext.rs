//! The frame's extension points: where screens ported later plug in
//! without editing the frame. Each group fills one module under
//! `screens/`, whose `register(&mut Ext)` the frame calls once as the
//! model is made (screens/mod.rs). See DESIGN.md "Screens".
//!
//! Hooks are plain `fn`s, run on the UI thread in the order registered;
//! state a group keeps between calls lives in [`Model::kept`]. Go has no
//! such seam: each of these is a call written into model.go/keys.go/
//! view.go, named beside each hook.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rush_core::agent::Agent;
use rush_core::text::Line;

use super::places::{Place, PlaceId};
use super::{Mouse, Out, Press};
use crate::changes::{FileChange, Ver};
use crate::model::Model;
use crate::predict::{HistHit, Hit};
use crate::sess::ConnCore;
use crate::session::SessionView;

/// A key handler: `Out::Pass` when the key isn't its.
pub type KeyHook = fn(&mut Model, &Press) -> Out;
/// A project's changes: lines added and removed, and how many files.
pub type ProjectCount = ((i64, i64), usize);

/// A session's end read ahead of time: its conversation, the transcript it
/// was read from and that file as it was just before.
pub type ReadEarly = fn(
    &rush_core::adapter::Registry,
    &Agent,
    rush_core::time::Ms,
) -> Option<(rush_convo::Session, String, Option<std::fs::Metadata>)>;

/// A mouse handler: `Out::Pass` when the event isn't its.
pub type MouseHook = fn(&mut Model, &Mouse) -> Out;
/// A paste handler: `Out::Pass` when the paste isn't its.
pub type PasteHook = fn(&mut Model, &str) -> Out;
/// A `#name arg` command typed into the Agents' box: given the arg.
pub type CommandFn = fn(&mut Model, &str) -> Out;
/// One more cell in the footer of the Agents dock's card for an agent
/// (view.go:1828 cardLines' cells); None to add nothing.
pub type CardCell = fn(&Model, &Agent) -> Option<Line>;
/// The top bar down the header's right (view.go:165 header, :256
/// topRights, bars_quiet.go:418 headerGauges): rush's own status line,
/// built in /statusline.
#[derive(Clone, Copy)]
pub struct TopBar {
    /// Its lines down the right, one in each of `rooms`' rows (0 the
    /// counts, 1 the folder, 2 the tabs, 3 clanker's feet), each at most
    /// its room wide; what a line left out goes in the drops.
    pub rights: fn(&Model, &[usize], &mut crate::header::Drops) -> Vec<Line>,
    /// While the top bar is the default, its gauges, a row each, when they
    /// fit in `w` (given today's spend); None draws the lines instead.
    pub gauges: fn(&Model, f64, usize) -> Option<Vec<Line>>,
    /// The usage segment shows: the second row is the folder alone.
    pub usage_shown: fn(&Model) -> bool,
    /// Keeps what the frame just drawn left out (barDropped).
    pub dropped: fn(&mut Model, crate::header::Drops),
    /// Agents' leftovers among the counts (view.go:189), None with none.
    pub orphans: fn(&Model) -> Option<Line>,
}
/// Rows drawn under the list or the open conversation, `w` wide: the
/// Agents box, a Session's message box and its cards (view.go listView's
/// prompt, the pane's dock). The body gives up the rows they take.
pub type Dock = fn(&mut Model, usize) -> Vec<Line>;
/// Rows at the very foot of the Agents list, `w` wide, at most a share of
/// its `body_h` rows, the list scrolling behind them: each with its key
/// for the pointer (Model::hover) and clicks (view.go:1159 streamDock).
pub type Foot = fn(&mut Model, usize, usize) -> Vec<(Line, String)>;
/// Draws over the finished frame, its rows given, before the sheet,
/// question and key help go on top (the quick ask, the command bar:
/// view.go:610 quickOver, overlayBar).
pub type Overlay = fn(&mut Model, &mut Vec<Line>);
/// What's typed in the Agents box when enter sends it (not a #command):
/// Pass when it isn't this screen's to send.
pub type SubmitHook = fn(&mut Model, &str) -> Out;
/// Called after each reading of the fleet is taken in (model.go's
/// snapMsg: notify, the menu bar, …).
pub type FleetHook = fn(&mut Model);
/// The bottom row's keys under the Agents list, `w` wide, when no flash
/// shows (view.go:2799, promptLines' hint): the first Some wins.
pub type Hint = fn(&Model, usize) -> Option<Line>;
/// Whether the Agents list shows an agent (model.go:2249, the filter):
/// it's left out when any says no.
pub type ListFilter = fn(&Model, &Agent) -> bool;
/// The list's column header drawn another way, `w` wide (view.go:1966,
/// the filter's): the first Some wins.
pub type ColumnHeader = fn(&Model, usize) -> Option<Line>;
/// What the list says when it shows no agent (view.go:1700): the first
/// Some wins.
pub type ListEmpty = fn(&Model) -> Option<String>;

/// The bottom row's keys, `w` wide, in place of the frame's own when Some
/// (a flash still goes over them): the Session's hint row (paneDock's
/// last row).
pub type StatusHook = fn(&Model, usize) -> Option<Line>;
/// The open Session's body, `w` wide (the frame indents it two cells)
/// and `rows` tall, in place of the conversation when Some: its other
/// views (Overview, Changes, …), and the conversation with its map
/// beside it (rushmode.go:2324 paneView's views).
pub type SessionBody = fn(&mut Model, usize, usize) -> Option<Vec<Line>>;
/// Goes over the open Session's header rows once drawn (`w` wide, five of
/// them): its tabs and the rule lit under the one showing, the context
/// it has used (rushmode.go:2721 paneHeader).
pub type SessionHead = fn(&mut Model, &mut [Line], usize);
/// What the header's second row says new sessions start on (view.go:196
/// startTag): the provider's glyph, the account, the profile.
pub type StartTag = fn(&Model) -> Line;

/// A `#name` command: what it does, for the command picker.
#[derive(Clone, Copy)]
pub struct Command {
    pub name: &'static str,
    pub about: &'static str,
    pub run: CommandFn,
}

/// Every extension point. Fields are filled by `register` functions;
/// places and Agents' pages through [`Ext::place`] and
/// [`Ext::agents_page`].
#[derive(Default)]
pub struct Ext {
    /// Every key, first, before any box over the screen has it (Go's
    /// quickKey, remapKey and barToggle at keys.go:34-55).
    pub keys: Vec<KeyHook>,
    /// The Agents list's keys, before its own (keys.go:273 listKey).
    pub list_keys: Vec<KeyHook>,
    /// The open conversation's keys, before its own (paneKey).
    pub session_keys: Vec<KeyHook>,
    /// Every mouse event, first.
    pub mouse: Vec<MouseHook>,
    /// Every paste, first; the Agents box takes the rest.
    pub paste: Vec<PasteHook>,
    /// `#name` commands (keys.go:874 command).
    pub commands: Vec<Command>,
    pub card_cells: Vec<CardCell>,
    /// The top bar; None draws today's spend and this rush.
    pub top_bar: Option<TopBar>,
    pub list_dock: Vec<Dock>,
    /// Rows at the foot of the list itself, above the agent's card and
    /// the docks: the first that has any is drawn (view.go:1130, Getting
    /// started while the list keeps 8 rows, else the feed's dock). The
    /// list gives up the rows they take.
    pub list_foot: Vec<Foot>,
    pub session_dock: Vec<Dock>,
    pub overlays: Vec<Overlay>,
    pub on_fleet: Vec<FleetHook>,
    pub status: Vec<StatusHook>,
    pub session_body: Vec<SessionBody>,
    pub session_head: Vec<SessionHead>,
    /// The header's start tag; None draws Model::account as it is.
    pub start_tag: Option<StartTag>,
    pub submit: Vec<SubmitHook>,
    pub list_hint: Vec<Hint>,
    pub list_filter: Vec<ListFilter>,
    pub column_header: Vec<ColumnHeader>,
    pub list_empty: Vec<ListEmpty>,
    /// A `#name` typed that no Command names, given the whole text: Pass
    /// when it isn't this screen's (plugins' #plugin.command).
    pub hash: Vec<SubmitHook>,
    /// The Agents box is on screen (a list_dock draws it from
    /// `Model::input`): text typed in the list goes into it, and the
    /// list's letter keys (j k g G h l q) are text, as in Go.
    pub agents_box: bool,
    /// The places past Agents, by PlaceId (Agents is the frame's own).
    pub places: [Option<Box<dyn Place>>; 4],
    /// The Agents place's pages past its list ("Wall").
    pub agents_pages: Vec<Option<Box<dyn Place>>>,
    /// What the frame asks of the screens by name (see [`Calls`]).
    pub calls: Calls,
    /// What the frame does at once for a screen (see [`Act`]).
    pub act: Option<ActFn>,
    /// What rush does by itself on Go's tick (chores.rs), in this order.
    pub chores: Vec<crate::chores::Chore>,
}

impl Ext {
    /// Puts p in place id, replacing what's there (the frame's
    /// placeholder until a group registers the real one). Agents can't be
    /// replaced: add pages to it with [`Ext::agents_page`].
    pub fn place(&mut self, id: PlaceId, p: Box<dyn Place>) {
        if id != PlaceId::Agents {
            self.places[id as usize] = Some(p);
        }
    }

    /// Adds a page to Agents after its list, or replaces the page of the
    /// same name.
    pub fn agents_page(&mut self, p: Box<dyn Place>) {
        let name = p.name().to_string();
        if let Some(slot) = self
            .agents_pages
            .iter_mut()
            .find(|s| s.as_ref().is_some_and(|q| q.name() == name))
        {
            *slot = Some(p);
            return;
        }
        self.agents_pages.push(Some(p));
    }

    /// Adds a `#name` command, replacing one of the same name.
    pub fn command(&mut self, name: &'static str, about: &'static str, run: CommandFn) {
        self.commands.retain(|c| c.name != name);
        self.commands.push(Command { name, about, run });
    }
}

/// The list's rows (lw wide) and the pane's (pw wide) side by side, n of
/// them, the list's faded when fade_list: (m, left, lw, pane, pw, n,
/// fade_list).
pub type JoinSplit = fn(&Model, &[Line], usize, &[Line], usize, usize, bool) -> Vec<Line>;
/// Draws the Session w wide and h tall: (m, w, h).
pub type DrawPane<'a> = &'a mut dyn FnMut(&mut Model, usize, usize) -> Vec<Line>;
/// The Session's pane at (x, y), pw wide and rows tall, with what's
/// round it: (m, pw, rows, x, y, draw).
pub type RoundPane = fn(&mut Model, usize, usize, usize, usize, DrawPane) -> Vec<Line>;

/// Runs an [`Act`]: filled by screens/mod.rs's register.
pub type ActFn = fn(&mut Model, Act);

/// Something the frame has a screen do, at once on the UI thread (never
/// queued: a queued act could land a frame later). [`Model::act`] runs
/// it; the one match is screens/mod.rs's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// The key guide (cmdbar/keyhelp.rs open).
    OpenKeyHelp,
    /// A room in the pane, by its row's key or a member's
    /// (social/rooms.rs open_room).
    OpenRoom(String),
    /// Projects at a project's folder (projects_machine pick_in_projects).
    PickInProjects(String),
    /// The hosts are watched again after an update (settings/updates.rs
    /// watch_hosts); with rebuilt_said, an installed one counts as said.
    WatchHosts { rebuilt_said: bool },
    /// The limits of every account asked for again, and the logins looked
    /// for (accounts logins::fetch_usage and find_logins, bars
    /// fetch::fetch_now).
    RefetchUsage,
    /// An open session's memory read again, by its key
    /// (session_core/memory.rs forget).
    ForgetMemory(String),
    /// An agent put away, or brought back from there, by its key
    /// (cmdbar/stops.rs toggle_done).
    ToggleDone(String),
    /// Keybindings put in force again, plugins' commands as they are now
    /// (settings/keys.rs rebuild).
    RebuildKeys,
    /// A run clicked in the views panel: a workflow's agents, a run picked
    /// in the subagents view, or a task picked (session_core/subview.rs).
    OpenRun { key: String, r: String },
    /// The agent sent text as if typed, in a session rush runs
    /// (session_core/keys.rs send_as).
    SendAs { key: String, text: String },
    /// Every provider's limits read again, off the UI thread, without the
    /// logins looked for (bars/fetch.rs fetch_now).
    FetchNow,
    /// Claude Code's own screen for a session, with the terminal handed to
    /// it (session_cards/claudescreen.rs open_screen).
    OpenClaudeScreen { key: String, screen: String },
}

impl Model {
    /// Has a screen do a, at once.
    pub fn act(&mut self, a: Act) {
        let f = self.ext.act.expect("act registered by screens");
        f(self, a);
    }
}

/// Declares [`Calls`]: each slot a `fn` a screen fills at register, and
/// an accessor that panics loudly, naming who fills it, when it wasn't.
macro_rules! calls {
    ($($(#[$doc:meta])* $name:ident($by:literal): $ty:ty,)*) => {
        /// What the frame asks the screens for and has them do by name,
        /// each filled once by screens/mod.rs's register: the calls the
        /// frame made into a screen's module.
        #[derive(Default)]
        pub struct Calls {
            $($(#[$doc])* pub $name: Option<$ty>,)*
        }

        impl Calls {
            $($(#[$doc])* pub fn $name(&self) -> $ty {
                self.$name
                    .expect(concat!(stringify!($name), " registered by ", $by))
            })*

            /// The slots no screen filled.
            pub fn missing(&self) -> Vec<&'static str> {
                let mut out = Vec::new();
                $(if self.$name.is_none() {
                    out.push(stringify!($name));
                })*
                out
            }
        }
    };
}

calls! {
    /// The command bar is open (cmdbar/bar.rs is_open).
    bar_open("cmdbar"): fn(&Model) -> bool,
    /// The key guide is open (cmdbar/keyhelp.rs is_open).
    key_help_open("cmdbar"): fn(&Model) -> bool,
    /// `#` and a command's name, not a heading or an issue
    /// (cmdbar/fleetslash.rs is_hash_cmd).
    is_hash_cmd("cmdbar"): fn(&str) -> bool,
    /// /stop typed the old way, run as #stop (fleetslash.rs legacy_command).
    legacy_command("cmdbar"): fn(&mut Model, &str) -> Option<Out>,
    /// The command a name stands for (fleetslash.rs alias_of).
    alias_of("cmdbar"): fn(&str) -> &str,
    /// A grid tile's box takes the key (list_extras/grid.rs key).
    grid_key("list_extras"): fn(&mut Model, &Press) -> Option<Out>,
    /// The grid's tiles and edge take the mouse (grid.rs mouse).
    grid_mouse("list_extras"): fn(&mut Model, &Mouse) -> Out,
    /// The pinned agents' keys, the grid's (grid.rs grid().keys).
    grid_keys("list_extras"): fn(&mut Model) -> Vec<String>,
    /// A pinned agent's Session kept for its tile (grid.rs park).
    grid_park("list_extras"): fn(&mut Model, SessionView),
    /// A tile's Session, taken to open (grid.rs adopt).
    grid_adopt("list_extras"): fn(&mut Model, &str) -> Option<SessionView>,
    /// The opened Session gives up the keys (list_extras/split.rs
    /// leave_pane).
    leave_pane("list_extras"): fn(&mut Model),
    /// The Session beside the list is there for now (split.rs preview).
    preview("list_extras"): fn(&Model) -> bool,
    /// The Session beside the list for now goes (split.rs leave_chat).
    leave_chat("list_extras"): fn(&mut Model),
    /// What's being renamed, if a name is (list_extras/rename.rs renaming).
    renaming("list_extras"): fn(&Model) -> Option<&str>,
    /// An agent's subagent rows tucked away or brought back
    /// (list_extras/subs.rs toggle).
    sub_toggle("list_extras"): fn(&mut Model, &str),
    /// How many subagent rows an agent with n has (subs.rs rows).
    sub_rows("list_extras"): fn(&Model, &str, usize) -> usize,
    /// An agent's subagent row i (subs.rs row_of).
    sub_row_of("list_extras"): fn(&Model, &Agent, usize, usize, usize) -> Line,
    /// The list moved by d: what's read ahead follows
    /// (list_extras/predict.rs moved).
    predict_moved("list_extras"): fn(&mut Model, isize),
    /// The strip over a mirrored screen (list_extras/live.rs summary_strip).
    summary_strip("list_extras"): fn(&mut Model, usize) -> Option<Line>,
    /// A harness's own spinner at a tick (list_extras/wall.rs spin).
    spin("list_extras"): fn(&str, usize) -> &'static str,
    /// An #intervene waiting for its chat, run when ready
    /// (session_cards/discuss.rs intervene_ready).
    intervene_ready("session_cards"): fn(&mut Model) -> bool,
    /// A memory file with unsaved edits (session_core/memedit.rs
    /// dirty_path).
    dirty_path("session_core"): fn(&mut Model) -> Option<String>,
    /// The spawn steps read with a conversation, kept for its subagents
    /// (session_core/subs.rs prime_spawns).
    prime_spawns("session_core"): fn(&mut Model, &str, crate::worker::SpawnSteps),
    /// The screen's size changed (settings resized).
    resized("settings"): fn(&mut Model),
    /// How far a second line fades (settings/interface.rs sub_line_fade).
    sub_line_fade("settings"): fn(&str) -> f64,
    /// A room's Session header: its agent and line (social/rooms.rs
    /// pane_head).
    pane_head("social"): fn(&Model, &str) -> Option<(Agent, Line)>,
    /// A room's row (rooms.rs room_line).
    room_line("social"): fn(&rush_room::room::Summary, usize, bool, usize) -> Line,
    /// A room member's row (rooms.rs room_member_line).
    room_member_line("social"):
        fn(Option<&rush_room::room::Summary>, &Agent, usize, bool, usize) -> Line,
    /// What the runaway guard says on an agent's row
    /// (accounts/guard.rs row_note).
    row_note("accounts"): fn(&Model, &Agent) -> Option<String>,
    /// What the runaway guard holds, in the header (guard.rs header_line).
    guard_header("accounts"): fn(&Model) -> Option<Line>,
    /// The limits have been fetched once (bars/fetch.rs fetched_once).
    limits_fetched("bars"): fn(&Model) -> bool,
    /// The machine's processes have been read (projects_machine
    /// machine_read).
    machine_read("projects_machine"): fn(&Model) -> bool,
    /// Always asked on an agent: hide, stop, restart or delete
    /// (cmdbar/stops.rs ask_close).
    ask_close("cmdbar"): fn(&mut Model, Option<Agent>) -> Out,
    /// An agent's pull request opened, or that there's none
    /// (list_extras/listkeys.rs open_pr).
    open_pr("list_extras"): fn(&mut Model, Option<&Agent>) -> Out,
    /// The sessions worth reading, by index (list_extras/work.rs
    /// work_agents).
    work_agents("list_extras"): fn(&Model) -> Vec<usize>,
    /// How far through its plan an agent is: (tasks planned, tasks ticked)
    /// from what the work page read (list_extras/work.rs WorkState).
    work_plan("list_extras"): fn(&Model, &str) -> (i64, i64),
    /// What the next session starts as in a folder (accounts/setup.rs
    /// next_start).
    next_start("accounts"): fn(&Model, &str) -> crate::accounts::StartOver,
    /// The profile of yours a folder starts with (accounts/rows.rs
    /// start_profile).
    start_profile("accounts"): fn(&Model, &str) -> rush_state::Profile,
    /// What plugins put on an agent's row (plugin_ui/hooks.rs
    /// plugin_status).
    plugin_status("plugin_ui"): fn(&Model, &str) -> Line,
    /// An agent's own view, said short (plugin_ui/views summary_of,
    /// view_summary_short).
    view_summary("plugin_ui"): fn(&Model, &Agent) -> Option<Line>,
    /// Something happened in a box, told to plugins (plugin_ui/hooks.rs
    /// emit_box).
    emit_box("plugin_ui"): fn(&mut Model, &str, &str, &str),
    /// A box cleared, told to plugins (hooks.rs cleared).
    cleared("plugin_ui"): fn(&mut Model, &str, &str),
    /// The Session beside the list follows the agent picked
    /// (list_extras/split.rs follow).
    pane_follow("list_extras"): fn(&mut Model),
    /// The picked agent's own screen, mirrored, w wide and rows tall
    /// (split.rs mirror).
    pane_mirror("list_extras"): fn(&mut Model, usize, usize) -> Option<Vec<Line>>,
    /// What the pane says while there's no Session to draw in it
    /// (split.rs placeholder).
    pane_placeholder("list_extras"): fn(&mut Model, usize, usize) -> Option<Vec<Line>>,
    /// The list's rows (lw wide) and the pane's (pw wide) side by side, n
    /// of them, the list's faded when fade_list (split.rs join_rows, with
    /// pane_kind and divider_lit).
    join_split("list_extras"): JoinSplit,
    /// The Session's pane at (x, y), pw wide and rows tall, with the
    /// grid's tiles round it, the Session drawn by the last argument
    /// (list_extras/grid.rs round_pane).
    round_pane("list_extras"): RoundPane,
    /// The terminal lost or got back focus (split.rs pointer_back).
    pointer_back("list_extras"): fn(&mut Model),
    /// The list's edge and the mouse: Ok when the edge took it, else
    /// whether its lighting changed (split.rs edge_mouse).
    edge_mouse("list_extras"): fn(&mut Model, &Mouse) -> Result<Out, bool>,
    /// The pointer at column x gives that side the keys as it crosses
    /// (split.rs pointer_at).
    pointer_at("list_extras"): fn(&mut Model, usize, bool) -> bool,
    /// A click at column x gives that side the keys (split.rs focus_at).
    focus_at("list_extras"): fn(&mut Model, usize) -> bool,
    /// Agents are on screen for a peek from the Session alone
    /// (list_extras/peek.rs peeking).
    peeking("list_extras"): fn(&Model) -> bool,
    /// enter while peeking opens agent i (peek.rs open_peeked).
    open_peeked("list_extras"): fn(&mut Model, usize),
    /// esc while peeking goes back to the Session left; false with no
    /// peek (peek.rs back).
    peek_back("list_extras"): fn(&mut Model) -> bool,
    /// An agent's transcript's end, read ahead (list_extras/predict.rs
    /// take).
    predict_take("list_extras"): fn(&mut Model, &str, &Path) -> Option<Hit>,
    /// An agent's history, read ahead (predict.rs take_history).
    predict_take_history("list_extras"): fn(&mut Model, &str) -> Option<HistHit>,
    /// The Session kept as the list moved off it (predict.rs Predict's
    /// left).
    predict_left("list_extras"): fn(&mut Kept) -> Option<&mut SessionView>,
    /// The filter found something said in a's conversation
    /// (list_extras/filter.rs has_match).
    filter_match("list_extras"): fn(&Model, &Agent) -> bool,
    /// The match the filter found in a's conversation, lit, w wide
    /// (filter.rs snippet).
    filter_snippet("list_extras"): fn(&Model, &Agent, usize) -> Option<Line>,
    /// The open session's edits as the worker that read it worked them
    /// out (session_core/changes.rs prime).
    prime_changes("session_core"): fn(&mut Model, Ver, &str, Arc<Vec<FileChange>>),
    /// Session key's side every screen reads, made on first use
    /// (session_core conn's base).
    conn("session_core"): for<'a> fn(&'a mut Model, &str) -> &'a mut ConnCore,
    /// Session key's side, when it has one, to read.
    peek_conn("session_core"): for<'a> fn(&'a Model, &str) -> Option<&'a ConnCore>,
    /// Session key's side, when it has one, to change: none is made.
    conn_if("session_core"): for<'a> fn(&'a mut Model, &str) -> Option<&'a mut ConnCore>,
    /// Hosts that can start sessions were set: the screens start through
    /// them (session_core/start.rs install).
    start_install("session_core"): fn(&mut Model),
    /// The actions plugins offer a key for (plugin_ui/hooks.rs
    /// plugin_key_actions).
    plugin_key_actions("plugin_ui"): fn(&Model) -> Vec<rush_keymap::Action>,
    /// The keys plugins suggest for their actions (hooks.rs plugin_keys).
    plugin_keys("plugin_ui"): fn(&Model) -> std::collections::BTreeMap<String, Vec<String>>,
    /// The group-by modes on offer: rush's own and one for each plugin that
    /// arranges the list (plugin_ui/sidebar.rs group_modes).
    group_modes("plugin_ui"): fn(&Model) -> Vec<String>,
    /// How the guard sees an account's pace, in words; none while the pace
    /// guard is off (accounts/guard.rs pace_words).
    pace_words("accounts"): fn(&Model, &str, &str) -> Option<Line>,
    /// An account row's pace past the warning, as a chip (guard.rs
    /// pace_chip).
    pace_chip("accounts"): fn(&Model, &str, &str) -> Line,
    /// Opens a file in $EDITOR, then runs done (session_core/memedit.rs
    /// edit_then).
    edit_then("session_core"): fn(&mut Model, &str, super::outside::Done) -> Out,
    /// #reload (social/services.rs reload).
    reload("social"): fn(&mut Model, bool) -> Out,
    /// Whether the feed follows is now config's (social/stream.rs
    /// feed_follows).
    feed_follows("social"): fn(&mut Model, bool),
    /// The id of the one session a hosted view shows (hosted.rs id).
    hosted_id("hosted"): fn(&Model) -> Option<&str>,
    /// A hosted view showing its one session, the list hidden (hosted.rs
    /// alone).
    hosted_alone("hosted"): fn(&Model) -> bool,
    /// Every agent, a hosted view's hidden ones included (hosted.rs
    /// all_agents).
    all_agents("hosted"): fn(&Model) -> &[Agent],
    /// A reading of the list as a hosted view takes it (hosted.rs snap).
    hosted_snap("hosted"): fn(&mut Model),
    /// A hosted view stays on its session; whether it opened it
    /// (hosted.rs pin).
    hosted_pin("hosted"): fn(&mut Model) -> bool,
    /// A hosted view's one session is open with no list to go back to
    /// (hosted.rs keeps_open).
    hosted_keeps_open("hosted"): fn(&Model) -> bool,
    /// The keys come off a hosted view's box (hosted.rs set_away).
    hosted_set_away("hosted"): fn(&mut Model),
    /// The agent row after a key in its section, or the one before ("" when
    /// alone): list_extras/mod.rs neighbour.
    neighbour("list_extras"): fn(&Model, &str) -> String,
    /// The ways a session can start here now (accounts/setup.rs
    /// start_routes).
    start_routes("accounts"): fn(&Model) -> Vec<crate::accounts::Route>,
    /// Agent k as Settings says it starts (accounts/setup.rs
    /// start_defaults).
    start_defaults("accounts"): fn(&Model, &str) -> crate::accounts::StartOver,
    /// What agent k can start on (accounts/setup.rs models).
    models("accounts"): fn(&Model, &str) -> Vec<rush_core::adapter::Choice>,
    /// Agent k's models read from its home off the UI thread
    /// (accounts/startsheet.rs load_models).
    load_models("accounts"): fn(&mut Model, &str),
    /// A new session in a folder as a start says, with what Settings gives
    /// its agent for the rest (session_core/start.rs config_as).
    config_as("session_core"): fn(&Model, &crate::accounts::StartOver, &str) -> crate::host::Start,
    /// The rooms as just read: each finished room on a chat hands its
    /// verdict back (session_cards/discuss.rs verdicts_back).
    verdicts_back("session_cards"): fn(&mut Model, Vec<rush_room::room::Summary>),
    /// The view a session shows (session_core/views.rs view_name).
    view_name("session_core"): fn(&Model, &str) -> &'static str,
    /// Whether a session has runs or tasks the dock would show
    /// (session_core/running.rs any).
    running_any("session_core"): fn(&mut Model, &str) -> bool,
    /// Clears the conversation's lit row under the pointer; whether one
    /// was lit (session_core views ViewState hover_ref).
    clear_hover("session_core"): fn(&mut Model, &str) -> bool,
    /// What's in a session's box sent, or held for plugins to answer
    /// first (session_core/keys.rs send).
    send_box("session_core"): fn(&mut Model, &str, bool) -> Out,
    /// What a session is running as, to switch it (bars/agent.rs
    /// session_start).
    session_start("bars"): fn(&Model, &str) -> Option<crate::accounts::StartOver>,
    /// The pointer is over the context figure in a session's header
    /// (session_core views.rs, hostConn.hoverCtx).
    hover_context("session_core"): fn(&Model, &str) -> bool,
    /// Where a session's header drew its context figure, for a click on it
    /// (session_core views.rs ViewState.context).
    set_context_at("session_core"): fn(&mut Model, &str, (usize, usize)),
    /// The lines and files the open session's project counts, read again
    /// as a step changes (session_core/changes.rs project_count).
    project_count("session_core"): fn(&mut Model) -> ProjectCount,
    /// The same for a session read from a shared Model (changes.rs
    /// project_count_of).
    project_count_of("session_core"):
        fn(&Model, &rush_convo::Session) -> ProjectCount,
    /// The used part of the open session's context window, on the name's
    /// row (session_core/views.rs header_context).
    header_context("session_core"): fn(&Model, bool) -> Option<Line>,
    /// A /command run as typed in a session (session_core/rushcmd.rs run).
    rush_cmd("session_core"): fn(&mut Model, &str, &str) -> Out,
    /// #permission for a session (session_cards/permissions.rs
    /// permission_command): the arg, and whether it's yolo.
    permission_command("session_cards"): fn(&mut Model, &str, &str, bool) -> Out,
    /// The account to switch to when the one in use is nearly out, and
    /// why (list_extras/plan.rs next_account).
    next_account("list_extras"): crate::accounts::NextAccount,
    /// One of an agent's own screens in place of rush's sheet; Pass for
    /// the ones it has none of (session_cards/claudescreen.rs rush_screen).
    rush_screen("session_cards"): fn(&mut Model, &str, &str) -> Out,
    /// #model and #effort with the agent's session open: its /model and
    /// /effort; None when it isn't (session_core/rushcmd.rs hash_arg).
    hash_arg("session_core"): fn(&mut Model, &str, &str) -> Option<Out>,
    /// ctrl+b, #go and the Wall's: tell a stopped agent to go on, or say
    /// why it can't (cmdbar/hashcmds.rs keep_going).
    keep_going("cmdbar"): fn(&mut Model, &Agent) -> Out,
    /// The connection and screen of the Session shown follow the pane
    /// (session_core/live.rs sync).
    sync_host("session_core"): fn(&mut Model),
    /// Lets go of session key's connection (session_core/live.rs
    /// drop_link).
    drop_link("session_core"): fn(&mut Model, &str),
    /// A session's end read from disk ahead of time (session_core/live.rs
    /// read_early): the conversation, its transcript and that file as it was.
    read_early("session_core"): ReadEarly,
    /// ctrl+l with nothing to move: the folder new sessions start in
    /// (session_core/moveto.rs open_dir_picker).
    open_dir_picker("session_core"): fn(&mut Model) -> Out,
    /// Asks what enter should do for an agent (session_cards/enter.rs
    /// ask_enter).
    ask_enter("session_cards"): fn(&mut Model, &str),
    /// A click on an image in a draft opens it (session_cards/message.rs
    /// draft_image): the marker clicked and the box's images.
    draft_image("session_cards"): fn(&mut Model, &str, &crate::imagerefs::ImageRefs) -> Out,
    /// A click on the quick ask or the btw box floating over the list's
    /// edge is theirs (session_cards quickask::quick_mouse, btw::btw_mouse).
    float_mouse("session_cards"): fn(&mut Model, &Mouse) -> Out,
    /// The image on the clipboard as a file, read off the UI thread
    /// (session_cards/clipimage.rs clip_image).
    clip_image("session_cards"): fn() -> Result<Option<String>, String>,
}

/// State screens keep between calls, one value per type, made on first
/// use: a group's own struct, without a field on Model for it.
#[derive(Default)]
pub struct Kept(HashMap<TypeId, Box<dyn Any>>);

impl Kept {
    pub fn get<T: Default + 'static>(&mut self) -> &mut T {
        self.0
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut()
            .expect("kept by its type")
    }

    pub fn peek<T: 'static>(&self) -> Option<&T> {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref())
    }
}
