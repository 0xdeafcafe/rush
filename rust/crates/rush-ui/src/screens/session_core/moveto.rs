//! Folders: ctrl+l in the list picks where new sessions start, or with a
//! # command typed, the folder the selected agent moves to; #cd moves it
//! there by name or opens the same picker; #rush moves it to rush mode.
//! An agent moved is told to work there from now on rather than stopped
//! and started again; a rush session's host lists it there at once and
//! names it afresh. After Go's internal/ui/picker.go (openDirPicker :42,
//! openMovePicker :57, knownProjects :66, scanDisk :79, diskDirs :101,
//! folderChoices :137, shownDirs :162, dirSection :193, moveTo :227,
//! cdHosted :242, moveNote :257, dirPickerKey :340, pickerBody's folders
//! :411-445), known.go:127 (worktreesOf, readWorktrees), rushmode.go:5440
//! (retitleHosted) and keys.go (ctrl+l :431, #cd :1052, #rush :1084).
//! What's on disk is read off the UI thread as the picker opens and as
//! what's typed changes.

use std::collections::HashSet;

use rush_core::agent::Agent;

use super::host::hosts;
use crate::frame::picker::{Pick, Picker};
use crate::frame::{Ext, Out, Press};
use crate::model::Model;
use crate::screens::list_extras::prompt::{
    agent_dir, is_hash_cmd, set_start_dir, start_dir, start_dirs,
};
use crate::screens::session_cards::util::expand;
use crate::style::*;

/// Adds #cd, #rush and ctrl+l.
pub fn register(x: &mut Ext) {
    x.command(
        "cd",
        "tell the agent to work in another folder from now on",
        cd_command,
    );
    x.command(
        "rush",
        "move the agent into rush mode (a terminal one is copied, not stopped)",
        |m, _| {
            if let Some(a) = need(m) {
                super::rushmove::move_to_rush(m, &a);
            }
            Out::Draw
        },
    );
    x.list_keys.push(list_key);
    x.session_keys.push(session_key);
}

/// ctrl+l in a Session: where its agent works, the same folders as #cd
/// with no path (rushmode.go:4434).
fn session_key(m: &mut Model, k: &Press) -> Out {
    if k.s != "ctrl+l" || m.sheet.is_some() || m.confirm.is_some() {
        return Out::Pass;
    }
    let Some(a) = super::open_key(m).and_then(|key| super::agent(m, &key).cloned()) else {
        return Out::Pass;
    };
    open_move_picker(m, &a)
}

/// The agent picked, or says to pick one (need).
fn need(m: &mut Model) -> Option<Agent> {
    let a = m.selected_row();
    if a.is_none() {
        m.flash_err("select an agent first");
    }
    a
}

/// #cd [path]: the agent picked told to work there, or with none named,
/// the folders to choose from (keys.go:1052).
fn cd_command(m: &mut Model, arg: &str) -> Out {
    let Some(a) = need(m) else {
        return Out::Draw;
    };
    let dir = expand(arg);
    if dir.is_empty() {
        return open_move_picker(m, &a);
    }
    move_to(m, &a, &dir)
}

/// ctrl+l in the list: where new sessions start, typed or not; with a #
/// command typed, where the selected agent moves to (keys.go:431).
fn list_key(m: &mut Model, k: &Press) -> Out {
    if k.s != "ctrl+l" {
        return Out::Pass;
    }
    let text = m.input.string();
    match m.selected_row() {
        Some(a) if is_hash_cmd(&text) => open_move_picker(m, &a),
        _ => open_dir_picker(m),
    }
}

/// ctrl+l with nothing to move: the folder new sessions start in
/// (openDirPicker).
pub fn open_dir_picker(m: &mut Model) -> Out {
    let start = start_dir(m);
    let (mut p, f) = folder_picker(m, "Start new sessions in", "start new sessions here");
    if let Some(i) = p.shown().iter().position(|r| r.value == start) {
        p.cursor = i;
    }
    p.on_pick = Box::new(|m, pick, _| {
        let Some(dir) = pick.map(|p| p.value.clone()) else {
            return Out::Idle;
        };
        set_start_dir(m, &dir);
        m.flash(&format!("new sessions start in {}", tildify(&dir)));
        Out::Draw
    });
    open(m, p, f)
}

