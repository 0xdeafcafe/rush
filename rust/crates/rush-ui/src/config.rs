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
}

/// rush's folder: RUSH_HOME, else ~/.config/rush, or ~/.config/agtop while
/// that's the one in use.
pub fn dir() -> PathBuf {
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
