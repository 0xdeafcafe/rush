//! rush's thin drawing layer: the only crate that knows ratatui and
//! crossterm, and none of their types cross it. Screens draw rush's own
//! styled lines ([`rush_core::text::Line`]) on a [`Canvas`]; input comes
//! as rush's own [`Event`]s. See DESIGN.md.

mod canvas;
mod fit;
mod headless;
mod input;
mod perf;
mod reader;
mod run;
mod term;

pub use canvas::{Canvas, Rect};
pub use fit::{truncate, truncate_line};
pub use headless::Headless;
pub use input::{Button, Event, Key, KeyMods, MouseKind, key_string, keystroke};
pub use perf::note as perf_note;
pub use run::{App, Loop, Msg, Outside, Sender, Step, TrySend};
pub use term::Term;

#[cfg(test)]
mod tests;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
