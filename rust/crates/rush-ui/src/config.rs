//! The few settings the list reads from rush's config.json, read only.
//! After Go's internal/state/state.go (Dir :27, pick :52, Shows :962,
//! StackPercent :965) and ui/model.go (folded :2103, activeFor,
//! justLeftFor, justLeftCount) and model.go:698 (dockLines).

use std::collections::HashMap;
use std::path::PathBuf;

use rush_core::time::Ms;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub hide_columns: Vec<String>,
    pub stack_at: i32,
    pub sort_by: String,
    pub folds: HashMap<String, bool>,
    pub dock_lines: i32,
    pub hide_logo: bool,
    pub active_minutes: i32,
    pub just_left_minutes: i32,
    pub just_left_count: i32,
    /// No map beside the conversation (state.go:77 HideMinimap).
    pub hide_minimap: bool,
    /// "project" splits the list's sections by project (state.go SplitBy).
    pub split_by: String,
    /// "classic" is the list with a heading per project; else the rail
    /// look (state.go ListLook).
    pub list_look: String,
    /// What the rest of the list groups by: "agent", "group" (GroupBy).
    pub group_by: String,
    /// What the screen opens as: "split", "agent", "list" (state.go View).
    pub view: String,
    /// A wide screen kept to the list alone (state.go ListOnly).
    pub list_only: bool,
    /// The list's share of a split screen; 0 is rush's own (SideWidth).
    pub side_width: f64,
    /// A Session opened from the list alone has the screen (ChatFull).
    pub chat_full: bool,
    /// The agents pinned as tiles beside the Session, and which sit under
    /// it; every one under it in an older config (state.go:78 Grid,
    /// GridBelow, GridBelowKeys, GridShare).
    pub grid: Vec<String>,
    pub grid_below: bool,
    pub grid_below_keys: Vec<String>,
    pub grid_share: f64,
    /// The one key that opens or closes a picked row, "" for both enter
    /// and space (state.go RowKey).
    pub row_key: String,
}

/// rush's folder: RUSH_HOME, else ~/.config/rush, or ~/.config/agtop while
/// that's the one in use.
pub fn dir() -> PathBuf {
    let d = resolve_dir();
    rush_core::sandbox::guard("rush's folder", &d);
    d
}

fn resolve_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let rush = home.join(".config").join("rush");
    let agtop = home.join(".config").join("agtop");
    if !rush.join("config.json").exists() && agtop.join("config.json").exists() {
        return agtop;
    }
    rush
}

