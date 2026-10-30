//! The host's side of rush_fleet::Hosts: the sessions' infos, as the list
//! reads them, through a Lister that reads again only what changed; their
//! configs; and bringing back a session whose host went away mid-turn, or
//! with its background agents at work.
//! Port of internal/host/revive.go:79-99 (Revive; the rest is revive.rs)
//! and the host.List / ReadConfig calls of internal/fleet/fleet.go:706.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rush_core::agent::{Left, Workspace};
use rush_core::time::Ms;
use rush_fleet::{Away, HostConfig, HostInfo, HostTask, Hosts, Limit, Retry};

use crate::client::{Lister, ensure_with, read_config};
use crate::events::log_event;
use crate::paths::dir;
pub use crate::revive::REVIVE_NOTE;
use crate::revive::revive_text;
use crate::wire::Info;

/// How recently a lost session must have been heard from to be revived.
const REVIVE_WITHIN: Ms = 24 * 60 * 60 * 1000;

/// A listing, and what it was made into for the list.
type Made = (Arc<Vec<Info>>, Arc<Vec<HostInfo>>);

/// rush's sessions, as the list reads them.
#[derive(Default)]
pub struct Sessions {
    lister: Lister,
    /// The last listing and what it was made into, shared again while the
    /// listing is.
    last: Option<Made>,
    /// Whether the list is a rush's that follows another, asked each time:
    /// it brings back no lost session then, leaving that to the rush that
    /// leads (rush-ui's lead.rs). None: it never follows.
    follows: Option<fn() -> bool>,
}

impl Sessions {
    pub fn new() -> Sessions {
        Sessions::default()
    }

    /// rush's sessions as a rush that follows reads them: a lost one is
    /// shown stopped, never started again from here.
    pub fn following() -> Sessions {
        Sessions {
            follows: Some(|| true),
            ..Sessions::default()
        }
    }

    /// rush's sessions as a rush reads them that follows while follows
    /// says so: one over SSH, which leads only while no other view does.
    pub fn following_while(follows: fn() -> bool) -> Sessions {
        Sessions {
            follows: Some(follows),
            ..Sessions::default()
        }
    }
}

impl Hosts for Sessions {
    fn list(&mut self, fresh: &dyn Fn(&Path, u64) -> bool, at: u64) -> Arc<Vec<HostInfo>> {
        let infos = self.lister.list_trusting(fresh, at);
        if let Some((src, out)) = &self.last
            && Arc::ptr_eq(src, &infos)
        {
            return out.clone();
        }
        let out = Arc::new(infos.iter().map(host_info).collect::<Vec<_>>());
        self.last = Some((infos, out.clone()));
        out
    }

    fn config(&self, id: &str) -> Option<HostConfig> {
        let c = read_config(id).ok()?;
        Some(HostConfig {
            branches: c.branches.into_iter().map(|b| b.session_id).collect(),
            account: c.account,
            env: c.env,
        })
    }

    fn revive(&self, info: &HostInfo) {
        if self.follows.is_some_and(|f| f()) {
            return;
        }
        let info = info.clone();
        std::thread::spawn(move || {
            let _ = revive(&info);
        });
    }
}

/// Starts a lost session's host again on the same conversation and tells
/// it to carry on, so it never sits stopped mid-turn. At most once every
/// ten minutes per session, so an agent that keeps taking its host down
/// isn't restarted in a loop. Never on the UI thread.
pub fn revive(info: &HostInfo) -> crate::client::Result<()> {
    if !info.lost || rush_core::time::now() - info.updated_at > REVIVE_WITHIN || cfg!(test) {
        return Ok(()); // a test's records are never started with the test binary
    }
    let cfg = read_config(&info.id)?;
    if cfg.owner > 0 {
        return Ok(()); // a stand-in's run ended with the program that started it
    }
    let mark = dir(&info.id).join("revived");
    if let Ok(m) = std::fs::metadata(&mark).and_then(|m| m.modified())
        && SystemTime::now()
            .duration_since(m)
            .is_ok_and(|d| d < Duration::from_secs(600))
    {
        return Ok(());
    }
    if rush_core::dryrun::would(
        "revive-host",
        &info.id,
        &format!(
            "its host was lost while {}: started again and told to carry on",
            info.state
        ),
    ) {
        return Ok(());
    }
    std::fs::write(&mark, b"")?;
    // The list's info has no background agents left on it (lose cleared
    // them): the reading again names those that went with its host.
    let cut = crate::client::read_info(&info.id)
        .map(|i| i.cut_agents)
        .unwrap_or_default();
    let agents = if cut.is_empty() {
        serde_json::Value::Null // Go's nil slice
    } else {
        serde_json::json!(cut)
    };
    log_event(
        &info.id,
        "revived",
        serde_json::json!({ "lastState": info.state, "agents": agents }),
    );
    let text = revive_text(&Info {
        cut_agents: cut,
        ..Default::default()
    });
    ensure_with(&info.id, &text, "")
}

