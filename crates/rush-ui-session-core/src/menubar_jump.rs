//! A click on a notification, or on an agent in the menu bar's menu,
//! brings this rush's terminal forward and leaves the agent to go to
//! (rush_menubar::goto): it's opened once it's in the list. Port of
//! internal/ui/snapload.go:41 and :52-55 (msg.jump); read again as the
//! window comes back into focus too, since a reading here comes only when
//! the list changes.

use crate::frame::Ext;
use crate::model::Model;

pub fn register(x: &mut Ext) {
    x.on_fleet.push(jump);
    x.on_focus.push(jump);
}

/// Opens the agent the menu bar asked for, if it asked and it's listed;
/// a hosted view stays on its own session.
fn jump(m: &mut Model) {
    if (m.ext.calls.hosted_id())(m).is_some() {
        return;
    }
    let key = rush_menubar::goto();
    jump_to(m, key);
}

/// Opens the agent with row key, when it's in the list.
pub fn jump_to(m: &mut Model, key: String) {
    if key.is_empty() {
        return;
    }
    let Some(i) = m.agents.iter().position(|a| a.key == key) else {
        return;
    };
    // On Agents' list, nothing over it, as the bar's goAgent goes
    // (cmdbar/bar.rs go_agent): not behind Settings, Projects, the Wall
    // or a sheet.
    if m.place != crate::frame::places::PlaceId::Agents {
        m.set_place(crate::frame::places::PlaceId::Agents);
    } else {
        m.sheet = None;
        m.confirm = None;
    }
    m.agents_page = 0;
    if m.session.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    // The one it replaces is kept for its tile when that agent is pinned
    // (gridPark).
    if let Some(old) = m.session.take() {
        m.ext.calls.grid_park()(m, old);
    }
    m.select(key);
    m.open(i);
    super::live::sync(m); // its host's link, now it's showing
}
