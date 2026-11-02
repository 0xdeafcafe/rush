//! A git repository's address typed in the Agents box: rush offers to
//! start the agent in it. A clone among the projects rush knows (matched
//! by its remote, not its folder's name) is a tab away; with none, tab
//! clones it where the others are (~/Source/<host>/<owner>/<repo>) and
//! moves there. Nothing is cloned or moved without the key, and the
//! address stays in the task. Rush-only: Go had no such offer.

use std::collections::HashMap;
use std::path::Path;

use rush_core::text::{Line, width};

use super::prompt::set_start_dir;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::startdir::{known_projects, project_of, start_dir};
use crate::style::*;

/// A repository by where it's hosted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Repo {
    pub host: String,
    pub owner: String,
    pub name: String,
    /// Written as an ssh address (git@host:owner/repo, ssh://).
    pub ssh: bool,
}

impl Repo {
    /// The same repository, however it was written.
    pub fn same(&self, o: &Repo) -> bool {
        self.host.eq_ignore_ascii_case(&o.host)
            && self.owner.eq_ignore_ascii_case(&o.owner)
            && self.name.eq_ignore_ascii_case(&o.name)
    }

    /// host/owner/repo.
    pub fn short(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }

    /// The address to clone it from: ssh or https.
    pub fn url(&self, ssh: bool) -> String {
        if ssh {
            format!("git@{}:{}/{}.git", self.host, self.owner, self.name)
        } else {
            format!("https://{}/{}/{}.git", self.host, self.owner, self.name)
        }
    }
}

/// Hosts whose addresses count without https:// or .git.
const HOSTS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "git.sr.ht",
];

/// A host's first path parts that are its own pages, not an owner.
const NOT_OWNERS: &[&str] = &[
    "orgs",
    "settings",
    "features",
    "marketplace",
    "topics",
    "sponsors",
    "apps",
    "notifications",
    "explore",
    "login",
    "about",
    "pricing",
    "collections",
    "enterprise",
    "search",
    "issues",
    "pulls",
    "users",
    "trending",
];

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
}

/// word as a repository's address, or None. hosts are hosts besides the
/// usual ones whose addresses count without https:// or .git (those the
/// projects here come from). Any /tree/branch, /pull/N or other part
/// after owner/repo is left out.
pub fn parse(word: &str, hosts: &[String]) -> Option<Repo> {
    let w = word
        .trim_start_matches(['<', '(', '[', '{', '"', '\'', '`'])
        .trim_end_matches([
            '>', ')', ']', '}', '"', '\'', '`', '.', ',', ';', ':', '!', '?',
        ]);
    let known = |h: &str| {
        HOSTS.iter().any(|k| k.eq_ignore_ascii_case(h))
            || hosts.iter().any(|k| k.eq_ignore_ascii_case(h))
    };
    let (host, path, ssh, explicit) = if let Some((scheme, rest)) = w.split_once("://") {
        let ssh = match scheme.to_ascii_lowercase().as_str() {
            "https" | "http" | "git" => false,
            "ssh" | "git+ssh" | "ssh+git" => true,
            _ => return None,
        };
        let (auth, path) = rest.split_once('/')?;
        let host = auth.rsplit('@').next().unwrap_or(auth);
        let host = host.split(':').next().unwrap_or(host);
        (host, path, ssh, ssh || scheme.eq_ignore_ascii_case("git"))
    } else if let Some((user_host, path)) = w.split_once(':')
        && let Some((user, host)) = user_host.split_once('@')
        && !user.is_empty()
        && !user.contains('/')
        && !path.starts_with('/')
    {
        // scp's form: git@host:owner/repo.git.
        (host, path, true, true)
    } else {
        let (host, path) = w.split_once('/')?;
        (host, path, false, false)
    };
    if host.is_empty() || (!host.contains('.') && !explicit) {
        return None;
    }
    let mut parts = path.split(['/', '?', '#']).filter(|p| !p.is_empty());
    let owner = parts.next()?;
    let raw = parts.next()?;
    let name = raw.strip_suffix(".git").unwrap_or(raw);
    let dot_git = name.len() != raw.len();
    if !(explicit || dot_git || known(host)) {
        return None;
    }
    if !name_ok(owner)
        || !name_ok(name)
        || NOT_OWNERS.contains(&owner.to_ascii_lowercase().as_str())
    {
        return None;
    }
    Some(Repo {
        host: host.to_ascii_lowercase(),
        owner: owner.to_string(),
        name: name.to_string(),
        ssh,
    })
}

/// The first repository's address in text, if any.
pub fn find(text: &str, hosts: &[String]) -> Option<Repo> {
    text.split_whitespace().find_map(|w| parse(w, hosts))
}

