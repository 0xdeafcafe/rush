//! More of the Agents list (Go: internal/ui): the Agents box and its row
//! of keys (prompt.rs, view.go:2683 promptLines), the list's filter
//! (filter.rs, listfilter.go), the Wall page (wall.rs, wall.go),
//! neighbour (model.go:2744). Each part is added through the frame's
//! extension points in `register`; nothing here edits the list itself.

use crate::frame::Ext;
use crate::list::ListLine;
use crate::model::Model;

pub mod filter;
pub mod fuzzy;
pub mod prompt;
pub mod wall;

#[cfg(test)]
mod tests;

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
    // The Wall: every open agent, a tile each.
    x.agents_page(Box::new(wall::Wall::default()));
    x.on_fleet.push(wall::on_fleet);
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
