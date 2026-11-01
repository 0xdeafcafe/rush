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

use rush_core::agent::Agent;
use rush_core::text::Line;

use super::places::{Place, PlaceId};
use super::{Mouse, Out, Press};
use crate::model::Model;
use crate::session::SessionView;

/// A key handler: `Out::Pass` when the key isn't its.
pub type KeyHook = fn(&mut Model, &Press) -> Out;
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
    pub(crate) places: [Option<Box<dyn Place>>; 4],
    /// The Agents place's pages past its list ("Wall").
    pub(crate) agents_pages: Vec<Option<Box<dyn Place>>>,
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