/// The remotes of the checkout at dir, as its .git/config names them:
/// read from the file, no git run.
pub fn remotes(dir: &str) -> Vec<Repo> {
    let Ok(cfg) = std::fs::read_to_string(Path::new(dir).join(".git").join("config")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_remote = false;
    for l in cfg.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            in_remote = t.starts_with("[remote ");
            continue;
        }
        if !in_remote {
            continue;
        }
        if let Some((k, v)) = t.split_once('=')
            && k.trim() == "url"
            && let Some(r) = parse(v.trim(), &[])
        {
            out.push(r);
        }
    }
    out
}

/// Where the projects here are kept, as their paths say: the folder
/// that holds <host>/<owner>/<repo> for most of them.
pub fn layout_base(projects: &[(String, Vec<Repo>)]) -> Option<String> {
    let mut count: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (path, rs) in projects {
        let low = path.to_ascii_lowercase();
        for r in rs {
            let tail = format!("/{}", r.short()).to_ascii_lowercase();
            if low.ends_with(&tail) && path.len() > tail.len() {
                let base = path[..path.len() - tail.len()].to_string();
                if !count.contains_key(&base) {
                    order.push(base.clone());
                }
                *count.entry(base).or_default() += 1;
                break;
            }
        }
    }
    let most = order.iter().map(|b| count[b]).max()?;
    order.into_iter().find(|b| count[b] == most)
}

/// What the box offers for a repository typed in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    /// A clone of it is here: tab moves the box there.
    Move { repo: Repo, dir: String },
    /// None is: tab clones it into dir from url, then moves there.
    Clone {
        repo: Repo,
        url: String,
        dir: String,
    },
}

impl Offer {
    pub fn dir(&self) -> &str {
        match self {
            Offer::Move { dir, .. } | Offer::Clone { dir, .. } => dir,
        }
    }
}

/// What the box offers for repo, given the projects here with their
/// remotes and the project the box points at now; None when the box is
/// already there, or the place a clone would go is taken by something
/// else. dest_remotes reads what's at the place a clone would go.
pub fn offer_for(
    repo: &Repo,
    projects: &[(String, Vec<Repo>)],
    here: &str,
    home: &str,
    dest_remotes: impl Fn(&str) -> Option<Vec<Repo>>,
) -> Option<Offer> {
    let has = |rs: &[Repo]| rs.iter().any(|r| r.same(repo));
    // The box's own first, then the others in order.
    let mut found: Option<&str> = None;
    for (p, rs) in projects {
        if has(rs) {
            if p == here {
                return None;
            }
            found.get_or_insert(p.as_str());
        }
    }
    if let Some(p) = found {
        return Some(Offer::Move {
            repo: repo.clone(),
            dir: p.to_string(),
        });
    }
    let base = layout_base(projects).unwrap_or_else(|| format!("{home}/Source"));
    let dir = format!("{base}/{}/{}/{}", repo.host, repo.owner, repo.name);
    if let Some(rs) = dest_remotes(&dir) {
        if !has(&rs) {
            return None;
        }
        if dir == here {
            return None;
        }
        return Some(Offer::Move {
            repo: repo.clone(),
            dir,
        });
    }
    // The way the projects from that host were cloned, unless it was
    // typed as ssh.
    let (mut ssh, mut https) = (0, 0);
    for (_, rs) in projects {
        for r in rs
            .iter()
            .filter(|r| r.host.eq_ignore_ascii_case(&repo.host))
        {
            if r.ssh { ssh += 1 } else { https += 1 }
        }
    }
    let url = repo.url(repo.ssh || ssh > https);
    Some(Offer::Clone {
        repo: repo.clone(),
        url,
        dir,
    })
}

/// What the box keeps for its offer: the projects' remotes read once
/// each, the offer for what's typed now, and a clone under way.
#[derive(Default)]
pub struct GitState {
    remotes: HashMap<String, Vec<Repo>>,
    /// The typed repository and the box's project the offer was for.
    for_key: Option<(Repo, String)>,
    offer: Option<Offer>,
    /// The offer the box drew last, for the row of keys under it.
    pub shown: Option<Offer>,
    /// The repository being cloned, and where to.
    pub cloning: Option<(Repo, String)>,
    /// Each folder the box has pointed at: its project, and whether
    /// that's a git repository.
    projects_of: HashMap<String, (String, bool)>,
}

/// The project the box points at, and whether it's a git repository:
/// looked up on the disk once per folder.
pub fn box_project(m: &mut Model) -> (String, bool) {
    let dir = start_dir(m);
    let st = m.kept::<GitState>();
    if let Some(p) = st.projects_of.get(&dir) {
        return p.clone();
    }
    let p = project_of(&dir);
    let repo = crate::startdir::is_repo(&p);
    st.projects_of.insert(dir, (p.clone(), repo));
    (p, repo)
}

