//! The Session: an open conversation's message box, sending, the queue,
//! interrupting and the live state of a session rush hosts. The
//! milestone-1 read-only view (session.rs) stays the conversation; this
//! group adds what's under it and the keys that type, send and stop.
//! After Go's internal/ui/rushmode.go (hostConn :1452, syncHost :1894,
//! onHostLines :2083, paneDock's sending/queue/box/hint :3209-3687,
//! paneKey :3792, sendPane :4440, sendOffline :5001), sending.go,
//! sendmode.go, localqueue.go, subqueue.go, sessionviews.go (the queue's
//! keys :23-280), hosted.go (canInterrupt :196, stopTurn :202) and
//! stash.go (wipeBox :173, boxVert :225).
//!
//! Its views (sessionviews.go's tabs, rushmode.go's paneView and
//! paneHeader tabs), the dragged-over text (textsel.go), the map
//! (minimap.go), the pointer over the pane and header (pane_pointer.go,
//! header_pointer.go, header_context.go) and #expand/#collapse
//! (history_folds.go) are views.rs and the modules it names. rail.go is
//! the list's (list.rs).
//!
//! What the session sends goes through [`host::Hosts`]: the binary sets
//! rush-host's with [`host::set_hosts`], which also lets the screens start
//! sessions ([`start`]); without it [`host::NoHosts`] says rush can't send.

pub mod actions;
pub mod cards_host;
pub mod carry;
pub mod changes;
pub mod context;
pub mod conversation;
pub mod dock;
pub mod exchange;
pub mod finish;
pub mod history;
pub mod host;
pub mod jobkeys;
pub mod jobprocs;
pub mod keys;
pub mod live;
pub mod memedit;
pub mod memory;
pub mod minimap;
pub mod moveto;
pub mod overview;
pub mod paint;
pub mod queue;
pub mod relay;
pub mod running;
pub mod rushcmd;
pub mod rushmove;
pub mod scripts;
pub mod sending;
pub mod sendmode;
pub mod side;
pub mod slash;
pub mod spawned;
pub mod start;
pub mod subname;
pub mod subqueue;
pub mod subs;
pub mod subview;
pub mod switch;
pub mod tasks;
pub mod textsel;
pub mod told;
pub mod turnwants;
pub mod unused;
pub mod views;
pub mod workflow;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_cards;
#[cfg(test)]
mod tests_clicks;
#[cfg(test)]
mod tests_cold;
#[cfg(test)]
mod tests_drops;
#[cfg(test)]
mod tests_grid;
#[cfg(test)]
mod tests_header;
#[cfg(test)]
mod tests_jobs;
#[cfg(test)]
mod tests_move;
#[cfg(test)]
mod tests_next;
#[cfg(test)]
mod tests_pastes;
#[cfg(test)]
mod tests_plugins;
#[cfg(test)]
mod tests_scripts;
#[cfg(test)]
mod tests_slash;
#[cfg(test)]
mod tests_split;
#[cfg(test)]
mod tests_subs;
#[cfg(test)]
mod tests_subview;
#[cfg(test)]
mod tests_switch;
#[cfg(test)]
mod tests_tasks;
#[cfg(test)]
mod tests_undo;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;

use crate::frame::inputbox::InputBox;
use crate::frame::{Editor, Ext};
use crate::model::Model;
use host::{Hosts, Info, Link};
use queue::LocalQueue;
use sending::Outbox;
use sendmode::SendMode;
use subqueue::SubNote;

