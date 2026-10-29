//! The host's side of rush_fleet::Hosts: the sessions' infos, as the list
//! reads them, through a Lister that reads again only what changed.

use std::path::PathBuf;

use rush_fleet::{Hosted, Hosts};

use crate::client::Lister;
use crate::decisions::needs_decisions;
use crate::paths::root;
use crate::wire::Info;

/// info as the list needs it.
pub fn hosted(i: &Info) -> Hosted {
    Hosted {
        id: i.id.clone(),
        kind: i.kind.clone(),
        session_id: i.session_id.clone(),
        account: i.account.clone(),
        profile: i.profile.clone(),
        cwd: i.cwd.clone(),
        name: i.name.clone(),
        state: i.state.clone(),
        detail: i.detail.clone(),
        needs: i.needs.clone(),
        error: i.error.clone(),
        model: i.model.clone(),
        effort: i.effort.clone(),
        permission_mode: i.permission_mode.clone(),
        host_pid: i.host_pid,
        agent_pid: i.claude_pid,
        sleeping: i.sleeping,
        remote: i.remote.clone(),
        lost: i.lost,
        started_at: ms(i.started_at),
        updated_at: ms(i.updated_at),
        idle_since: ms(i.idle_since),
        stalled: ms(i.stalled),
        cost_usd: i.cost_usd,
        context_tokens: i.context_tokens,
        started_by: i.started_by.clone(),
        spawned_by: i.meta.get("spawnedBy").cloned().unwrap_or_default(),
        spawn_step: i.meta.get("spawnStep").cloned().unwrap_or_default(),
        queued: i.queue.len(),
        tasks: i.tasks,
        background: i.background.len(),
        decisions: i
            .decisions
            .as_ref()
            .map(needs_decisions)
            .unwrap_or_default(),
        branch: i
            .workspace
            .as_ref()
            .map(|w| w.branch.clone())
            .unwrap_or_default(),
        temp_dir: i.temp_dir.clone(),
    }
}

fn ms(t: rush_state::Time) -> rush_core::time::Ms {
    if t.is_zero() { 0 } else { t.to_ms() }
}

impl Hosts for Lister {
    fn list(&mut self) -> Vec<Hosted> {
        self.list_shared().iter().map(hosted).collect()
    }

    /// The sessions' folder, where a session comes or goes, and each
    /// session's own, where its host writes its info.
    fn watched(&self) -> Vec<PathBuf> {
        let r = root();
        let mut out = vec![r.clone()];
        if let Ok(es) = std::fs::read_dir(&r) {
            out.extend(es.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    #[test]
    fn lists_hosted_sessions() {
        let _h = Home::new();
        let id = "hs1";
        std::fs::create_dir_all(crate::paths::dir(id)).unwrap();
        let mut info = Info {
            id: id.into(),
            kind: "claude".into(),
            state: "idle".into(),
            name: "Fix it".into(),
            started_at: rush_state::Time::now(),
            ..Info::default()
        };
        info.meta.insert("spawnedBy".into(), "parent".into());
        info.queue = vec!["a".into(), "b".into()];
        std::fs::write(crate::paths::dir(id).join("info.json"), info.to_json()).unwrap();
        let mut l = Lister::default();
        let got = Hosts::list(&mut l);
        assert_eq!(got.len(), 1);
        let h = &got[0];
        assert!(h.id == id && h.name == "Fix it" && h.spawned_by == "parent" && h.queued == 2);
        assert!(h.started_at > 0 && h.idle_since == 0);
        assert!(l.watched().contains(&crate::paths::dir(id)));
    }
}
