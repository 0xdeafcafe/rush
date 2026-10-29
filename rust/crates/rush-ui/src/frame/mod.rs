//! The frame every screen hangs off: the places and their bar, key and
//! mouse routing, the boxes drawn over a screen (sheets, questions,
//! pickers), the message box and its editing, and the ways screens are
//! added without touching the frame (DESIGN.md "Screens"). After Go's
//! internal/ui/model.go (Update :943, setView :1828), keys.go (key :22),
//! view.go (render :604, frame :651) and the files each module names.

pub mod caps;
pub mod choice;
pub mod confirm;
pub mod dialog;
pub mod edgepush;
pub mod editor;
pub mod everywhere;
pub mod ext;
pub mod framehold;
pub mod ghostty;
pub mod ground;
pub mod headerwash;
pub mod inputbox;
pub mod keyname;
pub mod keys;
pub mod location;
pub mod mackeys;
pub mod messages;
pub mod overlayclick;
pub mod panegeometry;
pub mod picker;
pub mod places;
pub mod relayout;
pub mod scheme;
pub mod screeninfo;
pub mod sheet;
pub mod uistall;
pub mod work;

use rush_draw::{Key, KeyMods, MouseKind, Step};

pub use choice::ChoiceSheet;
pub use confirm::Confirm;
pub use editor::Editor;
pub use ext::{Command, Ext};
pub use inputbox::InputBox;
pub use picker::{Pick, Picker};
pub use places::{Place, PlaceId};
pub use sheet::Sheet;

/// What a screen made of a key, a click or a paste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// Not this screen's: the frame (or the next screen in line) has it.
    Pass,
    /// Taken; nothing on screen changed.
    Idle,
    /// Taken; draw again.
    Draw,
    /// Taken, and the sheet or page that took it is done: a sheet closes,
    /// a place other than Agents goes back to Agents.
    Close,
    /// Ends rush.
    Quit,
}

impl Out {
    /// What the loop makes of it.
    pub fn step(self) -> Step {
        match self {
            Out::Pass | Out::Idle => Step::Idle,
            Out::Draw | Out::Close => Step::Draw,
            Out::Quit => Step::Quit,
        }
    }

    /// Pass as Idle: for a screen that has the last word on a key.
    pub fn or_idle(self) -> Out {
        if self == Out::Pass { Out::Idle } else { self }
    }

    /// Whether the screen took it.
    pub fn used(self) -> bool {
        self != Out::Pass
    }
}

impl From<Step> for Out {
    fn from(s: Step) -> Out {
        match s {
            Step::Idle => Out::Idle,
            Step::Draw => Out::Draw,
            Step::Quit => Out::Quit,
        }
    }
}

/// A key pressed, with its name as Go rush names it (bubbletea's
/// `KeyPressMsg.String()`, cmd as super whichever way the terminal sends
/// it: keys.go:23).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Press {
    pub code: Key,
    pub mods: KeyMods,
    /// "ctrl+c", "shift+tab", "a", "?", "space".
    pub s: String,
}

impl Press {
    pub fn new(code: Key, mods: KeyMods) -> Press {
        let mods = if mods.contains(KeyMods::META) {
            KeyMods(mods.0 & !KeyMods::META.0).with(KeyMods::SUPER)
        } else {
            mods
        };
        Press {
            code,
            mods,
            s: rush_draw::key_string(code, mods),
        }
    }

    /// The press named s ("ctrl+k", "enter", "?"); None for a name no key
    /// has (keyname.go:30 keyOf).
    pub fn of(s: &str) -> Option<Press> {
        keyname::key_of(s).map(|(c, m)| Press::new(c, m))
    }

    /// The text the key types: a char without ctrl, alt or cmd (shift
    /// allowed), as bubbletea's `Text`.
    pub fn text(&self) -> Option<char> {
        match self.code {
            Key::Char(c)
                if !self.mods.contains(KeyMods::CTRL)
                    && !self.mods.contains(KeyMods::ALT)
                    && !self.mods.contains(KeyMods::SUPER) =>
            {
                Some(c)
            }
            _ => None,
        }
    }

    pub fn is(&self, s: &str) -> bool {
        self.s == s
    }
}

/// The mouse, as a screen gets it: what it did, and where, in the screen's
/// cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    pub x: u16,
    pub y: u16,
    pub mods: KeyMods,
}

/// The frame's extension points with its own in: a placeholder for each
/// place and for Agents' Wall until a group registers the real one, and
/// the frame's #commands; then every group's (screens/mod.rs).
pub(crate) fn ext() -> Ext {
    let mut x = Ext::default();
    for id in [PlaceId::Projects, PlaceId::Harnesses, PlaceId::Settings] {
        x.place(id, Box::new(places::Placeholder(id.name())));
    }
    x.agents_page(Box::new(places::Placeholder("Wall")));
    x.command(
        "ghostty",
        "LangWatch's light and dark themes for Ghostty",
        ghostty::command,
    );
    x.command(
        "mackeys",
        "⌘ keys for Terminal.app, through Hammerspoon",
        mackeys::command,
    );
    x.command(
        "log",
        "the last 50 things rush said: answers, what failed, and what it did on its own (clean-ups, worktrees kept, work landed)",
        messages::log_command,
    );
    crate::screens::register(&mut x);
    x
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_messages;
