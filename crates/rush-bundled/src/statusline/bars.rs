//! Bars are rush's own status lines, built the same way as Claude Code's
//! but drawn by rush from what it knows: top at the top right of the
//! window, about every agent at once, and agent at the top of an agent's
//! Session. Their segments are rush's (the screen's bars).
//! Ported from internal/statusline/bars.go:1-88.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{Layout, layout, write_indented};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bars {
    pub top: Layout,
    pub agent: Layout,
}

/// How many lines the agent header has room for; TOP_LINES, rush's top,
/// has one more, beside the tabs.
pub const BAR_LINES: usize = 2;
pub const TOP_LINES: usize = 3;

/// Keeps everyday status quiet; detailed segments remain available in the
/// status-line editor.
pub fn default_top() -> Layout {
    layout(
        &[&["system", "today", "plan", "memory", "version"], &[]],
        " · ",
    )
}

/// Exact historical defaults, including the full metric strip. Only an
/// unchanged default and separator migrate to the quieter layout.
const OLD_TOPS: &[&[&[&str]]] = &[
    &[&["today", "plan"], &["memory", "system", "statushelp"]],
    &[
        &["today", "usage"],
        &["ram", "tokens", "cpu", "net", "disk", "battery", "tmp"],
    ],
    &[&["today", "usage"], &["ram", "cpu", "tmp"]],
    &[
        &["today", "usage"],
        &["ram", "cpu", "disk", "battery", "tmp"],
    ],
    &[
        &["today", "usage"],
        &["ram", "cpu", "net", "disk", "battery", "tmp"],
    ],
];

/// Exact historical agent defaults.
const OLD_AGENTS: &[&[&[&str]]] = &[
    &[
        &["context", "cost", "billing"],
        &["folder", "branch", "model", "effort", "mode", "tmp"],
    ],
    &[
        &["context", "cost"],
        &["folder", "branch", "model", "effort", "mode", "tmp"],
    ],
];

pub fn default_agent() -> Layout {
    layout(
        &[
            &["context", "model", "effort", "billing", "rush"],
            &["folder", "branch"],
        ],
        " · ",
    )
}

/// Where they're kept.
pub fn bars_path() -> PathBuf {
    rush_state::dir().join("bars.json")
}

fn is_old(lines: &[Vec<String>], olds: &[&[&[&str]]]) -> bool {
    olds.iter().any(|o| {
        o.len() == lines.len()
            && o.iter()
                .zip(lines)
                .all(|(a, b)| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y))
    })
}

/// Reads them; one never saved is its default.
pub fn load_bars() -> Bars {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Has {
        lines: Option<serde_json::Value>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Saved {
        top: Has,
        agent: Has,
    }
    let mut b = Bars {
        top: default_top(),
        agent: default_agent(),
    };
    let Ok(raw) = std::fs::read(bars_path()) else {
        return b;
    };
    let (Ok(got), Ok(has)) = (
        rush_state::json::from_slice::<Bars>(&raw),
        rush_state::json::from_slice::<Saved>(&raw),
    ) else {
        return b;
    };
    let present = |h: &Has| h.lines.as_ref().is_some_and(|l| !l.is_null());
    if present(&has.top) && !(is_old(&got.top.lines, OLD_TOPS) && got.top.sep == b.top.sep) {
        b.top = got.top;
    }
    if present(&has.agent)
        && !(is_old(&got.agent.lines, OLD_AGENTS) && got.agent.sep == b.agent.sep)
    {
        b.agent = got.agent;
    }
    b
}

/// Writes them.
pub fn save_bars(b: &Bars) -> Result<(), String> {
    write_indented(&bars_path(), b)
}
