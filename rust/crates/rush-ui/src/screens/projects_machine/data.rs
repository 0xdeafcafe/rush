//! What the Projects place knows, kept between frames (Model::kept), and
//! the projects made of the agents. Go: internal/ui/projects.go (project
//! :45, projects :52, worktreeAt :97, projectTrees :108, ownCommits :128,
//! projectPRs :172), folders.go (folderCache :47, treeOf :122, rootIsRepo
//! :184), workstreams.go (workSince :22, workAgents :139), cleanup.go
//! (cleanup :29, untouched :192, dueIn :203, running :476),
//! projects_paths.go (pathInfo :32, goesAfter :305, canGo :314) and
//! tempwork.go:16 (tempShown).

use std::collections::{HashMap, HashSet};

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;
use rush_fleet::Predicates;
use rush_fleet::disk::Scratch;
use rush_fleet::folder::Folder;
use rush_fleet::machine::Machine;
use rush_fleet::worktree::Worktree;
use rush_gh::ghpr::Repo;
use serde::Deserialize;

use crate::list::{folder_key, folder_titles};
use crate::model::Model;

/// How long back an agent's work counts for the page.
pub(crate) const WORK_SINCE: Ms = 24 * 3600 * 1000;
/// Temp work smaller than this isn't shown.
pub(crate) const TEMP_SHOWN: i64 = 1 << 20;
/// How long an agent sits untouched before it can go, unless set.
pub(crate) const DEFAULT_CLEANUP: Ms = 3 * 3600 * 1000;
/// How often git is asked about the folders, at most.
pub(crate) const FOLDER_EVERY: Ms = 15_000;

/// The List's rows that aren't projects, by id.
pub(crate) const PANE_TEMP: &str = "~temp";
pub(crate) const PANE_SYSTEM: &str = "~system";
pub(crate) const PANE_PRS: &str = "~prs";

/// The settings the page reads from rush's config.json (state.Config's
/// ListLook, KeepOrphans, CleanupHours, CleanupDiskGB and
/// CleanupRAMMinutes).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Cfg {
    pub list_look: String,
    pub keep_orphans: bool,
    /// How long, in hours, done work waits before the tidy-up: 0 the
    /// default (3h), below 0 never.
    pub cleanup_hours: i64,
    /// Settings › Disk and memory: the free disk, in GB, and the minutes
    /// under memory pressure that have rush offer a cleanup agent (0 the
    /// default, -1 never).
    #[serde(rename = "cleanupDiskGB")]
    pub cleanup_disk_gb: i64,
    #[serde(rename = "cleanupRAMMinutes")]
    pub cleanup_ram_minutes: i64,
}

impl Cfg {
    pub fn load() -> Cfg {
        std::fs::read(crate::config::dir().join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The rail look, unless set to classic (rail.go:22).
    pub fn rail(&self) -> bool {
        self.list_look != "classic"
    }

    /// How long done work waits before it goes: 0 is never
    /// (state.go:425 CleanupAfter).
    pub fn cleanup_after(&self) -> Ms {
        match self.cleanup_hours {
            n if n < 0 => 0,
            0 => DEFAULT_CLEANUP,
            n => n * 3_600_000,
        }
    }
}

/// The top of a folder as last looked at: how many things, and when the
/// newest of them changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PathInfo {
    pub items: usize,
    pub modified: Ms,
    pub at: Ms,
}

/// One thing at the top of a session's temp folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TempEntry {
    pub path: String,
    pub modified: Ms,
    pub dir: bool,
}

/// An agent's temp work: its folders, and what they hold, as last measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TempWork {
    pub dirs: Vec<rush_core::adapter::TempDir>,
    pub bytes: i64,
}

