//! A plugin's arrangement of the agent list, group-by plugin:<name>: its
//! sections replace rush's, each agent where the plugin places it, the
//! rest under Other. After Go's internal/ui/sidebar.go:124 (sidebarPlace)
//! and model.go:2268-2274 (rebuild's sidebar case) and :2353 (its rows in
//! the plugin's order).

use rush_core::agent::Agent;
use rush_plugin::Sidebar;

/// Where the agents it doesn't place go.
pub const OTHER_SECTION: &str = "Other";

/// The arrangement the list is grouped by now, None for rush's own: set
/// by the plugin layer at the start of each rebuild (on_rebuild's
/// nameAgents), kept with the model (`Model::kept`).
#[derive(Default)]
pub struct Arranged(pub Option<Sidebar>);

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
