//! More of the Agents list (Go: internal/ui): the Agents box and its row
//! of keys (prompt.rs, view.go:2683 promptLines), the list's filter
//! (filter.rs, listfilter.go), the Wall page (wall.rs, wall.go),
//! neighbour (model.go:2744), the Projects place's session lines and
//! timelines (work.rs, workstreams.go), program status (progstatus.rs),
//! #away's words (away.rs), the limit meters (meters.rs, usagebar.go,
//! bars.go, usage_limit.go), views' bars (views.rs, agentviews.go) and
//! kept rows (rowmemo.rs, rowmemo.go, rowindex.go), the plan's line and
//! the next account (plan.rs, bars_quiet.go, agentquota.go), a session's
//! tasks (jobs.rs, jobs.go, job_activity.go), the grid (grid.rs,
//! grid.go), subagent rows (subs.rs, subrows.go, subpreview.go), spawned
//! agents' steps (spawns.rs), shells (shells.rs), stuck nudges' words
//! (stuck.rs, stucksubs.go), hosted's list key (hosted.rs) and the
//! finishing card for pull requests (finish.rs, finish.go), and what's
//! left before an agent's work lands (landing.rs, landing.go). Each part
//! is added through the frame's extension points in `register`.

use crate::frame::Ext;
use crate::list::ListLine;
use crate::model::Model;

pub mod away;
pub mod filter;
pub mod finish;
pub mod fuzzy;
pub mod grid;
pub mod hosted;
pub mod jobs;
pub mod landing;
pub mod meters;
pub mod plan;
pub mod progstatus;
pub mod prompt;
pub mod rowmemo;
pub mod shells;
pub mod spawns;
pub mod stuck;
pub mod subs;
pub mod views;
pub mod wall;
pub mod work;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_finish;
#[cfg(test)]
mod tests_grid;
#[cfg(test)]
mod tests_jobs;
#[cfg(test)]
mod tests_landing;
#[cfg(test)]
mod tests_list;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_subs;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    // The Agents box under the list, and the keys under it.
    x.agents_box = true;
    x.list_dock.push(prompt::dock);
    x.list_hint.push(prompt::hint);
    x.mouse.push(prompt::mouse);
    // alt+f: the list narrowed to what's typed.
    x.list_keys.push(filter::key);
    x.list_filter.push(filter::shows);
    x.column_header.push(filter::header);
    x.list_empty.push(filter::empty);
    // ctrl+b: tell the picked agent to keep going.
    x.list_keys.push(keep_going_key);
    // alt+p and #land: tell the picked agent to land its work; the list
    // asks git what's left before each lands.
    x.list_keys.push(landing::key);
    x.command(
        "land",
        "tell the agent to land its work: commit it, then merge its branch or pull request (alt+p)",
        landing::command,
    );
    x.on_fleet.push(landing::refresh);
    // The Wall: every open agent, a tile each.
    x.agents_page(Box::new(wall::Wall::default()));
    x.on_fleet.push(wall::on_fleet);
}

/// ctrl+b or alt+g in the list: the picked agent goes on, or why it
/// can't (keys.go:467, keepGoing :747).
fn keep_going_key(m: &mut Model, k: &crate::frame::Press) -> crate::frame::Out {
    if k.s != "ctrl+b" && k.s != "alt+g" {
        return crate::frame::Out::Pass;
    }
    match m.selected() {
        Some(i) => wall::keep_going(m, i),
        None => m.flash_err("select an agent first"),
    }
    crate::frame::Out::Draw
}

/// The agent row after key in its section, or the one before when key is
/// the last; "" when it's alone there (model.go:2744 neighbour).
pub fn neighbour(m: &Model, key: &str) -> String {
    let mut rows: Vec<&str> = Vec::new();
    let mut at: Option<usize> = None;
    for l in &m.lines {
        match l {
            ListLine::Section { .. } => {
                if at.is_some() {
                    break;
                }
                rows.clear();
            }
            ListLine::Agent(i) => {
                let k = m.agents[*i].key.as_str();
                if k == key {
                    at = Some(rows.len());
                }
                rows.push(k);
            }
            ListLine::Blank => {}
        }
    }
    match at {
        Some(i) if i + 1 < rows.len() => rows[i + 1].to_string(),
        Some(i) if i > 0 => rows[i - 1].to_string(),
        _ => String::new(),
    }
}