/// Everything the page knows, kept between frames.
#[derive(Default)]
pub(crate) struct Data {
    /// Set by tests and --render: nothing is asked of git, GitHub or the disk.
    pub frozen: bool,
    /// A row to pick in the list, asked from outside (pickInProjects), and
    /// the row picked as last seen.
    pub pick: Option<String>,
    pub sel: String,
    /// When Clean up was last suggested (nudgeClean).
    pub nudged: Ms,
    pub cfg: Cfg,
    pub cfg_read: bool,
    /// What git last said of each repository, and how many it's still
    /// being asked about.
    pub folders: HashMap<String, Folder>,
    pub looking: usize,
    pub folders_asked: Ms,
    /// Clean-up's look at every worktree.
    pub wts: Vec<Worktree>,
    pub checking: bool,
    pub checked: Ms,
    pub tmp: Scratch,
    /// Each project's agent tmp folders' sizes, by path.
    pub agent_tmp: HashMap<String, i64>,
    /// Each agent's temp work, by key.
    pub temp: HashMap<String, TempWork>,
    pub measuring: bool,
    /// The machine's agent processes, as last read.
    pub machine: Machine,
    pub machine_at: Ms,
    pub reading: bool,
    pub prev_table: Option<rush_sys::proc::Table>,
    /// What runs under the agents, to name what they leave (out with a
    /// reading of the machine), and when each orphan was first seen.
    pub reaper: Option<rush_fleet::reap::Reaper>,
    pub orphans: super::tidy::Orphans,
    /// Pull requests by repository.
    pub prs: HashMap<String, Repo>,
    pub asking: HashSet<String>,
    /// Merges asked for, by "root#n": how, while it's out; why the last
    /// failed (and whether as an admin gets past it); those done.
    pub merging: HashMap<String, String>,
    pub merge_failed: HashMap<String, (String, bool)>,
    pub merged: HashSet<String>,
    /// Folders' tops, opened temp work, and sizes of things in it.
    pub peeks: HashMap<String, PathInfo>,
    pub opened: HashSet<String>,
    pub lists: HashMap<String, (Vec<TempEntry>, Ms)>,
    pub sizes: HashMap<String, i64>,
}

impl Data {
    /// What the scan knows of the worktree at path, or a bare unchecked
    /// one before it has looked.
    pub fn worktree_at(&self, path: &str) -> Worktree {
        self.wts
            .iter()
            .find(|w| w.path == path)
            .cloned()
            .unwrap_or_else(|| {
                let mut w = Worktree::default();
                w.path = path.to_string();
                w
            })
    }

    /// An agent's temp work, as last measured (0 before).
    pub fn temp_of(&self, key: &str) -> i64 {
        self.temp.get(key).map_or(0, |t| t.bytes)
    }
}

/// The page's state as the model holds it; the default before the place
/// has ever been opened.
pub(crate) fn data(m: &Model) -> &Data {
    static EMPTY: std::sync::LazyLock<Data> = std::sync::LazyLock::new(Data::default);
    m.kept.peek::<Data>().unwrap_or(&EMPTY)
}

/// An agent with the main checkout of the repository it works in ("" when
/// it isn't in one).
#[derive(Clone, Copy)]
pub(crate) struct Ag<'a> {
    pub a: &'a Agent,
    pub root: &'a str,
}

impl<'a> Ag<'a> {
    /// The folder its row sits under (folders.go:68).
    pub fn key(&self) -> &'a str {
        folder_key(self.a, self.root)
    }

    /// The linked worktree it works in, "" for the main checkout.
    pub fn tree(&self) -> &'a str {
        if !self.root.is_empty() && self.a.repo != self.root {
            &self.a.repo
        } else {
            ""
        }
    }

    pub fn live(&self) -> bool {
        self.a.job.live()
    }

    /// A live process, or work in the background.
    pub fn open(&self) -> bool {
        self.a.job.open() || self.a.pid != 0
    }

    pub fn busy(&self) -> bool {
        self.a.busy()
    }

    pub fn waiting(&self) -> bool {
        self.a.job.state == State::Blocked
    }

    /// Working, or open and running: its worktree stays (worktree.go:210).
    pub fn in_use(&self) -> bool {
        self.live() || self.busy() || self.a.pid != 0
    }
}

/// Every agent, with its main checkout.
pub(crate) fn agents(m: &Model) -> impl Iterator<Item = Ag<'_>> {
    m.agents.iter().enumerate().map(|(i, a)| Ag {
        a,
        root: m.roots.get(i).map_or("", String::as_str),
    })
}

/// The sessions worth reading: open, busy or at work within the window.
pub(crate) fn work_agents(m: &Model) -> Vec<Ag<'_>> {
    agents(m)
        .filter(|g| {
            !g.a.past
                && (g.live() || g.busy() || g.a.pid != 0 || m.now - g.a.job.updated_at < WORK_SINCE)
        })
        .collect()
}

/// One repository on the page, and its agents.
#[derive(Clone)]
pub(crate) struct Project<'a> {
    pub key: String,
    pub title: String,
    pub agents: Vec<Ag<'a>>,
    /// An agent in it is running.
    pub open: bool,
    /// A repository, not just a folder.
    pub repo: bool,
}

