//! rush's sessions as their hosts leave them on disk, read only: each
//! session's info.json and config.json under rush's sessions folder. A
//! Hosts for until the host module lists them itself (it also revives a
//! host lost mid-turn, which this never does). Port of
//! internal/host/client.go:191-305 (readInfoFile, Lister.List), host.go:2435
//! (alive), revive.go:23 (midTurn), decisions.go:431 (dropDismissed) and
//! config.go:29 (a config's account).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use rush_core::adapter::{Kind, Profile};
use rush_core::agent::{Left, Workspace};
use rush_core::time::Ms;
use serde::Deserialize;

use crate::hosted::{Away, HostConfig, HostInfo, HostTask, Hosts, Limit, Retry, host_root};

/// Lists sessions again and again, parsing only the info files that
/// changed; every session ever started stays listed, so most are
/// unchanged.
#[derive(Default)]
pub struct InfoFiles {
    infos: HashMap<String, Listed>,
    /// The folder's sessions, as last read, and the listing that read them.
    ids: Option<Vec<String>>,
    ids_at: u64,
    last: Option<Arc<Vec<HostInfo>>>,
}

struct Listed {
    modified: Option<SystemTime>,
    size: u64,
    info: Option<HostInfo>,
    /// The listing that last looked at it.
    at: u64,
    /// Its host was found gone, as last listed.
    dead: bool,
}

impl InfoFiles {
    pub fn new() -> InfoFiles {
        InfoFiles::default()
    }
}

impl Hosts for InfoFiles {
    fn list(&mut self, fresh: &dyn Fn(&Path, u64) -> bool, at: u64) -> Arc<Vec<HostInfo>> {
        let root = host_root();
        let mut same = self.last.is_some();
        if self.ids.is_none() || !fresh(&root, self.ids_at) {
            let mut ids: Vec<String> = std::fs::read_dir(&root)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            ids.sort();
            same = same && self.ids.as_ref() == Some(&ids);
            self.ids = Some(ids);
            self.ids_at = at;
        }
        let ids = self.ids.clone().unwrap_or_default();
        let mut n = 0;
        let mut stand = Vec::new();
        for id in &ids {
            let dir = root.join(id);
            let trusted = self.infos.get(id).is_some_and(|c| fresh(&dir, c.at));
            if !trusted {
                let path = dir.join("info.json");
                let Ok(md) = std::fs::metadata(&path) else {
                    if self.infos.remove(id).is_some() {
                        same = false;
                    }
                    continue;
                };
                let modified = md.modified().ok();
                let c = self.infos.entry(id.clone()).or_insert(Listed {
                    modified: None,
                    size: u64::MAX,
                    info: None,
                    at: 0,
                    dead: false,
                });
                if c.modified != modified || c.size != md.len() {
                    c.modified = modified;
                    c.size = md.len();
                    c.info = read_info(&dir);
                    same = false;
                }
                c.at = at;
            }
            let Some(c) = self.infos.get_mut(id) else {
                continue;
            };
            let Some(info) = &c.info else { continue };
            let dead = info.state != "stopped" && !alive(info.host_pid);
            same = same && dead == c.dead;
            c.dead = dead;
            if info.is_remote() {
                stand.push(info.host_pid);
            }
            n += 1;
        }
        // The attach process behind a stand-in is no agent's to signal.
        rush_sys::proc::protect(&stand);
        if same
            && let Some(last) = &self.last
            && last.len() == n
        {
            return last.clone();
        }
        let mut out = Vec::with_capacity(n);
        for id in &ids {
            let Some(c) = self.infos.get(id) else {
                continue;
            };
            let Some(info) = &c.info else { continue };
            let mut info = info.clone();
            if c.dead {
                info.claude_pid = 0;
                // Nothing here to bring back for another machine's.
                info.lost = mid_turn(&info) && !info.is_remote();
                if !info.sleeping {
                    info.state = "stopped".into();
                }
            }
            out.push(info);
        }
        out.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
        let out = Arc::new(out);
        self.last = Some(out.clone());
        out
    }

    fn config(&self, id: &str) -> Option<HostConfig> {
        let b = std::fs::read(host_root().join(id).join("config.json")).ok()?;
        let w: WireConfig = serde_json::from_slice(&b).ok()?;
        let kind = if w.kind.is_empty() {
            "claude".to_string()
        } else {
            w.kind
        };
        Some(HostConfig {
            branches: w.branches.into_iter().map(|b| b.session_id).collect(),
            account: Profile {
                kind: Kind::from(kind.as_str()),
                name: w.account.name,
                dir: w.account.config_dir,
            },
            env: w.env,
        })
    }
}

/// A turn under way: a host gone then has gone mid-turn.
fn mid_turn(i: &HostInfo) -> bool {
    !i.sleeping && (i.state == "working" || i.state == "blocked")
}

/// Whether a host's process lives: one rush started before a #reload
/// exec'd it lingers as a zombie once it ends.
fn alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill with signal 0 only asks.
    let r = unsafe { libc::kill(pid, 0) };
    let ok = r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    ok && !rush_sys::proc::zombie(pid)
}