/// ctrl+l with a # command typed, or #cd alone: the same folders, and
/// the one chosen is where a is told to work from now on
/// (openMovePicker).
pub fn open_move_picker(m: &mut Model, a: &Agent) -> Out {
    let (mut p, f) = folder_picker(
        m,
        &format!("Move {} to", one_line(&a.display_name)),
        "tell it to work here",
    );
    p.note = format!("now in {}", tildify(agent_dir(a)));
    let key = a.key.clone();
    p.on_pick = Box::new(move |m, pick, _| {
        let Some(dir) = pick.map(|p| p.value.clone()) else {
            return Out::Idle;
        };
        match m.agents.iter().find(|a| a.key == key).cloned() {
            Some(a) => move_to(m, &a, &dir),
            None => Out::Draw,
        }
    });
    open(m, p, f)
}

/// What the folder pickers know besides their rows: the projects
/// Projects lists, the folders offered without looking at the disk, and
/// what was found on disk for what's typed (picker.known, .dirs, .disk).
#[derive(Clone, Default)]
struct Folders {
    known: HashSet<String>,
    dirs: Vec<String>,
    disk: Vec<String>,
}

impl Folders {
    /// The heading d is listed under (dirSection).
    fn section(&self, d: &str) -> &'static str {
        if self.known.contains(d) {
            "Projects"
        } else if self.dirs.iter().any(|x| x == d) {
            "Folders"
        } else if self.disk.iter().any(|x| x == d) {
            "On disk"
        } else {
            "Path"
        }
    }
}

/// The repositories' Claude worktrees as last read, and when, read again
/// at most every 30 seconds as a picker opens (pickTrees, worktreesOf
/// known.go:127).
#[derive(Default)]
struct PickTrees {
    by_repo: std::collections::HashMap<String, Vec<String>>,
    at: rush_core::time::Ms,
    reading: bool,
}

/// How long the worktrees read are taken as they are (known.go:131).
const TREES_FOR: rush_core::time::Ms = 30_000;

/// The folders worth offering: where new sessions have started, then
/// every folder an agent ran in, their repositories and those
/// repositories' Claude worktrees as last read, under Projects when
/// Projects lists them (folderChoices, knownProjects).
fn folders(m: &Model) -> Folders {
    let known: HashSet<String> = crate::screens::projects_machine::data::projects(m)
        .into_iter()
        .map(|p| p.key)
        .filter(|k| k.starts_with('/'))
        .collect();
    let trees = m.kept.peek::<PickTrees>();
    let mut dirs = start_dirs(m);
    let mut seen: HashSet<String> = dirs.iter().cloned().collect();
    let mut rest = Vec::new();
    let mut add = |d: &str| {
        if !d.is_empty() && seen.insert(d.to_string()) {
            rest.push(d.to_string());
        }
    };
    for a in &m.agents {
        add(&a.repo);
        if !a.repo.is_empty()
            && let Some(wts) = trees.and_then(|t| t.by_repo.get(&a.repo))
        {
            for wt in wts {
                add(wt);
            }
        }
        add(agent_dir(a));
    }
    rest.sort();
    dirs.extend(rest);
    Folders {
        known,
        dirs,
        disk: Vec::new(),
    }
}

/// A row for folder d under its heading (dirSection).
fn row(d: &str, section: &str) -> Pick {
    let t = tildify(d);
    Pick {
        label: line([sp(t.clone(), TEXT)]),
        find: t,
        section: section.into(),
        value: d.to_string(),
    }
}

/// A folder picker's folders, shared by its typed row and the reads off
/// the UI thread that fill it in.
type Shared = std::sync::Arc<std::sync::Mutex<Folders>>;

fn lock(f: &Shared) -> std::sync::MutexGuard<'_, Folders> {
    f.lock().unwrap_or_else(|e| e.into_inner())
}

