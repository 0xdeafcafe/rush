//! What's known of newer rushes, kept for the screens that show or act on
//! it: social's services find it out (checkUpdate, installUpdate), and
//! Settings' Updates page shows it. The header shows it too, as Go's list
//! hint did (view.go:2833 "#update new rush", :2840 "#reload a newer rush
//! is installed"), with the one key that updates and reloads; a click on
//! it does the same.

use std::sync::Arc;
use std::time::Instant;

use rush_core::text::{Line, width};
use rush_update::Release;

use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// The keymap's action for the one key: update, then reload onto it.
pub const ACTION: &str = "rush.update";

/// The # command the key and the click run.
pub const COMMAND: &str = "update";

/// What installs the newest rush: rush_update::install, or a stand-in in
/// tests.
pub type InstallFn =
    Arc<dyn Fn(&dyn rush_update::Host) -> Result<Release, rush_update::Error> + Send + Sync>;

/// What's known of newer rushes.
#[derive(Default)]
pub struct Update {
    /// A newer rush that's out.
    pub newer: Option<Release>,
    pub updating: bool,
    /// A look for a newer rush is out.
    pub checked: bool,
    /// When the last look for a newer rush came back.
    pub looked: Option<Instant>,
    /// The newest version the news has said is out: said once.
    pub said: String,
    /// A newer rush than this one is installed: #reload runs it.
    pub rebuilt: bool,
    /// When the installed rush was written, as the last look at the hosts
    /// found it: a session whose host runs an older one shows "old build".
    /// 0 unread. Rust-only.
    pub installed: rush_core::time::Ms,
    /// The version the last update put in place, "" when it was put there
    /// some other way.
    pub installed_version: String,
    /// What the update under way is doing: its step, or brew's last line.
    pub step: String,
    /// Why the last update failed, in brew's words; the next one clears it.
    pub failed: String,
    /// Where the header drew the notice: its row and its columns.
    pub notice_at: Option<(usize, usize, usize)>,
    /// Stands in for installing, in tests.
    pub install: Option<InstallFn>,
}

impl Model {
    /// The key that updates, as you have it (alt+u unless you moved it).
    pub fn update_key(&self) -> String {
        self.kept
            .peek::<crate::keybind::KeyState>()
            .and_then(|k| k.map.as_ref())
            .and_then(|m| m.keys(ACTION).first().map(ToString::to_string))
            .unwrap_or_else(|| "alt+u".into())
    }

    /// The header's word on updates: one under way and what it's doing, why
    /// the last failed, a newer rush installed and not running, or a newer
    /// one out. None while there's nothing to say.
    pub fn update_notice(&self) -> Option<Line> {
        let u = self.kept.peek::<Update>()?;
        let key = self.update_key();
        if u.updating {
            let mut l = line([sp("↑ updating rush", ORANGE)]);
            if !u.step.is_empty() {
                l.spans.push(sp(format!(" · {}", u.step), DIM));
            }
            return Some(l);
        }
        if !u.failed.is_empty() {
            return Some(line([
                sp(format!("↑ update failed: {}", first_error(&u.failed)), RED),
                sp(format!(" · {key} again"), DIM),
            ]));
        }
        if u.rebuilt {
            let what = if u.installed_version.is_empty() {
                "a newer rush".to_string()
            } else {
                format!("v{}", u.installed_version)
            };
            return Some(line([
                sp(format!("↑ {what} installed"), YELLOW),
                sp(format!(" · {key} restart"), DIM),
            ]));
        }
        let n = u.newer.as_ref()?;
        Some(line([
            sp(format!("↑ {} ready", n.short()), ORANGE),
            sp(format!(" · {key} update"), DIM),
        ]))
    }

    /// Notes where the header drew the notice in row y, drawn as text, for
    /// clicks.
    pub fn note_update_notice(&mut self, y: usize, text: &str) {
        let at = text.find("↑ ").map(|j| {
            let from = width(&text[..j]);
            (y, from, width(text.trim_end()))
        });
        if at.is_some() || self.kept.peek::<Update>().is_some() {
            self.kept::<Update>().notice_at = at;
        }
    }

    /// A click on the header's notice runs it, as its key does; false when
    /// it wasn't on it.
    pub fn click_update(&mut self, x: usize, y: usize) -> bool {
        let Some((row, from, to)) = self.kept.peek::<Update>().and_then(|u| u.notice_at) else {
            return false;
        };
        if y != row || x < from || x >= to {
            return false;
        }
        let _ = run(self);
        true
    }
}

/// The one action: what #update does, the newest rush installed then this
/// rush reloaded onto it, its sessions moved in place.
pub fn run(m: &mut Model) -> Out {
    match m
        .ext
        .commands
        .iter()
        .find(|c| c.name == COMMAND)
        .map(|c| c.run)
    {
        Some(run) => run(m, ""),
        None => Out::Idle,
    }
}

/// Asks for every session's host to move onto the rush installed now, in
/// place and each between turns: rush reload's note, which the rush that
/// leads acts on once it runs that rush. Off the UI: it writes a file.
pub fn want_moves() {
    let _ = rush_host::reload::want_reload(rush_host::wire::BinStamp::exe());
}

/// The line of what failed that says why: brew's "Error:" line, else the
/// first.
pub fn first_error(s: &str) -> &str {
    let mut lines = s.lines().map(str::trim).filter(|l| !l.is_empty());
    s.lines()
        .map(str::trim)
        .find(|l| l.starts_with("Error:"))
        .or_else(|| lines.next())
        .unwrap_or("")
}
