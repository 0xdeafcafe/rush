//! What the Projects place asks of git, GitHub, the disk and the process
//! table, each off the UI thread through Model::later: only while the
//! place is showing, and never more often than Go's (a folder's git every
//! 15s, worktrees every 2 minutes, GitHub when its answer is due, the
//! processes with each reading of the list). Go: internal/ui/folders.go
//! (refreshFolders :339, onFolders :393), cleanup.go (scanWorktrees :145,
//! onWorktrees :363, mergeChecks :378), prs.go (refreshPRs :63, onPRs
//! :101, askPRsAgain :148), projects_paths.go (peekDir :46, listTemp
//! :130, openTemp :150), tempwork.go (measureTemp :28), fleet/temp.go:27-40 (TempDirs) and
//! model.go:1110
//! (the tick's rescans).

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

use rush_core::adapter::TempDir;
use rush_core::time::{Ms, now};
use rush_fleet::find_scratch;
use rush_fleet::{Worktree, find_worktrees};
use rush_fleet::{check_folder, folder_wants};
use rush_gh::ghpr::Gh;

use super::data::*;
use super::view::agent_tmp_dirs;
use crate::frame::{Out, PlaceId};
use crate::model::Model;

/// Worktrees are looked at again after this while the place is open.
const RESCAN_EVERY: Ms = 2 * 60_000;
/// /tmp is walked again after this.
const TMP_EVERY: Ms = 10 * 60_000;
/// The processes are read again after this.
const MACHINE_EVERY: Ms = 2_000;

fn d(m: &mut Model) -> &mut Data {
    m.kept::<Data>()
}

/// Everything due, as the place opens or the list is read again while it
/// shows. In tests (frozen) nothing is asked.
pub fn refresh(m: &mut Model) {
    read_settings(m);
    if m.place != PlaceId::Projects || d(m).frozen {
        return;
    }
    refresh_folders(m);
    // The machine's load, as Go's reading carries it.
    read_machine(m);
    // Temp work as last measured, then what's due measured again.
    load_sizes(m);
    measure_temp(m);
    if m.sender().is_none() {
        // No loop (rush --render): git's word on each folder and the
        // machine only, as Go's Frame drains refreshFolders and its
        // reading has the machine. GitHub is asked, as Go's refreshPRs
        // marks it, but its answer never comes: "asking GitHub…".
        let roots: Vec<String> = projects(m)
            .into_iter()
            .filter(|p| p.repo)
            .map(|p| p.key)
            .collect();
        d(m).asking.extend(roots);
        return;
    }
    refresh_prs(m);
    super::projectmem::refresh_memory(m);
    if now() - d(m).checked > RESCAN_EVERY {
        scan_worktrees(m);
    }
    arm(m);
}

/// The settings are read again this often, as Go's tick's reloadConfig.
const CONFIG_EVERY: Ms = 3_000;

/// Reads the settings and what was put away by hand, off the UI thread:
/// once, then again every CONFIG_EVERY while lists are read or the place
/// shows. In tests they're set (cfg_read) and never read.
fn read_settings(m: &mut Model) {
    let st = d(m);
    if st.cfg_reading || st.cfg_read && (st.frozen || now() - st.cfg_at < CONFIG_EVERY) {
        return;
    }
    st.cfg_reading = true;
    m.later(Cfg::load, |m, (cfg, done)| {
        let st = d(m);
        let changed = !st.cfg_read || st.cfg != cfg || st.done != done;
        st.cfg = cfg;
        st.done = done;
        st.cfg_read = true;
        st.cfg_at = now();
        st.cfg_reading = false;
        if changed { Out::Draw } else { Out::Idle }
    });
}

/// While the place shows, what's due is asked again on Go's tick
/// (model.go:1087: folders, pull requests, temp work, worktrees every 2
/// minutes), not only when the list is read again, which an idle machine
/// never does: a wake-up every MACHINE_EVERY, one at a time, that stops
/// once the place is left. Nothing runs when it isn't showing.
fn arm(m: &mut Model) {
    let st = d(m);
    if st.armed {
        return;
    }
    st.armed = true;
    m.after(
        std::time::Duration::from_millis(MACHINE_EVERY as u64),
        |m| {
            d(m).armed = false;
            refresh(m);
            let st = data(m);
            // A spinner on the page turns while something's asked.
            if st.checking || st.looking > 0 || st.measuring {
                Out::Draw
            } else {
                Out::Idle
            }
        },
    );
}