/// Adds the Session's keys, dock, paste, mouse and live state to x.
pub fn register(x: &mut Ext) {
    // ctrl+b backgrounds what the turn waits on before /btw has it
    // (paneKey :4003).
    x.session_keys.insert(0, jobkeys::ctrl_b);
    // The box under a run beside the subagents list, while it has the
    // keys, has them all.
    x.session_keys.insert(0, subview::box_key);
    // A memory file being edited has every key first.
    x.session_keys.push(memedit::key);
    x.session_keys.push(memedit::draft_key);
    x.list_keys.push(memedit::list_draft_key);
    x.session_keys.push(keys::esc_twice);
    x.session_keys.push(textsel::key);
    x.session_keys.push(changes::full_key);
    x.session_keys.push(subview::key_first);
    x.session_keys.push(running::bg_key);
    x.session_keys.push(jobkeys::key);
    x.session_keys.push(exchange::key);
    // The picker over the box has ↑↓ tab enter while a command is typed
    // (paneKey :4079 slashKey).
    x.session_keys.push(slash::key);
    x.session_keys.push(keys::key);
    x.session_dock.push(dock::dock);
    x.session_body.push(views::body);
    x.session_head.push(views::head);
    x.paste.push(subview::paste);
    x.paste.push(keys::paste);
    // The views beside the conversation have the pointer over them first.
    x.mouse
        .push(crate::screens::plugin_ui::views::beside::mouse);
    x.mouse.push(subview::mouse);
    x.mouse.push(keys::mouse);
    x.mouse.push(conversation::mouse);
    x.mouse.push(textsel::mouse);
    x.mouse.push(views::mouse);
    history::register(x);
    x.overlays.push(dock::locate);
    x.overlays.push(conversation::over);
    x.on_fleet.push(live::on_fleet);
    x.on_fleet.push(start::pick_started);
    x.on_fleet.push(rushmove::move_pending);
    moveto::register(x);
    x.on_fleet.push(turnwants::on_fleet);
    x.on_fleet.push(finish::on_fleet);
    x.status.push(dock::status);
}

/// The views' keys (a row picked, ↑↓ through the conversation), after
/// every other group's session keys: the cards take ↑ from an empty box
/// first, as paneKey does.
pub fn register_last(x: &mut Ext) {
    x.session_keys.push(subview::key);
    x.session_keys.push(views::key);
    // A task typed in the box starts a session, once nothing before has
    // taken it (keys.go submit's last step).
    x.submit.push(start::submit);
}

/// What the Session keeps between frames, for every session opened.
#[derive(Default)]
pub struct Core {
    /// Where messages go (NoHosts until set).
    pub hosts: Option<Arc<dyn Hosts>>,
    /// Each session's box, outbox and connection, by its key.
    pub conns: HashMap<String, Conn>,
    /// Messages waiting for sessions rush doesn't run, by key, and for
    /// watched subagents, by subqueue::key.
    pub local_q: HashMap<String, LocalQueue>,
    /// How enter sends to each session while it works, once picked.
    pub send_modes: HashMap<String, SendMode>,
    /// Messages meant for subagents their conversation doesn't show.
    pub sub_notes: HashMap<String, Vec<SubNote>>,
    /// The bottom row's keys, as the dock last worked them out.
    pub hint: Option<rush_core::text::Line>,
    /// The session whose link is open.
    pub linked: String,
    /// No map beside the conversation, whatever the config says (tests
    /// drawn as Go's, with HideMinimap).
    pub hide_map: bool,
    /// Bumped for each link opened: what an older one says is dropped.
    pub link_gen: u64,
    /// The session just started, picked once a reading of the list has it
    /// (selectOnLoad).
    pub select_on_load: String,
    /// When esc last met an empty box in esc_key's Session: esc again
    /// within a second asks to close it (escAt, escKey).
    pub esc_at: Ms,
    pub esc_key: String,
    /// What a rewind or a compaction put back in each session's box once
    /// its host is reached again, by key (m.rewound).
    pub rewound: HashMap<String, String>,
    /// The stopped rush session whose saved info was read for the
    /// session showing: read once each time it's shown (openSavedHost).
    pub saved: String,
    /// While Some, the work [`later`] is handed is kept here in order, to
    /// run as one (a stop with a message: tea.Sequence).
    pub seq: Option<Vec<Job>>,
}

/// Work for off the UI thread, and what's done with its result on it.
pub type Job = Box<dyn FnOnce() -> crate::frame::work::Apply + Send>;

/// Runs work off the UI thread, then apply with its result on it, as
/// Model::later does; while a sequence is being made ([`in_order`]) it
/// waits its turn in it.
pub fn later<T: Send + 'static>(
    m: &mut Model,
    work: impl FnOnce() -> T + Send + 'static,
    apply: impl FnOnce(&mut Model, T) -> crate::frame::Out + Send + 'static,
) {
    if let Some(seq) = core(m).seq.as_mut() {
        seq.push(Box::new(move || {
            let v = work();
            Box::new(move |m: &mut Model| apply(m, v))
        }));
        return;
    }
    m.later(work, apply);
}

