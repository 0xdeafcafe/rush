//! A plugin with the sidebar capability arranges the agent list its own
//! way: it offers a group-by mode, plugin:<name>, whose sections are the
//! plugin's and whose rows carry the names it gives. Agents it doesn't
//! place go to Other, folded. After Go's internal/ui/sidebar.go
//! (groupModes :25, groupLabel :34, activeSidebar :47, loadSidebars :62,
//! nameAgents :73, namePeers :98, sidebarPlace :124).

use std::collections::HashMap;

use rush_core::agent::Agent;
use rush_plugin::Sidebar;

use crate::frame::Out;
use crate::model::Model;

pub const PLUGIN_GROUP_PREFIX: &str = "plugin:";
pub const OTHER_SECTION: &str = "Other";

/// rush's own group-by modes (model.go:58 groupModes).
pub const GROUP_MODES: [&str; 3] = ["status", "agent", "group"];

/// The group-by modes on offer: rush's own, then one for each plugin that
/// arranges the list (groupModes).
pub fn group_modes(sidebars: &[Sidebar]) -> Vec<String> {
    let mut out: Vec<String> = GROUP_MODES.iter().map(|s| s.to_string()).collect();
    out.extend(
        sidebars
            .iter()
            .map(|s| format!("{PLUGIN_GROUP_PREFIX}{}", s.plugin)),
    );
    out
}

/// How a group-by mode is named to you (groupLabel).
pub fn group_label(sidebars: &[Sidebar], mode: &str) -> String {
    if let Some(name) = mode.strip_prefix(PLUGIN_GROUP_PREFIX)
        && let Some(s) = sidebars.iter().find(|s| s.plugin == name)
    {
        return s.label().to_string();
    }
    mode.to_string()
}

/// The plugin arrangement the list is grouped by, if any (activeSidebar).
pub fn active<'a>(sidebars: &'a [Sidebar], group_by: &str) -> Option<&'a Sidebar> {
    let name = group_by.strip_prefix(PLUGIN_GROUP_PREFIX)?;
    sidebars.iter().find(|s| s.plugin == name)
}

/// Reads the plugins' arrangements again where their files changed, off
/// the UI (loadSidebars).
pub fn load(m: &mut Model) {
    let p = m.plugins();
    let mut files = std::mem::take(&mut p.sidebar_files);
    let disk = p.disk.clone();
    m.later(
        move || {
            let s = disk.sidebars(&mut files);
            (files, s)
        },
        |m, (files, s)| {
            let p = m.plugins();
            p.sidebar_files = files;
            if p.sidebars == s {
                return Out::Idle;
            }
            p.sidebars = s;
            Out::Draw
        },
    );
}

/// The names the arrangement gives agents, by key, except where you
/// renamed one in rush (nameAgents): `renamed` holds rush's own names.
pub fn agent_names(
    sb: &Sidebar,
    agents: &[Agent],
    renamed: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(placed) = &sb.agents else { return out };
    for a in agents {
        let sid = &a.job.session_id;
        if sid.is_empty() || renamed.contains_key(&a.key) {
            continue;
        }
        if let Some(p) = placed.get(sid)
            && !p.name.is_empty()
        {
            out.insert(a.key.clone(), p.name.clone());
        }
    }
    out
}

/// The session each codename is, by the list's names, so a message sent
/// to another session names it as the list does; a codename used again
/// goes to the session that has it now (namePeers).
pub fn peer_names(codes: &HashMap<String, String>, agents: &[Agent]) -> HashMap<String, String> {
    let mut names: HashMap<String, String> = HashMap::new();
    for a in agents {
        let Some(c) = codes.get(&a.job.session_id).filter(|c| !c.is_empty()) else {
            continue;
        };
        if names.contains_key(c) && a.pid == 0 {
            continue;
        }
        names.insert(c.clone(), a.display_name.clone());
    }
    names
}

/// The section an agent goes in under the arrangement, how high that
/// section ranks, and the agent's place in it (sidebarPlace).
pub fn place(sb: &Sidebar, a: &Agent) -> (String, usize, i64) {
    let sid = &a.job.session_id;
    if !sid.is_empty()
        && let Some(p) = sb.agents.as_ref().and_then(|m| m.get(sid))
        && let Some(i) = sb.sections.iter().position(|s| s.title == p.section)
    {
        return (sb.sections[i].title.clone(), i, p.order);
    }
    (OTHER_SECTION.to_string(), sb.sections.len(), 0)
}