/// Asks git about each repository on the page, whole, every FOLDER_EVERY
/// at most, and at once when one has no answer yet.
pub fn refresh_folders(m: &mut Model) {
    if d(m).looking > 0 {
        return;
    }
    let wants = {
        let ags = work_agents(m);
        let rooted: Vec<rush_core::agent::Agent> = ags
            .iter()
            .map(|g| rush_core::agent::Agent {
                root: g.root.to_string(),
                ..g.a.clone()
            })
            .collect();
        folder_wants(&rooted)
    };
    if wants.is_empty() {
        return;
    }
    let st = d(m);
    let mut stale = now() - st.folders_asked >= FOLDER_EVERY;
    for (root, trees) in &wants {
        match st.folders.get(root) {
            Some(f) if f.whole => {
                if trees.iter().any(|t| !f.trees.contains_key(t)) {
                    stale = true;
                }
            }
            _ => stale = true,
        }
    }
    if !stale {
        return;
    }
    st.looking = wants.len();
    st.folders_asked = now();
    for (root, trees) in wants {
        m.later(
            move || check_folder(&root, &trees, true),
            |m, f| {
                let st = d(m);
                st.looking = st.looking.saturating_sub(1);
                st.folders.insert(f.root.clone(), f);
                Out::Draw
            },
        );
    }
}

/// What the worktree scan found.
pub struct Found {
    wts: Vec<Worktree>,
    tmp: Option<rush_fleet::Scratch>,
    agent_tmp: HashMap<String, i64>,
    peeks: HashMap<String, PathInfo>,
    outside: Vec<rush_fleet::LeftItem>,
    /// What's in rush's scratch root that no session owns (Rust-only).
    orphans: Vec<rush_fleet::TempOrphan>,
    /// The worktrees' sizes are walked; if not, they keep the sizes they had.
    measured: bool,
}

/// Finds every agent's worktree and checks each with git and measures it,
/// /tmp when it's due, the projects' agent scratch, and the tops of the
/// folders the page names.
///
/// Rust-only: what git says of the worktrees is shown as soon as it's in,
/// before their sizes are walked (at a background pace, a big checkout's
/// build folders take minutes). Go showed nothing of the scan until all
/// of it was done; the page still says it's asking until then.
pub fn scan_worktrees(m: &mut Model) {
    if d(m).checking || d(m).frozen {
        return;
    }
    d(m).checking = true;
    let early = m.sender().map(|tx| {
        let tx = tx.lane(1);
        Box::new(move |wts: Vec<Worktree>| {
            tx.send(crate::worker::Note::Apply(Box::new(
                move |m: &mut Model| {
                    on_looked(m, wts);
                    Out::Draw
                },
            )));
        }) as Early
    });
    let scan = worktree_scan(m, early, true);
    m.later(scan, |m, f| {
        on_worktrees(m, f);
        Out::Draw
    });
}

/// Handed what git said of the worktrees, before their sizes.
pub type Early = Box<dyn Fn(Vec<Worktree>) + Send + Sync>;