/// Does what f starts off the UI thread as one job, each part in the
/// order f started it, and each one's result taken in that order after
/// (Go's tea.Sequence): a stop, then the message that says why.
pub fn in_order(
    m: &mut Model,
    f: impl FnOnce(&mut Model) -> crate::frame::Out,
) -> crate::frame::Out {
    let outer = core(m).seq.replace(Vec::new());
    let out = f(m);
    let jobs = std::mem::replace(&mut core(m).seq, outer).unwrap_or_default();
    if !jobs.is_empty() {
        later(
            m,
            move || jobs.into_iter().map(|j| j()).collect::<Vec<_>>(),
            |m, applies| {
                let mut out = crate::frame::Out::Idle;
                for a in applies {
                    if matches!(a(m), crate::frame::Out::Draw | crate::frame::Out::Close) {
                        out = crate::frame::Out::Draw;
                    }
                }
                out
            },
        );
    }
    out
}

/// One session's side of the Session (Go's hostConn, the parts that
/// aren't the conversation's view).
#[derive(Default)]
pub struct Conn {
    /// Its host, when rush runs it and it's connected.
    pub link: Option<Arc<dyn Link>>,
    pub link_gen: u64,
    /// The worker following the link stops once this is set.
    pub stop: Arc<AtomicBool>,
    /// Dialling the host is under way.
    pub dialing: bool,
    /// Its host sleeps, the session kept: the socket's let go and the
    /// conversation stays; a message wakes it (sleep_host.go parkHost).
    pub sleeping: bool,
    /// A wake is under way: the box keeps what was sent until it's known
    /// to have gone (c.waking).
    pub waking: bool,
    /// The back sheet has opened on this connection: an away that ended
    /// opens it by itself once (c.backShown).
    pub back_shown: bool,
    /// Plugins are being asked about what's in the box before it goes,
    /// and have just been: it goes past them once (c.intercepting,
    /// c.intercepted; plugin_ui::intercept).
    pub intercepting: bool,
    pub intercepted: bool,
    /// The process last dialled: a host gone is dialled again only once
    /// another runs it.
    pub dialed_pid: i32,
    /// The host's queue and state.
    pub info: Info,
    /// The message box.
    pub input: Editor,
    /// The box as last drawn, and the screen row its top edge landed on.
    pub drawn: InputBox,
    pub box_y: Option<usize>,
    /// The box's top edge, and the queue card's first row, as rows into
    /// the dock.
    pub box_off: usize,
    pub q_off: usize,
    /// The queue card's rows as last drawn: the message on each, from
    /// the screen row q_top.
    pub q_rows: Vec<Option<usize>>,
    pub q_top: usize,
    /// The queue card is on the transcript's end, scrolling with it, and
    /// the screen rows [from, to) the conversation shows it in.
    pub q_panel: bool,
    /// The task rows drawn just above the box, how wide, and whether
    /// they're opened from their one.
    pub task_rows: usize,
    pub task_w: usize,
    pub tasks_open: bool,
    /// The column beside a wide conversation, as the dock last made it.
    pub side: side::Side,
    pub q_shown: (usize, usize),
    pub out: Outbox,
    /// When the box was last wiped: its hints say how to bring it back.
    pub cleared_at: Ms,
    /// When it was last asked to stop.
    pub stop_armed: Ms,
    pub last_send: Ms,
    /// The queued message being edited in the box, +1; 0 when none; its
    /// text before, and whether editing held the queue.
    pub edit_q: usize,
    pub edit_was: String,
    pub edit_held: bool,
    /// Its views: which shows, where each is, what the pointer is over.
    pub vs: views::ViewState,
    /// Text dragged over in its conversation.
    pub txt: textsel::TextSel,
    /// Its conversation's map.
    pub map: minimap::Minimap,
    /// How many rows the dock took as last drawn: the map runs on down
    /// beside them.
    pub dock_rows: usize,
    /// The runs its session started, as last read (subs.rs).
    pub runs: subs::Runs,
    /// The dock's running blocks as last drawn: the first row's index
    /// into the dock, and each row's ref, for clicks and ↑.
    pub run_rows: (usize, Vec<String>),
    /// The running blocks the dock handed the column beside the
    /// conversation, for side::make.
    pub side_runs: Vec<running::Block>,
    /// The run it watches, and how it got there (subview.rs).
    pub watch: subview::Watch,
    /// The brief its start added to its prompt, by its rush id, and the
    /// id it was last asked for (c.brief, c.briefFor).
    pub brief: (String, String),
    pub brief_for: String,
    /// The whole transcript before the host's replay is being read: the
    /// host's lines are kept meanwhile (since), to put on top of it
    /// (c.keep, c.since).
    pub keep: bool,
    pub since: Vec<host::Said>,
    /// Rust-only: the view shows its transcript's end, read while its
    /// host was dialled, so it isn't left on "connecting…" (live.rs
    /// early). Once reached, the host's conversation is put together in
    /// shadow, the end staying on screen, and takes its place when the
    /// replay is in (the info that ends it) or by shadow_until.
    pub early: bool,
    pub shadow: Option<rush_convo::Session>,
    pub shadow_until: Option<std::time::Instant>,
    /// Control requests passed to its agent, waiting on their answers, by
    /// id; and the last id given (c.asks, c.askN).
    pub asks: HashMap<String, crate::screens::session_cards::Reply>,
    pub ask_n: u64,
    /// What fills its context window, as last counted, and what of it
    /// the session has never used, worked out when it came (sess.Usage,
    /// Unused).
    pub context: Option<Box<rush_core::usage::Context>>,
    pub unused: Option<Vec<crate::screens::session_cards::slim::Unused>>,
    /// The paths its rewinds left, newest first, as read when its host
    /// was reached.
    pub branches: Vec<crate::screens::session_cards::rewind::Branch>,
    /// The images its box holds, each [Image #N] in its text (c.imgs).
    pub imgs: crate::screens::session_cards::imagerefs::ImageRefs,
    /// Its latest turns' shell calls rush ran as scripts, by tool call id,
    /// as last read (c.scripts).
    pub scripts: HashMap<String, scripts::Seen>,
    /// The / # @ picker's row (c.slashSel).
    pub slash_sel: usize,
    /// The commands and skills on disk for its agent's account and
    /// folder, the names of the skills among them, and when they were
    /// read (c.local, c.skills, commandsOf's 30 seconds).
    pub local: Vec<rush_core::event::Command>,
    pub skills: std::collections::HashSet<String>,
    pub local_at: Option<Ms>,
    /// What /model and /effort were last switched to here, by name, ""
    /// for the agent's own default (c.picked): put back when the host
    /// refuses it.
    pub picked: HashMap<String, String>,
}