fn time(s: &Option<String>) -> Ms {
    s.as_deref().map_or(0, rush_core::wire::time::from_str)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireTask {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    label: String,
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireLimit {
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
    ask: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireRetry {
    attempt: i64,
    max: i64,
    #[serde(rename = "gaveUp")]
    gave_up: bool,
    why: String,
    offline: bool,
    proof: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireAway {
    until: Option<String>,
    next: Option<String>,
    #[serde(rename = "loop")]
    looping: bool,
    ended: Option<String>,
    held: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireLeft {
    path: String,
    at: Option<String>,
    shell: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireWorkspace {
    path: String,
    repo: String,
    branch: String,
    base: String,
    target: String,
    own: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireAsk {
    #[serde(rename = "Header")]
    header: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireQuestion {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Asks")]
    asks: Option<Vec<WireAsk>>,
}

/// info.json as a host writes it: the fields the list reads.
#[derive(Deserialize, Default)]
#[serde(default)]
struct WireInfo {
    id: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    account: String,
    cwd: String,
    #[serde(rename = "cwdAt")]
    cwd_at: Option<String>,
    left: Option<Vec<WireLeft>>,
    name: String,
    #[serde(rename = "hostPid")]
    host_pid: i32,
    #[serde(rename = "claudePid")]
    claude_pid: i32,
    state: String,
    detail: String,
    needs: String,
    #[serde(rename = "costUsd")]
    cost_usd: f64,
    limit: Option<WireLimit>,
    retry: Option<WireRetry>,
    error: String,
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    #[serde(rename = "idleSince")]
    idle_since: Option<String>,
    stalled: Option<String>,
    proto: i64,
    sleeping: bool,
    remote: String,
    kind: String,
    profile: String,
    meta: Option<HashMap<String, String>>,
    background: Option<Vec<WireTask>>,
    away: Option<WireAway>,
    workspace: Option<WireWorkspace>,
    decisions: Option<WireQuestion>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireBranch {
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireAccount {
    name: String,
    #[serde(rename = "configDir")]
    config_dir: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireConfig {
    kind: String,
    branches: Vec<WireBranch>,
    account: WireAccount,
    env: Vec<String>,
}

/// A session's info.json, as the list needs it; None when it can't be
/// read.
pub(crate) fn read_info(dir: &Path) -> Option<HostInfo> {
    let b = std::fs::read(dir.join("info.json")).ok()?;
    let w: WireInfo = serde_json::from_slice(&b).ok()?;
    let mut decisions = w.decisions;
    // A card you dismissed stays dismissed.
    if let Some(d) = &decisions
        && let Ok(gone) = std::fs::read_to_string(dir.join("decisions-dismissed"))
        && gone.trim() == d.id
    {
        decisions = None;
    }
    Some(HostInfo {
        id: w.id,
        session_id: w.session_id,
        account: w.account,
        cwd: w.cwd,
        cwd_at: time(&w.cwd_at),
        left: w
            .left
            .unwrap_or_default()
            .into_iter()
            .map(|l| Left {
                path: l.path,
                at: time(&l.at),
                shell: l.shell,
            })
            .collect(),
        name: w.name,
        host_pid: w.host_pid,
        claude_pid: w.claude_pid,
        state: w.state,
        detail: w.detail,
        needs: w.needs,
        cost_usd: w.cost_usd,
        limit: w.limit.map(|l| Limit {
            resets_at: time(&l.resets_at),
            ask: l.ask,
        }),
        retry: w.retry.map(|r| Retry {
            attempt: r.attempt,
            max: r.max,
            gave_up: r.gave_up,
            why: r.why,
            offline: r.offline,
            proof: r.proof,
        }),
        error: w.error,
        started_at: time(&w.started_at),
        updated_at: time(&w.updated_at),
        idle_since: time(&w.idle_since),
        stalled: time(&w.stalled),
        proto: w.proto,
        sleeping: w.sleeping,
        remote: w.remote,
        lost: false,
        // migration: older hosts wrote no kind for Claude Code.
        kind: if w.kind.is_empty() {
            "claude".into()
        } else {
            w.kind
        },
        profile: w.profile,
        meta: w.meta.unwrap_or_default(),
        background: w
            .background
            .unwrap_or_default()
            .into_iter()
            .map(|t| HostTask {
                id: t.id,
                kind: t.kind,
                label: t.label,
                started_at: time(&t.started_at),
            })
            .collect(),
        away: w.away.map(|a| Away {
            until: time(&a.until),
            next: time(&a.next),
            looping: a.looping,
            ended: time(&a.ended),
            held: a.held.map_or(0, |h| h.len()),
        }),
        workspace: w.workspace.map(|x| Workspace {
            path: x.path,
            repo: x.repo,
            branch: x.branch,
            base: x.base,
            target: x.target,
            own: x.own,
        }),
        decisions: decisions.map(|d| {
            d.asks
                .unwrap_or_default()
                .into_iter()
                .map(|a| a.header)
                .collect()
        }),
    })
}