/// The look scan_worktrees takes, to run off the UI (cleanup.go:155
/// worktreeScan): the cleanup agent's start takes one too. early, when
/// given, has the worktrees as git found them, before they're measured.
pub fn worktree_scan(
    m: &mut Model,
    early: Option<Early>,
    measure: bool,
) -> impl FnOnce() -> Found + Send + 'static {
    let workers: Vec<rush_core::agent::Agent> = agents(m).map(|g| g.a.clone()).collect();
    let tmp_due = now() - d(m).tmp.checked > TMP_EVERY;
    let mut tmps = Vec::new();
    let mut peeks = Vec::new();
    for p in projects(m) {
        let dirs = agent_tmp_dirs(m, &p.key);
        peeks.extend(dirs.iter().cloned());
        tmps.extend(dirs);
        if !p.repo && p.key.starts_with('/') {
            peeks.push(p.key.clone());
        }
    }
    let reg = m.reg.clone();
    move || {
        let mut f = Found {
            wts: Vec::new(),
            tmp: None,
            agent_tmp: HashMap::new(),
            peeks: HashMap::new(),
            outside: Vec::new(),
            orphans: Vec::new(),
            measured: true,
        };
        // git first, each worktree measured after (Worktree::check).
        let gh = Gh::shared();
        f.wts = find_worktrees(&workers, Some(gh));
        let looked = check_git_all(&mut f.wts, early.as_ref());
        if let Some(early) = &early {
            early(f.wts.clone());
        }
        f.measured = measure;
        for t in tmps {
            if std::fs::metadata(&t).is_ok_and(|s| s.is_dir()) {
                let n = disk_usage(&[t.as_str()]);
                f.agent_tmp.insert(t, n);
            }
        }
        for p in peeks {
            f.peeks.insert(p.clone(), peek_dir(&p));
        }
        if tmp_due {
            f.tmp = Some(find_scratch(&reg, "/tmp"));
        }
        f.outside = rush_fleet::look_left(&workers);
        f.orphans = rush_fleet::find_temp_orphans();
        if measure {
            for (w, ok) in f.wts.iter_mut().zip(looked) {
                if ok {
                    w.measure();
                }
            }
        }
        f
    }
}

/// Looks at each worktree with git, GIT_AT_ONCE at a time, each taking the
/// next one waiting (Go: one at a time; with dozens of checkouts on a
/// busy machine that took minutes); whether git could, for each. progress
/// is handed the worktrees looked at so far as they come in, at most
/// every PROGRESS_EVERY, and the whole of them at the end.
fn check_git_all(wts: &mut Vec<Worktree>, progress: Option<&Early>) -> Vec<bool> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    const GIT_AT_ONCE: usize = 8;
    const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(150);
    let n = wts.len();
    let next = AtomicUsize::new(0);
    let done = Mutex::new((vec![None::<(Worktree, bool)>; n], std::time::Instant::now()));
    let todo = Arc::new(std::mem::take(wts));
    std::thread::scope(|sc| {
        for _ in 0..GIT_AT_ONCE.min(n) {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let mut w = todo[i].clone();
                    let ok = w.check_git_cached();
                    let mut g = done.lock().unwrap_or_else(|e| e.into_inner());
                    g.0[i] = Some((w, ok));
                    if let Some(p) = progress
                        && g.1.elapsed() >= PROGRESS_EVERY
                        && g.0.iter().any(Option::is_none)
                    {
                        g.1 = std::time::Instant::now();
                        p(g.0.iter().flatten().map(|(w, _)| w.clone()).collect());
                    }
                }
            });
        }
    });
    let (rows, _) = done.into_inner().unwrap_or_else(|e| e.into_inner());
    let mut looked = Vec::with_capacity(n);
    for (i, r) in rows.into_iter().enumerate() {
        // A thread that died leaves its worktree as it was, unlooked.
        let (w, ok) = r.unwrap_or_else(|| (todo[i].clone(), false));
        wts.push(w);
        looked.push(ok);
    }
    looked
}

/// What git said of the worktrees, in before their sizes: each keeps the
/// size it last measured until it's measured again. The scan isn't done:
/// the page says it's still asking, and what waits for the whole scan
/// (Clean up, the sizes that can go) still waits.
fn on_looked(m: &mut Model, mut wts: Vec<Worktree>) {
    let st = d(m);
    if !st.checking {
        return; // the whole scan beat it in
    }
    for w in &mut wts {
        if let Some(old) = st.wts.iter().find(|o| o.path == w.path) {
            w.size = old.size;
        }
    }
    // Those not looked at yet stay as they were, until the scan is done.
    let rest: Vec<Worktree> = st
        .wts
        .iter()
        .filter(|o| !wts.iter().any(|w| w.path == o.path))
        .cloned()
        .collect();
    wts.extend(rest);
    wts.sort_by(|a, b| a.path.cmp(&b.path));
    st.wts = wts;
}