fn ms(t: rush_state::Time) -> Ms {
    if t.is_zero() { 0 } else { t.to_ms() }
}

/// info as the list reads it.
pub fn host_info(i: &Info) -> HostInfo {
    HostInfo {
        id: i.id.clone(),
        session_id: i.session_id.clone(),
        account: i.account.clone(),
        cwd: i.cwd.clone(),
        cwd_at: ms(i.cwd_at),
        left: i
            .left
            .iter()
            .map(|l| Left {
                path: l.path.clone(),
                at: ms(l.at),
                shell: l.shell,
            })
            .collect(),
        name: i.name.clone(),
        host_pid: i.host_pid as i32,
        claude_pid: i.claude_pid as i32,
        state: i.state.clone(),
        detail: i.detail.clone(),
        needs: i.needs.clone(),
        cost_usd: i.cost_usd,
        limit: i.limit.as_ref().map(|l| Limit {
            resets_at: ms(l.resets_at),
            ask: l.ask,
        }),
        retry: i.retry.as_ref().map(|r| Retry {
            attempt: r.attempt,
            max: r.max,
            gave_up: r.gave_up,
            why: r.why.clone(),
            offline: r.offline,
            proof: r.proof,
        }),
        error: i.error.clone(),
        started_at: ms(i.started_at),
        updated_at: ms(i.updated_at),
        idle_since: ms(i.idle_since),
        stalled: ms(i.stalled),
        proto: i.proto,
        sleeping: i.sleeping,
        remote: i.remote.clone(),
        lost: i.lost,
        kind: if i.kind.is_empty() {
            "claude".into() // migration: older hosts wrote no kind for Claude Code
        } else {
            i.kind.clone()
        },
        profile: i.profile.clone(),
        started_by: i.started_by.clone(),
        queue_stuck: ms(i.queue_stuck),
        meta: i.meta.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        background: i
            .background
            .iter()
            .map(|t| HostTask {
                id: t.id.clone(),
                kind: t.kind.clone(),
                label: t.label.clone(),
                started_at: ms(t.started_at),
            })
            .collect(),
        away: i.away.as_ref().map(|a| Away {
            until: ms(a.until),
            next: ms(a.next),
            looping: a.r#loop,
            ended: ms(a.ended),
            held: a.held.len(),
        }),
        workspace: i.workspace.as_ref().map(|w| Workspace {
            path: w.path.clone(),
            repo: w.repo.clone(),
            branch: w.branch.clone(),
            base: w.base.clone(),
            target: w.target.clone(),
            own: w.own,
        }),
        decisions: i
            .decisions
            .as_ref()
            .map(|d| d.asks.iter().map(|a| a.header.clone()).collect()),
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
        std::fs::create_dir_all(dir(id)).unwrap();
        let mut info = Info {
            id: id.into(),
            kind: "claude".into(),
            state: "idle".into(),
            name: "Fix it".into(),
            started_at: rush_state::Time::now(),
            ..Info::default()
        };
        info.meta.insert("spawnedBy".into(), "parent".into());
        std::fs::write(dir(id).join("info.json"), info.to_json()).unwrap();
        let mut s = Sessions::new();
        let got = s.list(&|_, _| false, 1);
        assert_eq!(got.len(), 1);
        let h = &got[0];
        assert!(h.id == id && h.name == "Fix it" && h.meta["spawnedBy"] == "parent");
        assert!(h.started_at > 0 && h.idle_since == 0);
        // Nothing changed: the same listing, shared.
        let again = s.list(&|_, _| true, 2);
        assert!(Arc::ptr_eq(&got, &again));
        assert!(s.config("gone").is_none());
    }
}
