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

/// One change to config.json at a time, from whichever thread.
static KEEPING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Changes config.json as f says, for off the UI thread: each change is
/// made on top of the last one kept, so two quick ones (a pin and a
/// drag, say) don't save over each other.
pub fn keep(f: impl FnOnce(&mut rush_state::Config)) -> Result<(), String> {
    let _g = KEEPING.lock().unwrap_or_else(|e| e.into_inner());
    // A save still on its way to disk is there before it's read again.
    rush_state::flush().map_err(|e| e.to_string())?;
    let mut s = rush_state::Store::load();
    f(&mut s.config);
    s.save_config().map_err(|e| e.to_string())
}