pub fn on_worktrees(m: &mut Model, f: Found) {
    let st = d(m);
    st.checking = false;
    st.checked = now();
    let mut wts = f.wts;
    if !f.measured {
        for w in &mut wts {
            if let Some(old) = st.wts.iter().find(|o| o.path == w.path) {
                w.size = old.size;
            }
        }
    }
    st.wts = wts;
    st.agent_tmp = f.agent_tmp;
    st.outside = f.outside;
    st.scratch_orphans = f.orphans;
    st.peeks.extend(f.peeks);
    if let Some(t) = f.tmp {
        st.tmp = t;
    }
    // Only on the whole scan, sizes in: git's look alone has none, and the
    // one nudge a day would say "–" (cleanup.go:376-379).
    super::clean::nudge_clean(m);
    if !f.measured {
        // Sizes were left for after: git's look is kept, so this is
        // the walking alone, off the card's way.
        scan_worktrees(m);
    }
}

/// What the few biggest processes under each orphan run, in words: their
/// command lines, for System's rows (known.go:102 shortCmd,
/// readProcWords). Read with the table, off the UI thread.
fn kid_words(mc: &rush_fleet::Machine, tab: &rush_sys::proc::Table) -> HashMap<i32, String> {
    let mut out = HashMap::new();
    for r in mc
        .rows
        .iter()
        .filter(|r| r.role == rush_fleet::Role::Orphan)
    {
        let mut kids: Vec<_> = tab.tree(r.pid).into_iter().skip(1).collect();
        kids.sort_by_key(|k| std::cmp::Reverse(k.footprint));
        for k in kids.iter().take(3).filter(|k| k.footprint >= 32 << 20) {
            let mut args = rush_sys::proc::args(k.pid);
            let Some(first) = args.first_mut() else {
                continue;
            };
            if let Some(base) = first.rsplit('/').next() {
                *first = base.to_string();
            }
            // No terminal escapes from a command line reach the screen.
            let line: String = crate::style::one_line(&args.join(" "))
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            let line = super::rows::trim_cmd(&line, 200);
            out.insert(k.pid, line.trim_end().to_string());
        }
    }
    out
}

/// Reads what each agent's temp work measured last, once, off the UI
/// thread (fleet.go:680, the loader's Temp.Load). With no loop (--render)
/// it's read at once, as Go's loader does before its first frame.
fn load_sizes(m: &mut Model) {
    let st = d(m);
    if st.temp_sizes.loaded || st.temp_loading {
        return;
    }
    st.temp_loading = true;
    m.later(
        || {
            let mut t = super::temp::TempSizes::default();
            t.load(&super::temp::temp_path());
            t
        },
        |m, disk| {
            let st = d(m);
            st.temp_loading = false;
            for (k, v) in disk.sizes {
                st.temp_sizes.sizes.entry(k).or_insert(v);
            }
            st.temp_sizes.loaded = true;
            for (k, t) in st.temp.iter_mut() {
                if let Some(b) = st.temp_sizes.bytes(k) {
                    (t.bytes, t.known) = (b, true);
                }
            }
            measure_temp(m);
            Out::Draw
        },
    );
}

/// Each agent's temp folders as they are now, with what they measured
/// last; and the temp work of the agents whose temp work may have changed
/// walked off the UI thread, one batch at a time: never-measured ones
/// once, finished ones again only after they've done something, running
/// ones every ten minutes, longer for ones slow to walk (tempwork.go:28
/// measureTemp, onTemp). The sizes are saved off the UI thread.
pub fn measure_temp(m: &mut Model) {
    if d(m).frozen || d(m).listing_temp {
        return;
    }
    // Where each agent's temp work is, worked out off the UI thread: a
    // rush session's folder is a link read on the disk (TempDirs).
    let workers: Vec<rush_core::agent::Agent> = agents(m).map(|g| g.a.clone()).collect();
    let reg = m.reg.clone();
    d(m).listing_temp = true;
    m.later(
        move || {
            workers
                .into_iter()
                .filter_map(|a| {
                    let dirs = rush_fleet::temp_dirs(&reg, &a);
                    (!dirs.is_empty()).then_some((a, dirs))
                })
                .collect::<Vec<_>>()
        },
        |m, found| {
            d(m).listing_temp = false;
            took_temp_dirs(m, found);
            Out::Draw
        },
    );
}