impl Conn {
    /// rush hosts it and it's connected (Go's c.client != nil).
    pub fn hosted(&self) -> bool {
        self.link.is_some()
    }
}

/// The open session's key, if one is open.
pub fn open_key(m: &Model) -> Option<String> {
    m.session.as_ref().map(|v| v.key.clone())
}

/// The agent with key, as the list last read it.
pub fn agent<'a>(m: &'a Model, key: &str) -> Option<&'a Agent> {
    m.agents.iter().find(|a| a.key == key)
}

/// The Session's state.
pub fn core(m: &mut Model) -> &mut Core {
    m.kept::<Core>()
}

/// Session key's side, made on first use.
pub fn conn<'a>(m: &'a mut Model, key: &str) -> &'a mut Conn {
    core(m).conns.entry(key.to_string()).or_default()
}

/// The key the row of keys says opens or closes a picked row: the one
/// you set, else enter (rushmode.go:5161 rowKey). rush's store is the
/// newest once it has read the config.
pub fn row_key(m: &Model) -> String {
    let set = crate::screens::settings::loaded_config(m)
        .map_or(m.cfg.row_key.as_str(), |c| c.row_key.as_str());
    if set.is_empty() {
        "enter".into()
    } else {
        set.into()
    }
}

/// Working or waiting on you (localqueue.go busy).
pub fn busy(a: &Agent) -> bool {
    matches!(a.job.state, State::Working | State::Blocked)
}

/// The agent's own words for where an interactive-kind session is driven
/// from (fleet.go:1551 Where).
pub fn where_(a: &Agent) -> &'static str {
    if a.headless && a.kind.as_str() == "claude" {
        return "run by another program (claude -p)";
    }
    if a.headless {
        return "run by another program";
    }
    "open in a terminal"
}
