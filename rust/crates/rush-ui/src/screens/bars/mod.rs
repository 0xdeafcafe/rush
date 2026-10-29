//! rush's own status lines: the top bar down the right of the header,
//! about every agent, and the agent header at the top of an agent's
//! Session, each built in /statusline from the segments here. After Go's
//! internal/ui/bars.go (barSeg :29, barCtx :37, barLayout :352, barLine
//! :378, barKeep :418, leastKept :432, barDropped :450), bars_quiet.go,
//! usagebar.go, header_context.go and view.go's header right side
//! (header :165, topRights :256, activeUsage :555, usageDetail :590,
//! usageMeter :496, runsOut :530). What always shows stays put and is the
//! frame's: the counts and clanker, an agent's name, state and
//! connection.
//!
//! [`line`] draws a line of segments and keeps what had no room;
//! [`top`] is the top bar's segments and the header's right; [`agent`]
//! is the agent header's segments, drawn over the Session's header rows,
//! and a click on its permission mode.

pub mod agent;
pub mod fetch;
pub mod line;
pub mod top;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use rush_bundled::statusline::{self as sl, Bars, Layout};

use crate::frame::ext::TopBar;
use crate::frame::{Ext, Out};
use crate::model::Model;
use crate::screens::settings::statusline::{self as builder, Header, StatusSheet};

pub use line::{BAR_AGENT, BAR_TOP};

/// What the bars keep between frames.
#[derive(Default)]
pub struct State {
    /// The bars as saved, once read (m.bars); None draws the defaults.
    pub bars: Option<Bars>,
    /// They're being read.
    loading: bool,
    /// The launch folder's repository was looked for.
    rooted: bool,
    /// What each line last drawn left out for room, by which*10+line
    /// (barDrops).
    pub drops: HashMap<usize, Vec<String>>,
    /// Where the open Session's permission mode was drawn: its header
    /// row, and from and width, cells into the row's content
    /// (hostConn.modeLabel).
    pub mode_at: (usize, usize, usize),
    /// The session whose mode the pointer is over (paneHover.mode).
    pub hover_mode: String,
}

/// Adds the bars to x: the header's top bar, the agent header over the
/// Session's header rows, a click on its permission mode, and their
/// segments to /statusline.
pub fn register(x: &mut Ext) {
    x.top_bar = Some(TopBar {
        rights: top::top_rights,
        gauges: top::header_gauges,
        usage_shown: |m| layout(m, BAR_TOP).shown("usage"),
        dropped: |m, drops| {
            let st = m.kept::<State>();
            for (k, v) in drops {
                st.drops.insert(k, v);
            }
        },
        orphans: top::orphans,
    });
    x.session_head.push(agent::head);
    x.mouse.push(agent::mouse);
    x.on_fleet.push(on_fleet);
    x.on_fleet.push(fetch::on_fleet);
    builder::provide(Header {
        segs: line::seg_infos,
        sample: line::sample,
        preview: line::preview,
        dropped,
        bars: |m| m.kept.peek::<State>().and_then(|s| s.bars.clone()),
        saved: |m, b| {
            m.kept::<State>().bars = Some(b);
        },
    });
}

/// With each reading of the list: the bars read once, off the UI thread
/// (loadBars), and the launch folder put at its repository's top
/// (launchDir).
fn on_fleet(m: &mut Model) {
    let st = m.kept::<State>();
    if !st.loading && st.bars.is_none() {
        st.loading = true;
        m.later(sl::load_bars, |m, b| {
            let st = m.kept::<State>();
            st.loading = false;
            if st.bars.is_none() {
                st.bars = Some(b);
            }
            Out::Draw
        });
    }
    // Only with a loop: a test's model stays where it was put.
    if m.sender().is_some() {
        root_launch_dir(m);
    }
}

/// Puts the launch folder at the top of the repository it's in, once,
/// off the UI thread (launchDir's RepoRoot).
pub fn root_launch_dir(m: &mut Model) {
    let st = m.kept::<State>();
    if !st.rooted {
        st.rooted = true;
        let dir = m.launch_dir.clone();
        m.later(
            move || rush_sys::actions::repo_root(&dir),
            |m, root| {
                if root.is_empty() || root == m.launch_dir {
                    return Out::Idle;
                }
                m.launch_dir = root;
                Out::Draw
            },
        );
    }
}

/// The line being drawn: the one being built in /statusline, so the real
/// header shows your changes as you make them, else the one saved, else
/// its default (barLayout).
pub fn layout(m: &Model, which: usize) -> Layout {
    if let Some(st) = m
        .sheet
        .as_deref()
        .and_then(|s| (s as &dyn std::any::Any).downcast_ref::<StatusSheet>())
    {
        match (which, st.tab) {
            (BAR_TOP, builder::ST_TOP) => return st.bars.top.clone(),
            (BAR_AGENT, builder::ST_AGENT) => return st.bars.agent.clone(),
            _ => {}
        }
    }
    let saved = m.kept.peek::<State>().and_then(|s| s.bars.as_ref());
    match (which, saved) {
        (BAR_TOP, Some(b)) if !b.top.lines.is_empty() => b.top.clone(),
        (BAR_TOP, _) => sl::default_top(),
        (_, Some(b)) if !b.agent.lines.is_empty() => b.agent.clone(),
        _ => sl::default_agent(),
    }
}

/// The segments the header last left out for room (barDropped).
pub fn dropped(m: &Model, which: usize) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Some(st) = m.kept.peek::<State>() {
        for i in 0..sl::TOP_LINES {
            if let Some(ids) = st.drops.get(&(which * 10 + i)) {
                out.extend(ids.iter().cloned());
            }
        }
    }
    out
}

/// The folder rush was opened in, as the shell names it: $PWD when that
/// is where we are, so /tmp stays /tmp rather than /private/tmp (Go's
/// os.Getwd, model.go:495 launchDir).
pub fn getwd() -> String {
    let real = std::env::current_dir().unwrap_or_default();
    if let Some(pwd) = std::env::var_os("PWD").map(std::path::PathBuf::from)
        && pwd.is_absolute()
        && same_file(&pwd, &real)
    {
        return pwd.to_string_lossy().into_owned();
    }
    real.to_string_lossy().into_owned()
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}