/// Each agent's temp folders, as just worked out, with what they
/// measured last; the ones due measured again, one batch at a time.
fn took_temp_dirs(m: &mut Model, found: Vec<(rush_core::agent::Agent, Vec<TempDir>)>) {
    let t = now();
    let mut due = Vec::new();
    let mut temp = HashMap::new();
    let st = d(m);
    for (a, dirs) in found {
        if st.temp_sizes.loaded && st.temp_sizes.due(&a, t) {
            due.push((a.key.clone(), dirs.clone()));
        }
        let had = st.temp_sizes.bytes(&a.key);
        let (bytes, known) = (had.unwrap_or(0), had.is_some());
        temp.insert(a.key, TempWork { dirs, bytes, known });
    }
    let looped = m.sender().is_some();
    let st = d(m);
    st.temp = temp;
    if st.measuring || due.is_empty() || !looped {
        return;
    }
    st.measuring = true;
    m.later(
        move || {
            let mut out = HashMap::new();
            for (k, dirs) in due {
                let at = now();
                let start = std::time::Instant::now();
                let paths: Vec<&str> = dirs.iter().map(|d| d.path.as_str()).collect();
                let bytes = disk_usage(&paths);
                out.insert(
                    k,
                    super::temp::TempSize {
                        bytes,
                        at,
                        took: start.elapsed(),
                    },
                );
            }
            out
        },
        |m, got| {
            let st = d(m);
            st.measuring = false;
            for (k, s) in &got {
                if let Some(t) = st.temp.get_mut(k) {
                    (t.bytes, t.known) = (s.bytes, true);
                }
            }
            st.temp_sizes.set(got);
            if st.temp_sizes.dirty {
                let mut save = st.temp_sizes.clone();
                st.temp_sizes.dirty = false;
                m.later(
                    move || save.save(&super::temp::temp_path()),
                    |_, ()| Out::Idle,
                );
            }
            Out::Draw
        },
    );
}

/// The top of dir: its things, and when the newest changed. Only the top.
pub fn peek_dir(dir: &str) -> PathInfo {
    let mut p = PathInfo {
        at: now(),
        ..PathInfo::default()
    };
    if let Ok(st) = std::fs::symlink_metadata(dir) {
        p.modified = st.mtime() * 1000;
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            p.items += 1;
            if let Ok(st) = e.metadata()
                && let Ok(t) = st.modified()
            {
                p.modified = p.modified.max(ms_of(t));
            }
        }
    }
    p
}

/// Reads the process table and the machine's agent rows from it, and
/// what the agents left running (reap).
pub fn read_machine(m: &mut Model) {
    let st = d(m);
    if st.reading || st.frozen || now() - st.machine_at < MACHINE_EVERY {
        return;
    }
    st.reading = true;
    let prev = st.prev_table.take();
    let mut reaper = st.reaper.take().unwrap_or_default();
    let agents = m.agents.clone();
    let reg = m.reg.clone();
    m.later(
        move || {
            let mut tab = rush_sys::proc::snapshot();
            let left = reaper.scan(&reg, &tab, &agents);
            let mc = rush_fleet::machine_rows(&reg, "claude", &mut tab, prev.as_ref(), &agents);
            let words = kid_words(&mc, &tab);
            (mc, tab, reaper, left, words)
        },
        |m, (mc, tab, reaper, left, words)| {
            let st = d(m);
            st.reading = false;
            st.machine = mc;
            st.kid_words = words;
            st.machine_at = now();
            st.prev_table = Some(tab);
            st.reaper = Some(reaper);
            super::tidy::after_reading(m, left);
            Out::Draw
        },
    );
}

/// The machine read again now, its CPU from the reading before.
pub fn read_machine_again(m: &mut Model) {
    d(m).machine_at = 0;
    read_machine(m);
}

/// Asks GitHub again of each repository on the page whose answer is due.
pub fn refresh_prs(m: &mut Model) {
    let roots: Vec<String> = projects(m)
        .into_iter()
        .filter(|p| p.repo)
        .map(|p| p.key)
        .collect();
    ask_prs(m, roots);
}

