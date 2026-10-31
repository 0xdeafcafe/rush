//! More of the Agents list (Go: internal/ui): the Agents box and its row
//! of keys (prompt.rs, view.go:2683 promptLines), the list's filter
//! (filter.rs, listfilter.go), the Wall page (wall.rs, wall.go),
//! neighbour (model.go:2744), the Projects place's session lines and
//! timelines (work.rs, workstreams.go), program status (progstatus.rs),
//! #away's words (away.rs), the limit meters (meters.rs, usagebar.go,
//! bars.go, usage_limit.go; views' bars are plugin_ui's views) and
//! kept rows (rowmemo.rs, rowmemo.go, rowindex.go), the plan's line and
//! the next account (plan.rs, bars_quiet.go, agentquota.go), a session's
//! tasks (jobs.rs, jobs.go, job_activity.go), the grid (grid.rs,
//! grid.go), subagent rows (subs.rs, subrows.go, subpreview.go), spawned
//! agents' steps (spawns.rs), stuck nudges' words
//! (stuck.rs, stucksubs.go), hosted's list key (hosted.rs), the
//! "agent" layout's peek at Agents (peek.rs) and the
//! finishing card for pull requests (finish.rs, finish.go), and what's
//! left before an agent's work lands (landing.rs, landing.go), and
//! renaming an agent in the box (rename.rs, keys.go startRename), the
//! list's keys on the agent and the list (listkeys.rs: ctrl+y, ctrl+t,
//! ctrl+s, ctrl+p, ctrl+v) and the stash (stash.rs, stash.go). What a
//! finished turn wants of you is session_core's turnwants.rs, which the
//! list and the Wall read. Each part is added through the frame's
//! extension points in `register`.

use crate::frame::{Ext, Out};
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
pub mod listkeys;
pub mod live;
pub mod meters;
pub mod peek;
pub mod plan;
pub mod progstatus;
pub mod prompt;
pub mod rename;
pub mod rowmemo;
pub mod shelf;
pub mod spawns;
pub mod split;
pub mod stash;
pub mod stuck;
pub mod subs;
pub mod vt;
pub mod wall;
pub mod work;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_finish;
#[cfg(test)]
mod tests_grid;
#[cfg(test)]
mod tests_grouping;
#[cfg(test)]
mod tests_jobs;
#[cfg(test)]
mod tests_landing;
#[cfg(test)]
mod tests_list;
#[cfg(test)]
mod tests_listkeys;
#[cfg(test)]
mod tests_live;
#[cfg(test)]
mod tests_plan;
#[cfg(test)]
mod tests_rename;
#[cfg(test)]
mod tests_rooms;
#[cfg(test)]
mod tests_subs;
#[cfg(test)]
mod tests_turnwants;
#[cfg(test)]
mod tests_wall;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(x: &mut Ext) {
    // The Agents box under the list, and the keys under it.
    x.agents_box = true;
    x.list_dock.push(prompt::dock);
    x.list_hint.push(prompt::hint);
    x.mouse.push(prompt::mouse);
    // ctrl+r, and renaming in the box: before the list's other keys, as
    // Go's rename step comes before tab's and ↑↓'s (keys.go:179).
    x.list_keys.insert(0, rename::key);
    x.command(
        "rename",
        "rename the agent, or type the new name",
        rename::command,
    );
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
    // Agents and the Session side by side: shift+← →, #view; with
    // something typed they select in the box instead (keys.go:502-507).
    x.list_keys.push(|m, k| {
        if m.input.is_empty() {
            split::key(m, k)
        } else {
            Out::Pass
        }
    });
    // ctrl+y, ctrl+t, ctrl+s, ctrl+p and ctrl+v: the agent's pull
    // request and pin, how the list groups and splits, an image into the
    // box.
    x.list_keys.push(listkeys::key);
    // #stash: what was set aside, sent, cleared and replaced.
    x.command(
        "stash",
        "what you set aside, sent, cleared and replaced, to put back in the box (ctrl+r · ctrl+s sets what's typed aside)",
        |m, _| stash::command(m, "history"),
    );
    // ⌥1…9: what the next session starts as, off the box's edge.
    x.list_keys.push(shelf::key);
    // alt+l: a worktree agent's new sessions in its worktree or checkout.
    x.list_keys.push(prompt::worktree_key);
    // The grid's tiles round the Session (a click swaps one in, its edge
    // resizes them) take the mouse in frame/keys.rs, at screen columns.
    x.command(
        "view",
        "Agents and the Session side by side, the agent's Session alone, or Agents alone (shift+← →)",
        split::command,
    );
    // The preview's live mirror: the picked agent's own screen beside
    // the list, kept once a frame; v swaps it for the summary.
    x.overlays.push(live::sync);
    x.list_keys.push(live::key);
    // Typing into it: every key goes there first while it has them; tab,
    // → or a click on it gives them, ctrl+] or a click on Agents takes
    // them back.
    x.keys.insert(0, live::embed_key);
    x.list_keys.insert(0, live::enter_key);
    x.mouse.insert(0, live::mouse);
    x.paste.insert(0, live::paste);
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
        None => {
            m.flash_err("select an agent first");
            crate::frame::Out::Draw
        }
    }
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
            ListLine::Agent(i) | ListLine::Member { i, .. } => {
                let k = m.agents[*i].key.as_str();
                if k == key {
                    at = Some(rows.len());
                }
                rows.push(k);
            }
            ListLine::Room { id, .. } => {
                let k = m
                    .rooms_grouped()
                    .and_then(|g| g.row.get(id))
                    .map_or("", |a| a.key.as_str());
                if k == key {
                    at = Some(rows.len());
                }
                rows.push(k);
            }
            ListLine::Blank | ListLine::Project { .. } | ListLine::Tree { .. } => {}
        }
    }
    match at {
        Some(i) if i + 1 < rows.len() => rows[i + 1].to_string(),
        Some(i) if i > 0 => rows[i - 1].to_string(),
        _ => String::new(),
    }
}
