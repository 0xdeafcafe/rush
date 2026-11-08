//! A plugin with the sidebar capability arranges the agent list its own
//! way: it offers a group-by mode, plugin:<name>, whose sections are the
//! plugin's and whose rows carry the names it gives. Agents it doesn't
//! place go to Other, folded. After Go's internal/ui/sidebar.go
//! (groupModes :25, groupLabel :34, activeSidebar :47, loadSidebars :62,
//! nameAgents :73, namePeers :98, sidebarPlace :124).

use std::collections::{BTreeMap, HashMap};

use rush_core::agent::Agent;
use rush_plugin::Sidebar;

use crate::ModelPlugins;
use crate::frame::Out;
use crate::model::Model;

pub use rush_ui_base::arranged::{OTHER_SECTION, place};

pub const PLUGIN_GROUP_PREFIX: &str = "plugin:";

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
            (files, s, disk.rush_names())
        },
        |m, (files, s, names)| {
            let p = m.plugins();
            p.sidebar_files = files;
            if p.sidebars == s && p.rush_names == names {
                return Out::Idle;
            }
            p.sidebars = s;
            p.rush_names = names;
            Out::Draw
        },
    );
}

/// The names the arrangement gives agents, by key, except where you
/// renamed one in rush (nameAgents): `renamed` holds the names you gave in rush, by key.
pub fn agent_names(
    sb: &Sidebar,
    agents: &[Agent],
    renamed: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(placed) = &sb.agents else { return out };
    for a in agents {
        let sid = &a.job.session_id;
        if sid.is_empty() || renamed.get(&a.key).is_some_and(|n| !n.is_empty()) {
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

/// Gives the agents the names the arrangement in use places, except where
/// you renamed one in rush, and gives back the names it took over from an
/// arrangement no longer in use; then tells the conversation view the
/// peers' names, as the list now has them (nameAgents, sidebar.go:73-95,
/// its deferred namePeers run last). Agents are replaced by each reading
/// of the fleet, so a name is given back only where the agent still has
/// the name the arrangement gave it.
pub fn name_agents(m: &mut Model) {
    let sb = if (m.ext.calls.hosted_alone())(m) {
        None // hosted alone shows one session and is never arranged
    } else {
        let by = m.cfg.group_by.clone();
        active(&m.plugins().sidebars, &by).cloned()
    };
    let p = m.plugins();
    let old = std::mem::take(&mut p.renamed);
    let names = p.rush_names.clone();
    for a in m.agents.iter_mut() {
        if let Some((was, gave)) = old.get(&a.key)
            && a.display_name == *gave
        {
            a.display_name.clone_from(was);
        }
    }
    let mut renamed = BTreeMap::new();
    if let Some(sb) = &sb {
        let given = agent_names(sb, &m.agents, &names);
        for a in m.agents.iter_mut() {
            if let Some(n) = given.get(&a.key) {
                renamed.insert(a.key.clone(), (a.display_name.clone(), n.clone()));
                a.display_name.clone_from(n);
            }
        }
    }
    m.plugins().renamed = renamed;
    name_peers(&m.agents);
    // The list's sections are the arrangement's (rebuild's sidebarPlace).
    m.kept::<rush_ui_base::arranged::Arranged>().0 = sb;
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

/// Tells the conversation view which session each codename is, from every
/// adapter's codenames and the list as it stands (sidebar.go:98 namePeers,
/// run at the end of nameAgents).
pub fn name_peers(agents: &[Agent]) {
    let mut codes: HashMap<String, String> = HashMap::new();
    for r in rush_core::adapter::global().all().iter() {
        if let Some(c) = r.codenamer() {
            codes.extend(c.codenames()); // sidebar.go:101-110
        }
    }
    rush_convo::set_peers(peer_names(&codes, agents));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(sid: &str, pid: i32, name: &str) -> Agent {
        let mut a = Agent::default();
        a.job.session_id = sid.into();
        a.pid = pid;
        a.display_name = name.into();
        a
    }

    /// A codename used again goes to the session that has it now
    /// (sidebar.go:98 namePeers).
    #[test]
    fn sidebar_peer_names_a_live_session_wins_a_reused_codename() {
        let codes: HashMap<String, String> = [("old", "rush-8a"), ("new", "rush-8a")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let got = peer_names(&codes, &[agent("new", 42, "Live"), agent("old", 0, "Gone")]);
        assert_eq!(got.get("rush-8a").map(String::as_str), Some("Live"));
        let got = peer_names(&codes, &[agent("old", 0, "Gone"), agent("new", 42, "Live")]);
        assert_eq!(got.get("rush-8a").map(String::as_str), Some("Live"));
    }
}
