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
/// What goes at the right of header row `row` (0 the counts, 1 the
/// folder, 2 the tabs, 3 clanker's feet), at most `room` cells wide: the
/// top bar's gauges (view.go:256 topRights). None leaves it as it is.
pub type HeaderRight = fn(&Model, usize, usize) -> Option<Line>;
/// Rows drawn under the list or the open conversation, `w` wide: the
/// Agents box, a Session's message box and its cards (view.go listView's
/// prompt, the pane's dock). The body gives up the rows they take.
pub type Dock = fn(&mut Model, usize) -> Vec<Line>;
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
    pub header_right: Vec<HeaderRight>,
    pub list_dock: Vec<Dock>,
    pub session_dock: Vec<Dock>,
    pub overlays: Vec<Overlay>,
    pub on_fleet: Vec<FleetHook>,
    pub status: Vec<StatusHook>,
    pub session_body: Vec<SessionBody>,
    pub session_head: Vec<SessionHead>,
    pub submit: Vec<SubmitHook>,
    /// The Agents box is on screen (a list_dock draws it from
    /// `Model::input`): text typed in the list goes into it, and the
    /// list's letter keys (j k g G h l q) are text, as in Go.
    pub agents_box: bool,
    /// The places past Agents, by PlaceId (Agents is the frame's own).
    pub(crate) places: [Option<Box<dyn Place>>; 4],
    /// The Agents place's pages past its list ("Wall").
    pub(crate) agents_pages: Vec<Option<Box<dyn Place>>>,
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
