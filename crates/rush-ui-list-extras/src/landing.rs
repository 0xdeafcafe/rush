//! What's left before an agent's work lands, and Earlier has it by
//! itself: an agent in a worktree of its own that's stopped work says so
//! on its row, worst first. An agent in the main checkout has nothing to
//! land, so it says nothing. After Go's internal/ui/landing.go (landKind
//! :17, landState :31, toLand :40, tag :43, landOf :52, landTarget :97,
//! landText :105, land :127, as of 2e4662ff), and folders.go's
//! refreshFolders :337 and onFolders :395 for the list (every look, as
//! of 2e4662ff), which ask git about the projects the list shows, off
//! the UI thread, every folderEvery (:22) and at once for one with no
//! answer yet. The tag goes under the name in the rail look (rail.go:96),
//! at the row's end in the classic (view.go:2336).

use std::collections::HashMap;

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_fleet::{Folder, Predicates, check_folder, folder_wants};

use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;
use rush_ui_social::broadcast::ModelBroadcast;

// What's left before an agent's work lands is the frame's (crate::landing).
pub use rush_ui_base::landing::{
    FOLDER_EVERY, LandKind, LandState, ListFolders, land_of, land_target, tree_of,
};

/// What the agent is told to do to land its work (landText).
pub fn land_text(l: &LandState) -> String {
    let pr = format!("pull request #{}", l.pr);
    let t = &l.target;
    match l.kind {
        LandKind::Uncommitted => format!(
            "Land your work on {t}: commit what's left in this worktree, then get it onto {t} the way this project does (open a pull request and merge it once it's green, or merge the branch straight in)."
        ),
        LandKind::PrFailing => {
            format!("The checks on {pr} are failing. Fix them, push, and merge it once it's green.")
        }
        LandKind::PrChanges => format!(
            "Changes were asked for on {pr}. Make them, push, and merge it once it's approved and green."
        ),
        LandKind::PrWaiting => format!(
            "Get {pr} merged: mark it ready if it's a draft, see its checks through, and merge it once it's green."
        ),
        LandKind::PrReady => format!("Merge {pr} into {t}."),
        LandKind::Unmerged => format!(
            "Land your work on {t}: get this branch onto {t} the way this project does (open a pull request and merge it once it's green, or merge the branch straight in)."
        ),
        _ => String::new(),
    }
}

/// Tells agent i to land its work, as a message you see it get (land:
/// alt+p, #land), or says why not.
pub fn land(m: &mut Model, i: Option<usize>) {
    let Some(i) = i else {
        m.flash_err("select an agent first");
        return;
    };
    let a = m.agents[i].clone();
    let name = &a.display_name;
    let l = land_of(m, &a);
    if a.interactive {
        m.flash_err(&format!("{name} is open in another terminal"));
    } else if a.job.live() || a.busy() {
        m.flash(&format!("{name} is still working"));
    } else if tree_of(&a).is_empty() {
        m.flash(&format!(
            "{name} works in the main checkout: nothing to land"
        ));
    } else if l.kind == LandKind::None {
        m.flash(&format!(
            "git hasn't said yet what {name} has to land · try again in a moment"
        ));
    } else if l.kind == LandKind::Landed {
        m.flash(&format!("{name}'s work has landed · Earlier has it soon"));
    } else if l.kind == LandKind::Nothing {
        m.flash(&format!(
            "{name} has nothing to land · alt+d puts it in Earlier"
        ));
    } else {
        // replyTo: you see it go, then its answer.
        m.flash(&format!("sending to {name}…"));
        m.reply_to(&a, &land_text(&l));
    }
}

/// alt+p in the list (keys.go:470).
pub fn key(m: &mut Model, k: &Press) -> Out {
    if k.s != "alt+p" {
        return Out::Pass;
    }
    let i = m.selected();
    land(m, i);
    Out::Draw
}

/// #land (keys.go:994).
pub fn command(m: &mut Model, _: &str) -> Out {
    let i = m.selected();
    land(m, i);
    Out::Draw
}

/// Asks git about the projects the list shows, but Earlier's, off the
/// UI thread: every FOLDER_EVERY, and at once when a project or worktree
/// has no answer yet (refreshFolders, onFolders). Only with a loop
/// running; a model drawn once (tests, --render) is handed its answers.
pub fn refresh(m: &mut Model) {
    if m.sender().is_none() {
        return;
    }
    ask_folders(m);
}