/// The open agent's repository asked of GitHub when due, wherever rush
/// is, for its Overview and Changes and its finishing card (refreshPRs'
/// m.host, prs.go:72). Only with a loop to ask it off the UI, and never
/// in tests (frozen).
pub fn refresh_open_prs(m: &mut Model) {
    if m.sender().is_none() || d(m).frozen {
        return;
    }
    let Some(key) = m.session.as_ref().map(|v| v.key.clone()) else {
        return;
    };
    let Some(i) = m.agents.iter().position(|a| a.key == key) else {
        return;
    };
    let a = &m.agents[i];
    if a.repo.is_empty() {
        return;
    }
    let root = m.roots.get(i).map_or("", String::as_str);
    let k = crate::list::folder_key(a, root).to_string();
    if std::path::Path::new(&k).is_absolute() {
        ask_prs(m, vec![k]);
    }
}

/// Asks GitHub of each of roots whose answer is due, off the UI.
fn ask_prs(m: &mut Model, roots: Vec<String>) {
    let t = now();
    for root in roots {
        let st = d(m);
        if st.asking.contains(&root) || st.prs.get(&root).is_some_and(|r| t < r.next) {
            continue;
        }
        st.asking.insert(root.clone());
        m.later(
            move || Gh::shared().look(&root),
            |m, r| {
                let st = d(m);
                st.asking.remove(&r.root);
                let mut r = r;
                if let Some(old) = st.prs.get(&r.root)
                    && !r.err.is_empty()
                    && r.prs.is_empty()
                {
                    r.prs = old.prs.clone(); // GitHub failing once doesn't lose what it said before
                }
                st.prs.insert(r.root.clone(), r);
                Out::Draw
            },
        );
    }
}

/// Forgets when GitHub was last asked, so it's asked now.
pub fn ask_prs_again(m: &mut Model) {
    for r in d(m).prs.values_mut() {
        r.next = 0;
    }
    Gh::shared().forget();
    refresh_prs(m);
}

/// What's at the top of dirs.
fn list_temp(dirs: &[TempDir]) -> Vec<TempEntry> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d.path) else {
            continue;
        };
        for e in rd.flatten() {
            let st = e.metadata().ok();
            out.push(TempEntry {
                path: e.path().to_string_lossy().into_owned(),
                dir: st.as_ref().is_some_and(|s| s.is_dir()),
                modified: st.and_then(|s| s.modified().ok()).map(ms_of).unwrap_or(0),
            });
        }
    }
    out
}

/// Shows or hides what's in a session's temp folders: listed off the UI,
/// then each thing measured once.
pub fn open_temp(m: &mut Model, key: &str) {
    let st = d(m);
    if !st.opened.remove(key) {
        st.opened.insert(key.to_string());
    } else {
        return;
    }
    let dirs = st.temp.get(key).map(|t| t.dirs.clone()).unwrap_or_default();
    let key = key.to_string();
    m.later(
        move || list_temp(&dirs),
        move |m, ents| {
            let st = d(m);
            let todo: Vec<String> = ents
                .iter()
                .filter(|e| !st.sizes.contains_key(&e.path))
                .map(|e| e.path.clone())
                .collect();
            st.lists.insert(key, (ents, now()));
            if !todo.is_empty() {
                m.later(
                    move || {
                        todo.into_iter()
                            .map(|p| {
                                let n = disk_usage(&[p.as_str()]);
                                (p, n)
                            })
                            .collect::<Vec<_>>()
                    },
                    |m, got| {
                        d(m).sizes.extend(got);
                        Out::Draw
                    },
                );
            }
            Out::Draw
        },
    );
}

/// What a scan found of /tmp alone, for tests.
#[doc(hidden)]
pub fn found_tmp(tmp: rush_fleet::Scratch) -> Found {
    Found {
        wts: Vec::new(),
        tmp: Some(tmp),
        agent_tmp: HashMap::new(),
        peeks: HashMap::new(),
        outside: Vec::new(),
        orphans: Vec::new(),
        measured: true,
    }
}
