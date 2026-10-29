//! What the Projects place asks of git, GitHub, the disk and the process
//! table, each off the UI thread through Model::later: only while the
//! place is showing, and never more often than Go's (a folder's git every
//! 15s, worktrees every 2 minutes, GitHub when its answer is due, the
//! processes with each reading of the list). Go: internal/ui/folders.go
//! (refreshFolders :339, onFolders :393), cleanup.go (scanWorktrees :145,
//! onWorktrees :363, mergeChecks :378), prs.go (refreshPRs :63, onPRs
//! :101, askPRsAgain :148), projects_paths.go (peekDir :46, listTemp
//! :130, openTemp :150), tempwork.go (measureTemp :28) and model.go:1110
//! (the tick's rescans).

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

use rush_core::adapter::TempDir;
use rush_core::time::{Ms, now};
use rush_fleet::disk::{disk_usage, find_scratch, ms_of};
use rush_fleet::folder::{check_folder, folder_wants};
use rush_fleet::machine::{AgentProc, machine};
use rush_fleet::worktree::{WorkerOf, Worktree, find_worktrees};
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
pub(crate) fn refresh(m: &mut Model) {
    if !d(m).cfg_read {
        let st = d(m);
        st.cfg = Cfg::load();
        st.cfg_read = true;
    }
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
    if now() - d(m).checked > RESCAN_EVERY {
        scan_worktrees(m);
    }
    arm(m);
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
    m.later(
        || std::thread::sleep(std::time::Duration::from_millis(MACHINE_EVERY as u64)),
        |m, ()| {
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
pub(crate) fn refresh_folders(m: &mut Model) {
    if d(m).looking > 0 {
        return;
    }
    let wants = {
        let ags = work_agents(m);
        folder_wants(ags.iter().map(|g| (g.root, g.a.repo.as_str())))
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

/// The folders an agent is known to have worked in.
fn work_dirs(a: &rush_core::agent::Agent) -> Vec<String> {
    let mut out = a.spend.dirs.clone();
    for d in [&a.job.worktree_path, &a.job.cwd] {
        if !d.is_empty() {
            out.push(d.clone());
        }
    }
    out
}

/// An agent's temp folders: its agent's own scratch for the session.
fn temp_dirs(m: &Model, a: &rush_core::agent::Agent) -> Vec<TempDir> {
    let Some(ad) = m.reg.get(a.kind.as_str()) else {
        return Vec::new();
    };
    let Some(s) = ad.scratcher() else {
        return Vec::new();
    };
    let Some(p) = m
        .reg
        .profiles_of(&ad)
        .into_iter()
        .find(|p| p.name == a.job.account)
    else {
        return Vec::new();
    };
    let job = if !a.interactive && !a.past {
        a.job.id.as_str()
    } else {
        ""
    };
    s.scratch(&p, job, &a.job.session_id, &a.job.cwd)
}

/// What the agents' own scratch roots are called at the top of /tmp:
/// cleaned with their sessions, not as yours.
fn scratch_roots(m: &Model) -> Vec<String> {
    m.reg
        .all()
        .iter()
        .filter_map(|a| a.scratcher().map(|s| s.scratch_root()))
        .filter(|r| !r.is_empty())
        .map(|r| r.rsplit('/').next().unwrap_or(&r).to_string())
        .collect()
}

/// What the worktree scan found.
struct Found {
    wts: Vec<Worktree>,
    tmp: Option<rush_fleet::disk::Scratch>,
    agent_tmp: HashMap<String, i64>,
    peeks: HashMap<String, PathInfo>,
}

/// Finds every agent's worktree and checks each with git and measures it,
/// /tmp when it's due, the projects' agent scratch, and the tops of the
/// folders the page names.
pub(crate) fn scan_worktrees(m: &mut Model) {
    if d(m).checking || d(m).frozen {
        return;
    }
    d(m).checking = true;
    let workers: Vec<WorkerOf> = agents(m)
        .map(|g| WorkerOf {
            key: g.a.key.clone(),
            name: g.a.display_name.clone(),
            dirs: work_dirs(g.a),
            in_use: g.in_use(),
        })
        .collect();
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
    let own = scratch_roots(m);
    m.later(
        move || {
            let mut f = Found {
                wts: Vec::new(),
                tmp: None,
                agent_tmp: HashMap::new(),
                peeks: HashMap::new(),
            };
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
                f.tmp = Some(find_scratch("/tmp", &own));
            }
            let gh = Gh::shared();
            f.wts = find_worktrees(&workers, &|repo, branch| gh.for_branch(repo, branch));
            for w in &mut f.wts {
                w.check();
            }
            f
        },
        |m, f| {
            let st = d(m);
            st.checking = false;
            st.checked = now();
            st.wts = f.wts;
            st.agent_tmp = f.agent_tmp;
            st.peeks.extend(f.peeks);
            if let Some(t) = f.tmp {
                st.tmp = t;
            }
            Out::Draw
        },
    );
}

/// What the few biggest processes under each orphan run, in words: their
/// command lines, for System's rows (known.go:102 shortCmd,
/// readProcWords). Read with the table, off the UI thread.
fn kid_words(
    mc: &rush_fleet::machine::Machine,
    tab: &rush_sys::proc::Table,
) -> HashMap<i32, String> {
    let mut out = HashMap::new();
    for r in mc
        .rows
        .iter()
        .filter(|r| r.role == rush_fleet::machine::Role::Orphan)
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
                    t.bytes = b;
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
pub(crate) fn measure_temp(m: &mut Model) {
    if d(m).frozen {
        return;
    }
    let t = now();
    let mut due = Vec::new();
    let mut temp = HashMap::new();
    for g in agents(m) {
        let dirs = temp_dirs(m, g.a);
        if dirs.is_empty() {
            continue;
        }
        let st = data(m);
        if st.temp_sizes.loaded && st.temp_sizes.due(g.a, t) {
            due.push((g.a.key.clone(), dirs.clone()));
        }
        let bytes = st.temp_sizes.bytes(&g.a.key).unwrap_or(0);
        temp.insert(g.a.key.clone(), TempWork { dirs, bytes });
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
                    t.bytes = s.bytes;
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
pub(crate) fn peek_dir(dir: &str) -> PathInfo {
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
pub(crate) fn read_machine(m: &mut Model) {
    let st = d(m);
    if st.reading || st.frozen || now() - st.machine_at < MACHINE_EVERY {
        return;
    }
    st.reading = true;
    let prev = st.prev_table.take();
    let mut reaper = st.reaper.take().unwrap_or_default();
    let procs: Vec<AgentProc> = m
        .agents
        .iter()
        .filter(|a| a.pid != 0)
        .map(|a| AgentProc {
            pid: a.pid,
            key: a.key.clone(),
            name: a.display_name.clone(),
        })
        .collect();
    let reg = m.reg.clone();
    m.later(
        move || {
            let mut tab = rush_sys::proc::snapshot();
            let named: Vec<(i32, String)> = procs.iter().map(|a| (a.pid, a.name.clone())).collect();
            let left = reaper.scan(&tab, &named, std::process::id() as i32, &|c| {
                reg.is_program(c)
            });
            let program = reg.program_of("claude");
            let mc = machine(
                &mut tab,
                prev.as_ref(),
                &procs,
                &program,
                &|c| reg.is_program(c),
                &|pid, comm| {
                    let c = rush_sys::proc::command_line(pid);
                    if c.is_empty() { comm.to_string() } else { c }
                },
            );
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

/// Asks GitHub again of each repository on the page whose answer is due.
pub(crate) fn refresh_prs(m: &mut Model) {
    let roots: Vec<String> = projects(m)
        .into_iter()
        .filter(|p| p.repo)
        .map(|p| p.key)
        .collect();
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
pub(crate) fn ask_prs_again(m: &mut Model) {
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
pub(crate) fn open_temp(m: &mut Model, key: &str) {
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