/// The offer for what's typed in the box now, if any. Reads the projects'
/// .git/config the first time each is asked about, and looks at where a
/// clone would go once per repository typed.
pub fn offer(m: &mut Model) -> Option<Offer> {
    let text = m.input.string();
    if text.is_empty() || super::prompt::typing_hash(&text) {
        return None;
    }
    // Only a word with a / or : in it can be an address.
    if !text
        .split_whitespace()
        .any(|w| w.contains('/') || w.contains(':'))
    {
        return None;
    }
    let (here, _) = box_project(m);
    let projects = known_projects(m, &here);
    // Remotes first, so their hosts count without https://.
    let st = m.kept::<GitState>();
    for p in &projects {
        if !st.remotes.contains_key(p) {
            st.remotes.insert(p.clone(), remotes(p));
        }
    }
    let mut hosts: Vec<String> = Vec::new();
    for p in &projects {
        for r in &st.remotes[p] {
            if !hosts.contains(&r.host) {
                hosts.push(r.host.clone());
            }
        }
    }
    let repo = find(&text, &hosts)?;
    let key = (repo.clone(), here.clone());
    if st.for_key.as_ref() == Some(&key) {
        return st.offer.clone();
    }
    let with: Vec<(String, Vec<Repo>)> = projects
        .iter()
        .map(|p| (p.clone(), st.remotes[p].clone()))
        .collect();
    let home = std::env::var("HOME").unwrap_or_default();
    let o = offer_for(&repo, &with, &here, &home, |d| {
        Path::new(d).exists().then(|| remotes(d))
    });
    st.for_key = Some(key);
    st.offer = o.clone();
    o
}

/// The offer's words, for the box's bottom edge: what tab does.
pub fn offer_line(m: &mut Model, w: usize) -> Option<Line> {
    m.kept::<GitState>().shown = None;
    if let Some((r, dir)) = m.kept::<GitState>().cloning.clone() {
        return Some(line([
            sp(format!("cloning {}/{} into ", r.owner, r.name), DIM),
            sp(short_path(&dir, w / 2), TEXT),
            sp("…", DIM),
        ]));
    }
    let o = offer(m)?;
    m.kept::<GitState>().shown = Some(o.clone());
    let room = w.saturating_sub(40).max(12);
    Some(match &o {
        Offer::Move { dir, .. } => line([
            sp("in ", DIM),
            sp(short_path(dir, room), TEXT),
            sp("? ", DIM),
            spb("tab", TEXT),
            sp(" moves there", DIM),
        ]),
        Offer::Clone { repo, dir, .. } => line([
            sp(format!("{}/{} isn't here · ", repo.owner, repo.name), DIM),
            spb("tab", TEXT),
            sp(" clones it to ", DIM),
            sp(short_path(dir, room), TEXT),
        ]),
    })
}

fn short_path(p: &str, n: usize) -> String {
    let t = tildify(p);
    if width(&t) <= n {
        t
    } else {
        super::prompt::short_dir(&t, n)
    }
}

/// tab with a repository's address typed: the box moves to its clone, or
/// it's cloned and the box moves there.
pub fn key(m: &mut Model, k: &Press) -> Out {
    if k.s != "tab" || m.input.is_empty() || super::rename::renaming(m).is_some() {
        return Out::Pass;
    }
    if m.kept::<GitState>().cloning.is_some() {
        return Out::Pass;
    }
    match offer(m) {
        Some(o) => take(m, o),
        None => Out::Pass,
    }
}

/// Does what the offer says: moves the box, or clones and then moves it.
pub fn take(m: &mut Model, o: Offer) -> Out {
    match o {
        Offer::Move { dir, .. } => {
            set_start_dir(m, &dir);
            m.flash(&format!("new agent in {}", tildify(&dir)));
            Out::Draw
        }
        Offer::Clone { repo, url, dir } => {
            m.kept::<GitState>().cloning = Some((repo.clone(), dir.clone()));
            m.flash(&format!("cloning {} into {}", repo.short(), tildify(&dir)));
            let to = dir.clone();
            m.later(
                move || clone(&url, &to),
                move |m, r: Result<(), String>| {
                    let st = m.kept::<GitState>();
                    st.cloning = None;
                    st.for_key = None;
                    st.remotes.remove(&dir);
                    match r {
                        Ok(()) => {
                            set_start_dir(m, &dir);
                            m.flash(&format!(
                                "cloned {} · new agent in {}",
                                repo.short(),
                                tildify(&dir)
                            ));
                        }
                        Err(e) => m.flash_err(&format!("couldn't clone {}: {e}", repo.short())),
                    }
                    Out::Draw
                },
            );
            Out::Draw
        }
    }
}

/// git clone url into dir, its parent made first; git never asks for a
/// password or passphrase (the terminal is rush's).
pub fn clone(url: &str, dir: &str) -> Result<(), String> {
    if let Some(parent) = Path::new(dir).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut cmd = std::process::Command::new("git");
    cmd.args(["clone", "--", url, dir])
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("git clone failed")
        .trim()
        .trim_start_matches("fatal: ")
        .to_string();
    Err(last)
}