/// The rows: known projects, then other folders agents worked in, then
/// what was found on disk for q (shownDirs' order).
fn rows(f: &Folders, q: &str) -> Vec<Pick> {
    let mut known = Vec::new();
    let mut other = Vec::new();
    let mut seen = HashSet::new();
    for d in &f.dirs {
        if !seen.insert(d.as_str()) {
            continue;
        }
        if f.known.contains(d) {
            known.push(row(d, "Projects"));
        } else {
            other.push(row(d, "Folders"));
        }
    }
    let mut out = known;
    out.extend(other);
    for d in &f.disk {
        if seen.insert(d.as_str()) {
            let mut r = row(d, "On disk");
            // Found for what's typed: matched by it, as typed.
            r.find = format!("{} {}", r.find, q.to_lowercase());
            out.push(r);
        }
    }
    out
}

/// A folder picker titled title, enter doing what do says: what's typed
/// narrows the folders or is a path of its own, listed under the heading
/// it would have as a row (pickerBody's folders, shownDirs, dirSection).
fn folder_picker(m: &Model, title: &str, enter: &str) -> (Picker, Shared) {
    let f = folders(m);
    let mut p = Picker::new(title, rows(&f, ""), Box::new(|_, _, _| Out::Idle));
    let f: Shared = std::sync::Arc::new(std::sync::Mutex::new(f));
    p.query = Some(("Folder ❯ ".into(), Vec::new()));
    p.empty = "no folder matches · type a path".into();
    p.typed = Some(Box::new({
        let f = f.clone();
        move |q| {
            (q.starts_with('/') || q.starts_with('~')).then(|| {
                let d = expand(q);
                row(&d, lock(&f).section(&d))
            })
        }
    }));
    p.closes = vec!["ctrl+l".into()];
    p.keys = vec![
        ("type".into(), "filter or a path".into()),
        ("↑↓".into(), "choose".into()),
        ("enter".into(), enter.into()),
        ("esc".into(), "close".into()),
    ];
    p.on_query = Some(Box::new({
        let f = f.clone();
        let title = title.to_string();
        move |m, q| scan(m, &f, &title, q)
    }));
    (p, f)
}

/// Opens p, and looks on disk for more of what it could mean.
fn open(m: &mut Model, p: Picker, f: Shared) -> Out {
    let title = p.title.clone();
    m.open_sheet(Box::new(p));
    scan(m, &f, &title, "");
    Out::Draw
}

