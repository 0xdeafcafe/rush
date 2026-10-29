//! What the Efficiency place knows: where tokens go, hour by hour, what
//! each session carried in its context, the token savers that are
//! installed and when they were used, and what that changed; and the
//! advisor, which reads those figures with a cheap model and checks what
//! it finds with a careful one.
//!
//! Ported from Go's internal/efficiency (scan.go, aggregate.go,
//! catalog.go, compare.go, detect.go, estimate.go, events.go, findings.go,
//! gain.go, install.go, memory.go, source.go, store.go, top.go), the
//! Claude Code reader of internal/adapters/claude/efficiency.go, and
//! internal/advisor. Its files are Go's, so either rush reads the other's.
//!
//! Nothing here runs on the UI thread: a Store's refresh reads
//! transcripts on threads of its own, and a view is worked out from what
//! it holds (a read lock, never held across a read of the disk).

pub mod advisor;
pub mod aggregate;
pub mod catalog;
pub mod claude;
#[cfg(test)]
mod claude_tests;
pub mod compare;
pub mod detect;
pub mod estimate;
pub mod events;
pub mod findings;
pub mod gain;
pub mod install;
pub mod memory;
pub mod price;
pub mod scan;
pub mod source;
pub mod store;
pub mod top;
pub mod when;

use std::path::PathBuf;

pub use aggregate::{
    METRIC_NAMES, Metric, Point, Query, RANGES, Range, SaverUse, Series, View, quantile,
};
pub use catalog::{
    CATALOG, Detect, KIND_NAMES, Kind, Recipe, Saver, Setting, SettingValue, Step, find,
};
pub use compare::{Compare, FEW_SESSIONS, Side};
pub use detect::{Env, Found, Status, look_path, pick};
pub use estimate::{Base, Cut, Estimate, Measured, Verdict, Working};
pub use events::{Event, add_event, load_events, observe, remember};
pub use findings::{Finding, bytes, findings, money, tokens};
pub use gain::{Gain, rtk_gains};
pub use install::Plan;
pub use memory::memory_findings;
pub use price::cache_read_price;
pub use scan::{
    BIG_RESULT, Bucket, Compact, File, N_TOOLS, TOOL_BASH, TOOL_EDIT, TOOL_MCP, TOOL_NAMES,
    TOOL_OTHER, TOOL_READ, TOOL_SEARCH, Use, looks, scan,
};
pub use source::{Setup, Source, source, transcripts, transcripts_dir};
pub use store::{Retired, Store};
pub use top::SessionCost;

/// The agent whose sessions efficiency reads, and whose prices it counts
/// them at: Claude Code's transcripts are the only ones it reads
/// (aggregate.go Agent, state.LoginsKind).
pub const AGENT: &str = "claude";

/// Where the Efficiency place keeps what can't be worked out again: the
/// event log, the retired figures and backups of files it changed
/// (store.go Dir).
pub fn dir() -> PathBuf {
    rush_state::dir().join("efficiency")
}

/// Replaces path whole, never leaving half a file (store.go writeFile).
pub(crate) fn write_file(path: &std::path::Path, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        TMP_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let res = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(b)?;
        f.sync_all()
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

static TMP_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) mod testutil {
    use std::sync::Once;

    /// Claude Code's adapter in the global registry, as rush's binary
    /// registers it: its prices and its tools' names.
    pub fn claude() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| rush_core::adapter::global().register(rush_claude::Claude::new()));
    }
}
