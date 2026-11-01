//! Where a new session starts: the selected agent's folder, unless a
//! folder was picked for new sessions while it was selected; else the
//! one picked, or where rush was opened. The Agents box (screens/
//! list_extras/prompt.rs) picks; every screen that starts a session asks.
//! After Go's internal/ui/model.go (startDirs :627, readStartDirs :666,
//! startDir :708, followDir :721, workDir :735, inTree :746, pickDir
//! :769) and picker.go:125 (agentDir).

use rush_core::agent::Agent;

use crate::list::is_temp;
use crate::model::Model;

/// An agent working in a linked worktree (inTree): its checkout's root
/// isn't its repository.
pub fn in_tree(a: &Agent, root: &str) -> bool {
    !root.is_empty() && !a.repo.is_empty() && a.repo != root
}

/// Where an agent works now, as far as its transcript says (agentDir).
pub fn agent_dir(a: &Agent) -> &str {
    if a.spend.dir.is_empty() {
        &a.job.cwd
    } else {
        &a.spend.dir
    }
}

/// Whether d is somewhere to start work: not a temp folder, nor rush's
/// or an agent's own state (workDir).
pub fn work_dir(d: &str) -> bool {
    if d.is_empty() || is_temp(d) {
        return false;
    }
    // Asked once: finding rush's folder looks at the disk, and this runs
    // every frame.
    static STATE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let state = STATE.get_or_init(crate::config::dir);
    if std::path::Path::new(d).starts_with(state) {
        return false;
    }
    !d.contains("/projects/-")
}

/// The folder a new session takes from agent i: the top of its
/// repository, not the subfolder it's in; for one in a linked worktree,
/// the checkout it was made from, or with alt+l the worktree (followDir).
pub fn follow_dir(m: &Model, i: usize) -> String {
    let a = &m.agents[i];
    let root = m.roots.get(i).map_or("", String::as_str);
    if in_tree(a, root) && !start_in_tree(m) {
        return root.to_string();
    }
    [a.repo.as_str(), agent_dir(a), a.job.cwd.as_str()]
        .into_iter()
        .find(|d| work_dir(d))
        .unwrap_or("")
        .to_string()
}

/// The folder picked for new sessions (ctrl+l, Go's m.pickedDir,
/// m.dirIdx, m.pickedFor): one typed is offered first, and the pick
/// holds while the agent selected as it was made stays selected.
#[derive(Default)]
pub struct StartPick {
    /// A folder typed for new sessions, offered first.
    pub dir: String,
    /// Which of start_dirs was picked.
    pub idx: usize,
    /// The agent selected when it was picked; "" for none.
    pub for_key: String,
    /// It was picked at all.
    pub set: bool,
    /// alt+l: new sessions follow a selected worktree agent into its
    /// worktree, not its main checkout (Go's m.startInTree).
    pub in_tree: bool,
}

/// alt+l is on: new sessions follow a worktree agent into its worktree.
pub fn start_in_tree(m: &Model) -> bool {
    m.kept.peek::<StartPick>().is_some_and(|p| p.in_tree)
}

/// The folders a new session can start in: a folder typed for them, where
/// rush was opened, then those of agents open, then of the latest, twelve
/// at most (readStartDirs).
pub fn start_dirs(m: &Model) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(p) = m.kept.peek::<StartPick>().filter(|p| !p.dir.is_empty()) {
        out.push(p.dir.clone());
    }
    if !m.launch_dir.is_empty() && !out.contains(&m.launch_dir) {
        out.push(m.launch_dir.clone());
    }
    let mut agents: Vec<&Agent> = m.agents.iter().collect();
    agents.sort_by(|a, b| {
        b.job
            .open()
            .cmp(&a.job.open())
            .then(b.job.updated_at.cmp(&a.job.updated_at))
    });
    for a in agents {
        if out.len() >= 12 {
            break;
        }
        if (a.interactive || a.past) && is_temp(&a.job.cwd) {
            continue;
        }
        let d = &a.job.cwd;
        if !d.is_empty() && !out.contains(d) {
            out.push(d.clone());
        }
    }
    out
}

/// Where a new session starts: the selected agent's folder, unless a
/// folder was picked while it was selected; else the one picked, or
/// where rush was opened (startDir, pickDir).
pub fn start_dir(m: &Model) -> String {
    let pick = m.kept.peek::<StartPick>();
    if let Some(i) = m.focused()
        && pick.is_none_or(|p| !p.set || p.for_key != m.agents[i].key)
    {
        let d = follow_dir(m, i);
        if !d.is_empty() {
            return d;
        }
    }
    let dirs = start_dirs(m);
    if dirs.is_empty() {
        return String::new();
    }
    dirs[pick.map_or(0, |p| p.idx) % dirs.len()].clone()
}

/// The project dir is in: its repository's main checkout, or dir when
/// it's in none. Looks at the disk (stats only, no git).
pub fn project_of(dir: &str) -> String {
    rush_fleet::main_checkout(dir)
}

/// Whether project is a git repository's checkout (its .git is there).
pub fn is_repo(project: &str) -> bool {
    !project.is_empty() && std::path::Path::new(project).join(".git").exists()
}

/// Every project rush knows, each once: the one a new session would start
/// in (box, its main checkout) first, then the repositories agents work
/// in, those with a setting of their own and where rush was opened, by
/// name.
pub fn known_projects(m: &Model, box_project: &str) -> Vec<String> {
    let mut rest: Vec<String> = Vec::new();
    let mut add = |p: &str| {
        if p.starts_with('/') && p != box_project && !rest.iter().any(|r| r == p) {
            rest.push(p.to_string());
        }
    };
    for r in &m.roots {
        if work_dir(r) {
            add(r);
        }
    }
    for p in crate::accounts::cfg(m).own_worktrees.keys() {
        add(p);
    }
    // Where rush was opened, when it's a repository.
    if work_dir(&m.launch_dir) {
        let p = project_of(&m.launch_dir);
        if is_repo(&p) {
            add(&p);
        }
    }
    sort_projects(box_project, rest)
}

/// projects by name as the list titles them (a folder's name, more of
/// its path when two share one), first ahead of them when it's set.
pub fn sort_projects(first: &str, mut projects: Vec<String>) -> Vec<String> {
    projects.retain(|p| p != first);
    let titles = crate::list::folder_titles(projects.iter().map(String::as_str));
    projects.sort_by_cached_key(|p| {
        (
            titles.get(p).cloned().unwrap_or_default().to_lowercase(),
            p.clone(),
        )
    });
    if !first.is_empty() {
        projects.insert(0, first.to_string());
    }
    projects
}