/// Looks on disk, off the UI thread, for folders the picker's text could
/// mean: a typed path's subfolders, or else those beside the projects
/// rush knows; and, as it opens, the Claude worktrees of the repositories
/// it offers when they weren't read in the last 30 seconds. Taken in
/// while the picker is still open on the same text (scanDisk,
/// worktreesOf).
fn scan(m: &mut Model, f: &Shared, title: &str, q: &str) {
    let parents: Vec<String> = lock(f)
        .known
        .iter()
        .filter_map(|k| std::path::Path::new(k).parent())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let now = m.now;
    let pt = m.kept::<PickTrees>();
    let repos: Vec<String> = if q.is_empty() && !pt.reading && now - pt.at >= TREES_FOR {
        pt.reading = true;
        let mut r: Vec<String> = m
            .agents
            .iter()
            .map(|a| a.repo.clone())
            .filter(|r| !r.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        r.sort();
        r
    } else {
        Vec::new()
    };
    let read_trees = !repos.is_empty();
    let q = q.to_string();
    let f = f.clone();
    let title = title.to_string();
    m.later(
        {
            let q = q.clone();
            move || {
                let trees: Vec<(String, Vec<String>)> = repos
                    .into_iter()
                    .map(|r| {
                        let w = worktrees(&r);
                        (r, w)
                    })
                    .collect();
                (trees, disk_dirs(&q, &parents))
            }
        },
        move |m, (trees, disk): (Vec<(String, Vec<String>)>, Vec<String>)| {
            if read_trees {
                let now = m.now;
                let pt = m.kept::<PickTrees>();
                pt.by_repo = trees.into_iter().collect();
                pt.at = now;
                pt.reading = false;
            }
            // The folders again, with the worktrees just read.
            let mut fresh = folders(m);
            let Some(p) = m.picker().filter(|p| p.title == title) else {
                return Out::Idle;
            };
            let typed: String = p
                .query
                .as_ref()
                .map(|(_, q)| q.iter().collect())
                .unwrap_or_default();
            if typed.trim() != q {
                return Out::Idle;
            }
            fresh.disk = disk;
            p.rows = rows(&fresh, &q);
            *lock(&f) = fresh;
            Out::Draw
        },
    );
}

/// A repository's Claude worktrees (readWorktrees).
pub fn worktrees(repo: &str) -> Vec<String> {
    let dir = std::path::Path::new(repo).join(".claude").join("worktrees");
    let Ok(ents) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = ents
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| dir.join(e.file_name()).to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// The folders q could mean: with a path, those in its folder that start
/// as its last part does; else those in parents whose name holds q; 200
/// at most, typing more of a path narrows them (diskDirs).
pub fn disk_dirs(q: &str, parents: &[String]) -> Vec<String> {
    let lq = q.to_lowercase();
    let (parents, base, by_path): (Vec<String>, String, bool) =
        if q.starts_with('/') || q.starts_with('~') {
            let full = expand(q);
            let (dir, base) = if q.ends_with('/') {
                (full.trim_end_matches('/').to_string(), String::new())
            } else {
                let p = std::path::Path::new(&full);
                (
                    p.parent()
                        .map(|d| d.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    p.file_name()
                        .map(|b| b.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            };
            let dir = if dir.is_empty() { "/".to_string() } else { dir };
            (vec![dir], base.to_lowercase(), true)
        } else {
            (parents.to_vec(), String::new(), false)
        };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for par in &parents {
        if !seen.insert(par.clone()) {
            continue;
        }
        let Ok(ents) = std::fs::read_dir(par) else {
            continue;
        };
        for e in ents.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let ln = name.to_lowercase();
            let hit = if by_path {
                ln.starts_with(&base)
            } else {
                ln.contains(&lq)
            };
            if hit && !name.starts_with('.') && e.file_type().is_ok_and(|t| t.is_dir()) {
                out.push(
                    std::path::Path::new(par)
                        .join(&name)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    out.sort();
    out.truncate(200);
    out
}

/// Tells a to work in dir from now on, rather than stopping it to start
/// again there; its row follows once it cd's there or writes a file
/// there. A rush session is told mid-turn, not queued, so what it does
/// before reading it doesn't land in the old folder, then listed there
/// and named afresh for it unless you named it (moveTo).
pub fn move_to(m: &mut Model, a: &Agent, dir: &str) -> Out {
    if a.interactive {
        m.flash_err(&format!(
            "{} is open in another terminal · tell it there",
            a.display_name
        ));
        return Out::Draw;
    }
    let text = move_note(dir, agent_dir(a));
    if !a.rush {
        return super::cards_host::reply_to(m, &a.key, text.clone(), &text);
    }
    m.flash(&format!("sending to {}…", a.display_name));
    let h = hosts(m);
    let (id, key, dir, name) = (
        a.job.id.clone(),
        a.key.clone(),
        dir.to_string(),
        a.display_name.clone(),
    );
    super::later(
        m,
        move || {
            h.send_to(&id, &text, true)?;
            // A name you gave it stays.
            let named = rush_state::Store::load()
                .overlay
                .names
                .get(&key)
                .is_some_and(|n| !n.is_empty());
            let _ = h.cd(&id, &dir, !named);
            Ok::<(), String>(())
        },
        move |m, r| {
            match r {
                Ok(()) => m.flash(&format!("sent to {name}")),
                Err(e) => m.fail(&e),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// Asks an agent to carry on in dir. Its shell can go back to where it
/// started between commands, so each one cd's first; the leading cd is
/// also what tells rush where it works now (moveNote).
pub fn move_note(dir: &str, from: &str) -> String {
    let cd = if dir.contains([' ', '\'', '"', '$', '`']) {
        format!("'{}'", dir.replace('\'', r"'\''"))
    } else {
        dir.to_string()
    };
    format!(
        "Please move over to {dir} and work there from now on (you were in {from}). \
         Start by running `cd {cd}`. Your shell may go back to the old folder between commands, \
         so begin each Bash command with `cd {cd} && ` and use absolute paths under {dir}. \
         Paths from earlier in this conversation point at the old folder."
    )
}