impl Config {
    /// The config in rush's folder; defaults when there's none.
    pub fn load() -> Config {
        std::fs::read(dir().join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Whether the list is drawn in the rail look (rail.go:23).
    pub fn rail(&self) -> bool {
        self.list_look != "classic"
    }

    /// Whether each section's rows sit by project under its heading
    /// (folders.go:27 splitProjects).
    pub fn split_projects(&self) -> bool {
        if self.rail() {
            self.split_by == "project"
        } else {
            self.split_by != "none"
        }
    }

    /// Keeps the layout picked (state.go SetView).
    pub fn set_view(&mut self, v: &str) {
        self.view = v.into();
        self.list_only = v != "split";
        self.chat_full = v == "agent";
    }

    pub fn shows(&self, col: &str) -> bool {
        !self.hide_columns.iter().any(|c| c == col)
    }

    /// StackAt with its default: 42, and 0 when turned off.
    pub fn stack_percent(&self) -> usize {
        match self.stack_at {
            n if n < 0 => 0,
            0 => 42,
            n => n as usize,
        }
    }

    pub fn sort_by(&self) -> &str {
        if self.sort_by.is_empty() {
            "name"
        } else {
            &self.sort_by
        }
    }

    pub fn folded(&self, title: &str) -> bool {
        match self.folds.get(title) {
            Some(&v) => v,
            None => title == "Earlier" || title == "Scratch",
        }
    }

    /// How long a stopped agent stays in Idle before Today has it.
    pub fn active_for(&self) -> Ms {
        match self.active_minutes {
            n if n < 0 => 0,
            0 => 5 * 60_000,
            n => n as Ms * 60_000,
        }
    }

    /// How long an agent stays in Just left after its last activity.
    pub fn just_left_for(&self) -> Ms {
        match self.just_left_minutes {
            n if n < 0 => 0,
            0 => 2 * 3_600_000,
            n => n as Ms * 60_000,
        }
    }

    pub fn just_left_count(&self) -> usize {
        if self.just_left_count == 0 {
            10
        } else {
            self.just_left_count.max(0) as usize
        }
    }

    /// The card's lines of text, for a screen h rows tall.
    pub fn dock_lines(&self, h: usize) -> usize {
        let n = if self.dock_lines == 0 {
            3
        } else {
            self.dock_lines.max(1) as usize
        };
        n.min((h / 3).max(1))
    }
}

/// A change to rush's config.json, waiting to be written.
type Change = Box<dyn FnOnce(&mut rush_state::Config) + Send>;

/// The changes not yet written, in the order they were made.
static WAITING: std::sync::Mutex<std::collections::VecDeque<Change>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// One writer of config.json at a time, from whichever thread.
static KEEPING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Writes every change waiting, in the order they were made, on top of
/// config.json as it is: whichever thread gets here first writes them
/// all, so a later change never lands before an earlier one.
fn write_waiting() -> Result<(), String> {
    let _g = KEEPING.lock().unwrap_or_else(|e| e.into_inner());
    let changes: Vec<Change> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect();
    if changes.is_empty() {
        return Ok(()); // an earlier thread wrote this one with its own
    }
    // A save still on its way to disk is there before it's read again.
    rush_state::flush().map_err(|e| e.to_string())?;
    let mut s = rush_state::Store::load();
    for f in changes {
        f(&mut s.config);
    }
    s.save_config().map_err(|e| e.to_string())
}

/// Writes a whole copy of the config the UI holds (Settings', the
/// accounts'), never in the middle of a queued change being written: the
/// copy has every queued change made in it already.
pub fn write_store(s: &rush_state::Store) -> std::io::Result<()> {
    let _g = KEEPING.lock().unwrap_or_else(|e| e.into_inner());
    s.save_config()
}

/// Changes config.json as f says, for off the UI thread: each change is
/// made on top of the last one kept, so two quick ones (a pin and a
/// drag, say) don't save over each other.
pub fn keep(f: impl FnOnce(&mut rush_state::Config) + Send + 'static) -> Result<(), String> {
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(Box::new(f));
    write_waiting()
}

/// The one way the UI changes config.json (Go's SaveConfig, in order on
/// the UI thread): f made at once in each copy of the config rush holds
/// (Settings', and the one the accounts and the watcher for outside
/// changes read, so neither writes the old value back), then queued here,
/// in key order, and written off the UI thread. A failed write is flashed
/// after `what` ("couldn't keep the grid"). `--render` writes nothing.
pub fn save(
    m: &mut crate::model::Model,
    what: &'static str,
    f: impl Fn(&mut rush_state::Config) + Send + 'static,
) {
    use crate::screens::accounts::state::{accounts, read_only};
    if crate::screens::settings::loaded_config(m).is_some() {
        f(crate::screens::settings::config_mut(m));
    }
    if let Some(s) = accounts(m).store.as_mut() {
        f(&mut s.config);
    }
    if read_only(m) {
        return; // --render leaves config.json as it was
    }
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(Box::new(f));
    m.later(write_waiting, move |m, r| match r {
        Ok(()) => crate::frame::Out::Idle,
        Err(e) => {
            m.fail(&format!("{what}: {e}"));
            crate::frame::Out::Draw
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(f: impl FnOnce(&mut rush_state::Config) + Send + 'static) {
        WAITING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Box::new(f));
    }

    /// Two changes made in quick succession land in the order made, even
    /// when the later one's writer gets there first: Go saves them in
    /// order on the UI thread (SaveConfig).
    #[test]
    fn changes_land_in_the_order_made() {
        let step = |s: &'static str| {
            move |c: &mut rush_state::Config| {
                c.onboarding.steps.retain(|x| !x.starts_with("order-test"));
                c.onboarding.steps.push(s.into());
            }
        };
        queue(step("order-test-first"));
        queue(step("order-test-second"));
        // The second change's writer first: it writes both, in order.
        write_waiting().unwrap();
        write_waiting().unwrap();
        let steps = rush_state::Store::load().config.onboarding.steps;
        assert!(
            steps.contains(&"order-test-second".to_string()),
            "{steps:?}"
        );
        assert!(
            !steps.contains(&"order-test-first".to_string()),
            "{steps:?}"
        );
    }

    /// A change is made in Settings' copy of the config too, so a setting
    /// changed there afterwards doesn't save the old value back.
    #[test]
    fn a_change_reaches_settings_copy() {
        let mut m = crate::model::Model::new("t");
        crate::screens::settings::use_store(&mut m, rush_state::Store::load());
        let mark = "settings-copy-test".to_string();
        let f = mark.clone();
        save(&mut m, "couldn't", move |c| {
            c.onboarding.steps.push(f.clone())
        });
        let held = &crate::screens::settings::config_mut(&mut m)
            .onboarding
            .steps;
        assert!(held.contains(&mark), "{held:?}");
        let disk = rush_state::Store::load().config.onboarding.steps;
        assert!(disk.contains(&mark), "{disk:?}");
    }
}