/// The projects agents worked in over the last day: repositories first,
/// those with an agent running first, then by name.
pub(crate) fn projects(m: &Model) -> Vec<Project<'_>> {
    let order: HashMap<&str, usize> = m
        .order
        .iter()
        .enumerate()
        .filter_map(|(n, &i)| m.agents.get(i).map(|a| (a.key.as_str(), n)))
        .collect();
    let mut by: Vec<Project> = Vec::new();
    for g in work_agents(m) {
        let k = g.key();
        let p = match by.iter_mut().position(|p| p.key == k) {
            Some(i) => &mut by[i],
            None => {
                by.push(Project {
                    key: k.to_string(),
                    title: String::new(),
                    agents: Vec::new(),
                    open: false,
                    repo: false,
                });
                by.last_mut().unwrap()
            }
        };
        p.open = p.open || g.open() || g.busy();
        p.agents.push(g);
    }
    let titles = folder_titles(by.iter().map(|p| p.key.as_str()));
    for p in &mut by {
        p.title = titles.get(&p.key).cloned().unwrap_or_default();
        p.repo = p.key.starts_with('/') && p.agents.iter().any(|g| g.root == p.key);
        p.agents.sort_by(|a, b| {
            a.tree().cmp(b.tree()).then(
                order
                    .get(a.a.key.as_str())
                    .unwrap_or(&usize::MAX)
                    .cmp(order.get(b.a.key.as_str()).unwrap_or(&usize::MAX)),
            )
        });
    }
    by.sort_by(|a, b| {
        b.repo
            .cmp(&a.repo)
            .then(b.open.cmp(&a.open))
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    by
}

/// A project's linked worktrees: those its agents are in, then the rest
/// git lists.
pub(crate) fn project_trees(p: &Project, f: Option<&Folder>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in &p.agents {
        let t = g.tree();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    for t in f.map(|f| f.linked.as_slice()).unwrap_or_default() {
        if !out.contains(t) {
            out.push(t.clone());
        }
    }
    out
}

/// A worktree's commits past what it came from, or past its upstream.
pub(crate) fn own_commits(st: &rush_fleet::folder::GitState) -> usize {
    if st.base.is_empty() {
        st.ahead
    } else {
        st.base_ahead
    }
}

/// The agent with key, and its root.
pub(crate) fn agent_by_key<'a>(m: &'a Model, key: &str) -> Option<Ag<'a>> {
    agents(m).find(|g| g.a.key == key)
}

/// The first agent in keys with a process.
pub(crate) fn running<'a>(m: &'a Model, keys: &[String]) -> Option<Ag<'a>> {
    keys.iter()
        .filter_map(|k| agent_by_key(m, k))
        .find(|g| g.a.pid != 0)
}

/// How long since an agent was last active.
pub(crate) fn untouched(g: &Ag, now: Ms) -> Ms {
    g.a.age(now)
}

/// How long an agent sits untouched before it can go.
pub(crate) fn goes_after(d: &Data) -> Ms {
    match d.cfg.cleanup_after() {
        0 => DEFAULT_CLEANUP,
        n => n,
    }
}

/// A project's agent that's done with: stopped, untouched for goes_after,
/// and leaving no work only its worktree has.
pub(crate) fn can_go(d: &Data, g: &Ag, now: Ms) -> bool {
    if g.a.pid != 0 || g.live() || g.busy() || g.a.needs_you() || untouched(g, now) < goes_after(d)
    {
        return false;
    }
    let t = g.tree();
    if !t.is_empty() {
        return d.worktree_at(t).verdict().can_go();
    }
    true
}

/// Whether a is Earlier's: left a day without a turn, neither open nor at
/// work, and asking nothing (model.go:2612 stale, :2617 earlier). What
/// you put away by hand (the overlay's Done) isn't read here yet.
pub(crate) fn earlier(a: &Agent, now: Ms) -> bool {
    !a.job.open() && !a.busy() && a.age(now) >= 24 * 3_600_000 && !crate::list::asks(a)
}

/// How long until done work of these agents goes by itself: zero when
/// due now, None when it never will.
pub(crate) fn due_in(m: &Model, d: &Data, keys: &[String], now: Ms) -> Option<Ms> {
    let after = d.cfg.cleanup_after();
    if after == 0 || keys.is_empty() {
        return None;
    }
    let mut due = 0;
    for k in keys {
        let g = agent_by_key(m, k)?;
        if g.a.pid != 0 || !earlier(g.a, now) {
            return None;
        }
        due = due.max(after - untouched(&g, now));
    }
    Some(due.max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_hours() {
        let c = |n| Cfg {
            cleanup_hours: n,
            ..Cfg::default()
        };
        assert_eq!(c(-1).cleanup_after(), 0);
        assert_eq!(c(0).cleanup_after(), DEFAULT_CLEANUP);
        assert_eq!(c(5).cleanup_after(), 5 * 3_600_000);
        let read: Cfg = serde_json::from_str(
            r#"{"listLook":"classic","keepOrphans":true,"cleanupHours":-1,"cleanupDiskGB":40,"cleanupRAMMinutes":5}"#,
        )
        .unwrap();
        assert!(!read.rail() && read.keep_orphans);
        assert_eq!(
            (
                read.cleanup_after(),
                read.cleanup_disk_gb,
                read.cleanup_ram_minutes
            ),
            (0, 40, 5)
        );
    }
}