/// Asks git about the projects the list shows when it's time, off the UI
/// thread; with no loop (`rush --render`), at once, as Go's Frame drains
/// what a reading started.
pub fn ask_folders(m: &mut Model) {
    if m.kept::<ListFolders>().looking {
        return;
    }
    let listed: Vec<Agent> = m
        .order
        .iter()
        .filter(|&&i| m.group_of.get(&m.agents[i].key).map(String::as_str) != Some("Earlier"))
        .map(|&i| m.agents[i].clone())
        .collect();
    let wants = folder_wants(&listed);
    if wants.is_empty() {
        return;
    }
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    watch(m, &wants);
    let f = m.kept::<ListFolders>();
    let due = now - f.asked >= FOLDER_EVERY;
    let watching = f.watch.is_some();
    let changed = f.changed.load(std::sync::atomic::Ordering::Relaxed);
    // Watched, only what changed is asked again; else every FOLDER_EVERY.
    let mut stale = due && (changed || !watching);
    for (root, trees) in &wants {
        match f.by_root.get(root) {
            None => stale = true,
            Some(had) => stale = stale || trees.iter().any(|t| !had.trees.contains_key(t)),
        }
    }
    if !stale {
        let left = f.asked + FOLDER_EVERY - now;
        if (changed || !watching) && !f.due {
            wake_at(m, left);
        }
        return;
    }
    f.changed.store(false, std::sync::atomic::Ordering::Relaxed);
    f.looking = true;
    f.asked = now;
    rush_draw::perf_note(|| format!("folders asked {} watched={watching}", wants.len()));
    let wants: Vec<(String, Vec<String>)> = wants.into_iter().collect();
    m.later(
        move || {
            // A few projects ask git at once, not one per repository
            // (folderGate).
            let mut out = Vec::with_capacity(wants.len());
            for chunk in wants.chunks(4) {
                std::thread::scope(|s| {
                    let hs: Vec<_> = chunk
                        .iter()
                        .map(|(root, trees)| s.spawn(move || check_folder(root, trees, false)))
                        .collect();
                    out.extend(hs.into_iter().filter_map(|h| h.join().ok()));
                });
            }
            out
        },
        |m, got: Vec<Folder>| {
            let f = m.kept::<ListFolders>();
            f.looking = false;
            for g in got {
                f.by_root.insert(g.root.clone(), g);
            }
            m.rebuild();
            Out::Draw
        },
    );
}

/// Asks again once `left` has passed (only with a loop running).
fn wake_at(m: &mut Model, left: Ms) {
    if m.sender().is_none() {
        return;
    }
    m.kept::<ListFolders>().due = true;
    let left = std::time::Duration::from_millis(left.clamp(0, FOLDER_EVERY) as u64);
    m.later(
        move || std::thread::sleep(left),
        |m, ()| {
            m.kept::<ListFolders>().due = false;
            ask_folders(m);
            Out::Idle
        },
    );
}

/// Keeps the stream on the checkouts git is asked about: each root, and
/// each worktree not under one. What asking git writes (a checkout's
/// index, its objects) doesn't count as a change.
fn watch(m: &mut Model, wants: &HashMap<String, Vec<String>>) {
    let Some(tx) = m.sender() else { return };
    let mut paths: Vec<String> = wants.keys().cloned().collect();
    for (root, trees) in wants {
        for t in trees {
            if !std::path::Path::new(t).starts_with(root) {
                paths.push(t.clone());
            }
        }
    }
    paths.sort();
    paths.dedup();
    let f = m.kept::<ListFolders>();
    if f.watch.as_ref().is_some_and(|(p, _)| *p == paths) {
        return;
    }
    let roots: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
    let mut skip = Vec::new();
    for r in &roots {
        let git = r.join(".git");
        skip.push(git.join("index"));
        skip.push(git.join("index.lock"));
        skip.push(git.join("objects"));
        skip.push(git.join("worktrees")); // each worktree's own index
    }
    let (changed, told) = (f.changed.clone(), f.told.clone());
    // Two: news that comes while the last is being handled still lands.
    let tx = tx.lane(2);
    let tell: rush_sys::fswait::Tell = Box::new(move |v| {
        rush_draw::perf_note(|| format!("folders changed {v:?}"));
        changed.store(true, std::sync::atomic::Ordering::Relaxed);
        if told.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return; // the last news is still on its way
        }
        let told = told.clone();
        let sent = tx.try_send(crate::worker::Note::Apply(Box::new(
            move |m: &mut Model| {
                m.kept::<ListFolders>()
                    .told
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                ask_folders(m);
                Out::Idle
            },
        )));
        if sent.is_err() {
            told.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    });
    f.watch = rush_sys::fswait::Deep::new(&roots, &skip, tell).map(|d| (paths, d));
    let on = f.watch.is_some();
    rush_draw::perf_note(|| format!("folders watched {roots:?} on={on}"));
}

/// Under the name in the rail look: what's left before Earlier has it
/// (rail.go:96).
pub fn where_tag(m: &Model, a: &Agent) -> Option<Line> {
    land_of(m, a).tag().map(|s| line([s]))
}
