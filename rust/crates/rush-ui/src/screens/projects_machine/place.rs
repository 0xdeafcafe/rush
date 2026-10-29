//! The Projects place itself: the list on the left, what's picked on the
//! right, the keys and the bottom row's hint. Go: internal/ui/projects.go
//! (endProc :148, pickInProjects :311, projectsOpen :317, projectsBody
//! :849, projCur :900, projectsHint :910, projectsKey :955),
//! projects_paths.go (askRemovePath :228, removed :262, emptyScratch
//! :287), keys.go (endOrphans :1226, killTree :1254) and picker.go:301
//! (browse).

use rush_core::text::Line;
use rush_fleet::machine::Role;

use super::data::*;
use super::rows::*;
use super::scan;
use super::view::*;
use super::{clean, delete};
use crate::frame::confirm::Confirm;
use crate::frame::places::{Place, PlaceId};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// The Projects place: where the list's cursor is, and the one inside
/// what's picked once enter has gone into it.
#[derive(Default)]
pub(crate) struct Projects {
    pub proj_in: bool,
    pub proj_pos: usize,
    pub proj_sel: String,
    pub in_pos: usize,
    pub in_sel: String,
}

/// Picks the list's row id ("" for the first project), from outside what's
/// picked: taken up the next time the place draws or has a key.
pub fn pick_in_projects(m: &mut Model, id: &str) {
    m.kept::<Data>().pick = Some(id.to_string());
}

/// The row picked on the Projects page, by id (the command bar's spot).
pub fn projects_sel(m: &Model) -> String {
    data(m).sel.clone()
}

impl Projects {
    fn take_pick(&mut self, m: &Model) {
        if let Some(id) = data(m).pick.clone() {
            self.proj_in = false;
            self.proj_pos = 0;
            self.proj_sel = id;
        }
    }

    /// The rows the cursor is in and the one it's on: in what's picked,
    /// once gone into it, or in the list (projCur).
    fn cur(&mut self, m: &Model) -> (Vec<WorkRow>, Option<usize>) {
        let w = m.w.saturating_sub(4);
        if self.proj_in {
            let rows = picked_rows(m, &self.proj_sel, w);
            let i = pick_row(&rows, &mut self.in_pos, &mut self.in_sel);
            return (rows, i);
        }
        let rows = project_list(m, w);
        let i = pick_row(&rows, &mut self.proj_pos, &mut self.proj_sel);
        (rows, i)
    }

    fn mv(&mut self, rows: &[WorkRow], i: Option<usize>, d: isize) {
        let Some(i) = i else { return };
        let (pos, sel) = if self.proj_in {
            (&mut self.in_pos, &mut self.in_sel)
        } else {
            (&mut self.proj_pos, &mut self.proj_sel)
        };
        let step = d.signum();
        let mut n = 0;
        let mut j = i as isize + step;
        while j >= 0 && (j as usize) < rows.len() {
            if rows[j as usize].pickable() {
                *pos = j as usize;
                *sel = rows[j as usize].id.clone();
                n += 1;
                if n == d.abs() {
                    break;
                }
            }
            j += step;
        }
    }
}

impl Place for Projects {
    fn name(&self) -> &str {
        "Projects"
    }

    fn enter(&mut self, m: &mut Model) {
        scan::refresh(m);
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        self.take_pick(m);
        let w = w.saturating_sub(2);
        let mut out = vec![summary_bar(m, w), Line::new()];
        let mut lw = (w * 2 / 5).clamp(64, 84);
        let mut rw = w.saturating_sub(lw + 1);
        let mut stack = w < 130;
        if stack || data(m).cfg.rail() {
            (lw, rw, stack) = (w, w, true);
        }
        let list = project_list(m, lw.saturating_sub(4));
        let pick = pick_row(&list, &mut self.proj_pos, &mut self.proj_sel);
        // The boxes run to the bottom of the screen, whatever is in them.
        let fill = h.saturating_sub(out.len() + 3);
        let detail = picked_rows(m, &self.proj_sel, rw.saturating_sub(4));
        let dpick = if self.proj_in {
            pick_row(&detail, &mut self.in_pos, &mut self.in_sel)
        } else {
            None
        };
        let left = draw_rows(&list, pick, lw.saturating_sub(4), !self.proj_in);
        let right = draw_rows(&detail, dpick, rw.saturating_sub(4), self.proj_in);
        let bold = |s: &str| ln([spb(s, TEXT)]);
        let (title, meta) = match self.proj_sel.as_str() {
            PANE_TEMP => (
                cat(ln([kind_mark(Kind::Temp), plain(" ")]), bold("Temporary")),
                ln([sp("/tmp and all sessions’ temp work", DIM)]),
            ),
            PANE_SYSTEM => (bold("System"), ln([sp("processes no project owns", DIM)])),
            PANE_PRS => {
                let (open, want) = pr_counts(m);
                (
                    cat(ln([sp("⇡", BLUE), plain(" ")]), bold("Pull requests")),
                    ln([sp(
                        format!("yours and asking for your review · {open} open · {want} want you"),
                        DIM,
                    )]),
                )
            }
            sel => match picked_project(m, sel) {
                Some(p) => {
                    let mark = kind_mark(if p.repo { Kind::Project } else { Kind::Folder });
                    (
                        cat(ln([mark, plain(" ")]), bold(&p.title)),
                        folder_meta(&p.agents, m.now),
                    )
                }
                None => (Line::new(), Line::new()),
            },
        };
        let n_projects = projects(m).len();
        let lbox = |left: Vec<Line>, n: usize| {
            pbox(
                bold("Projects"),
                ln([sp(n_projects.to_string(), DIM)]),
                pad_rows(left, n),
                lw,
                !self.proj_in,
            )
        };
        let rbox =
            |right: Vec<Line>, n: usize| pbox(title, meta, pad_rows(right, n), rw, self.proj_in);
        if stack {
            out.extend(lbox(left, 0));
            out.push(Line::new());
            out.extend(rbox(right, 0));
            return out;
        }
        let n = left.len().max(right.len()).max(fill);
        out.extend(side_by_side(lbox(left, n), rbox(right, n), lw));
        out
    }

    fn hint(&self, m: &Model, _w: usize) -> Line {
        let mut me = Projects {
            proj_in: self.proj_in,
            proj_pos: self.proj_pos,
            proj_sel: self.proj_sel.clone(),
            in_pos: self.in_pos,
            in_sel: self.in_sel.clone(),
        };
        let (rows, i) = me.cur(m);
        let r = i.map(|i| rows[i].clone()).unwrap_or_default();
        projects_hint(m, &r, m.w.saturating_sub(4))
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        self.take_pick(m);
        if let Some(st) = m.kept.peek::<Data>()
            && st.pick.is_some()
        {
            m.kept::<Data>().pick = None;
        }
        let out = self.key_on(m, k);
        m.kept::<Data>().sel = self.proj_sel.clone();
        out
    }
}

/// The bottom row's keys for the row r the cursor is on (projectsHint).
pub(crate) fn projects_hint(m: &Model, r: &WorkRow, w: usize) -> Line {
    let d = data(m);
    match &r.what {
        What::Pr(_) => keys_fit(
            w,
            &[
                ("↑↓", "move"),
                ("enter", "open it"),
                ("o", "on GitHub"),
                ("r", "ask GitHub again"),
                ("←", "back"),
            ],
        ),
        What::Proc(_) => {
            let mut k = vec![("↑↓", "move"), ("x", "end"), ("!", "SIGKILL tree")];
            if d.machine.orphans > 0 {
                k.push(("X", "end all orphans"));
            }
            k.push(("esc", "back"));
            keys_fit(w, &k)
        }
        What::Wt(_) => keys_fit(
            w,
            &[
                ("↑↓", "move"),
                ("x", "remove…"),
                ("c", "clean up"),
                ("r", "check again"),
                ("esc", "back"),
            ],
        ),
        What::Tmp => keys_fit(
            w,
            &[
                ("↑↓", "move"),
                ("x", "clear untouched…"),
                ("c", "clean up"),
                ("r", "look again"),
                ("esc", "back"),
            ],
        ),
        What::Temp(key) => {
            let open = if d.opened.contains(key) {
                "close it"
            } else {
                "open it"
            };
            keys_fit(
                w,
                &[
                    ("↑↓", "move"),
                    ("enter", open),
                    ("x", "delete all of it…"),
                    ("X", "every finished agent's…"),
                    ("esc", "back"),
                ],
            )
        }
        What::Path(p) => {
            let x = if p.scratch.is_empty() {
                "delete it…"
            } else {
                "empty it…"
            };
            keys_fit(w, &[("↑↓", "move"), ("x", x), ("←", "back")])
        }
        What::Proj(_) | What::Pane => keys_fit(
            w,
            &[
                ("↑↓", "move"),
                ("enter", "into it"),
                ("c", "clean up"),
                ("esc", "back"),
            ],
        ),
        What::Agent(key) => {
            let mut k = vec![("↑↓", "move"), ("enter", "open"), ("x", "close it…")];
            if let Some(g) = agent_by_key(m, key)
                && d.temp_of(key) >= TEMP_SHOWN
                && g.a.pid == 0
            {
                k.push(("t", "remove temp work"));
            }
            k.extend([
                ("ctrl+y", "its PR"),
                ("ctrl+b", "keep going"),
                ("←", "the list"),
                ("esc", "back"),
            ]);
            keys_fit(w, &k)
        }
        What::Text => keys_fit(w, &[("esc", "back")]),
    }
}

impl Projects {
    fn key_on(&mut self, m: &mut Model, k: &Press) -> Out {
        let (rows, i) = self.cur(m);
        let r = i.map(|i| rows[i].clone()).unwrap_or_default();
        match k.s.as_str() {
            "esc" | "q" | "left" => {
                if self.proj_in {
                    self.proj_in = false;
                    return Out::Draw;
                }
                return Out::Close;
            }
            "up" | "k" => self.mv(&rows, i, -1),
            "down" | "j" => self.mv(&rows, i, 1),
            "pgup" => self.mv(&rows, i, -10),
            "pgdown" => self.mv(&rows, i, 10),
            "enter" | "right" => match &r.what {
                What::Proj(_) | What::Pane => {
                    self.proj_in = true;
                    self.in_pos = 0;
                    self.in_sel.clear();
                    let (rows, j) = self.cur(m);
                    if j.is_none_or(|j| rows[j].id.is_empty()) {
                        self.proj_in = false; // nothing in it to pick
                    }
                }
                What::Agent(key) => return open_agent(m, key),
                What::Wt(wt) => delete::ask_remove_worktree(m, (**wt).clone()),
                What::Tmp => delete::ask_clear_scratch(m),
                What::Temp(key) => scan::open_temp(m, key),
                What::Path(p) => ask_remove_path(m, p.clone()),
                What::Pr(p) => browse(m, &p.pr.url),
                _ => return Out::Idle,
            },
            "o" => match &r.what {
                What::Pr(p) => browse(m, &p.pr.url),
                _ => return Out::Idle,
            },
            "ctrl+y" | "ctrl+b" | "alt+g" | "g" => match &r.what {
                // The agent's own keys, as the list has them.
                What::Agent(key) => return list_key(m, key, k),
                _ => return Out::Idle,
            },
            "x" | "ctrl+x" | "backspace" | "delete" => match &r.what {
                What::Wt(wt) => delete::ask_remove_worktree(m, (**wt).clone()),
                What::Proc(p) => end_proc(m, (**p).clone()),
                What::Tmp => delete::ask_clear_scratch(m),
                What::Temp(key) => delete::ask_clean(m, key),
                What::Path(p) => ask_remove_path(m, p.clone()),
                What::Agent(key) => return list_key(m, key, &Press::of("ctrl+x").unwrap()),
                _ => return Out::Idle,
            },
            "t" => match &r.what {
                What::Agent(key) if data(m).temp_of(key) >= TEMP_SHOWN => delete::ask_clean(m, key),
                _ => return Out::Idle,
            },
            "!" => {
                if let What::Proc(p) = &r.what {
                    let (pid, start) = (p.pid, p.start);
                    m.ask(
                        Confirm::new(
                            format!("SIGKILL {pid} and everything under it?"),
                            move |m| kill_tree(m, pid, start),
                        )
                        .detail(trim_cmd(&p.cmd, 80)),
                    );
                }
            }
            "X" => {
                if let What::Temp(_) = r.what {
                    delete::ask_clean_all(m);
                } else {
                    end_all_orphans(m);
                }
            }
            "A" | "c" => clean::open_clean_sheet(m),
            "r" => {
                if matches!(r.what, What::Pr(_)) || self.proj_sel == PANE_PRS {
                    scan::ask_prs_again(m);
                } else {
                    // /tmp too, not only when it's due.
                    m.kept::<Data>().tmp.checked = 0;
                    scan::scan_worktrees(m);
                }
            }
            _ => return Out::Pass,
        }
        Out::Draw
    }
}

/// Goes to an agent's conversation from its row (focusPane).
fn open_agent(m: &mut Model, key: &str) -> Out {
    m.set_place(PlaceId::Agents);
    m.sel = key.to_string();
    m.rebuild();
    if let Some(i) = m.agents.iter().position(|a| a.key == key) {
        m.open(i);
    }
    Out::Draw
}

/// A key on an agent's row that's the list's: the agent picked, as the
/// list would have it, and the list's hooks given the key.
fn list_key(m: &mut Model, key: &str, k: &Press) -> Out {
    let was = std::mem::replace(&mut m.sel, key.to_string());
    for i in 0..m.ext.list_keys.len() {
        let h = m.ext.list_keys[i];
        let o = h(m, k);
        if o != Out::Pass {
            return o;
        }
    }
    m.sel = was;
    m.flash("that isn't in this rush yet");
    Out::Draw
}

/// Opens url in the browser, off the UI.
pub(crate) fn browse(m: &mut Model, url: &str) {
    let url = url.to_string();
    m.later(
        {
            let url = url.clone();
            move || {
                std::process::Command::new("open")
                    .arg(&url)
                    .status()
                    .map_err(|e| e.to_string())
                    .and_then(|s| {
                        if s.success() {
                            Ok(())
                        } else {
                            Err(format!("open said {s}"))
                        }
                    })
            }
        },
        move |m, r| {
            match r {
                Ok(()) => m.flash(&format!("opened {url}")),
                Err(e) => m.flash_err(&format!("couldn't open {url}: {e}")),
            }
            Out::Draw
        },
    );
}

/// Asks before ending a System process: an orphan's whole tree, or one
/// process with SIGTERM.
pub(crate) fn end_proc(m: &mut Model, r: ProcRow) {
    let (pid, start) = (r.pid, r.start);
    if r.role == Role::Orphan && r.other {
        let q = format!(
            "End {} and everything under it, freeing about {}?",
            trim_cmd(&r.cmd, 40),
            mem(r.mem)
        );
        let mut c = Confirm::new(q, move |m| end_orphans(m, vec![r], "you ended it"))
            .detail("SIGTERM, then SIGKILL after 3s");
        c.bang_text = "SIGKILL it now".into();
        c.on_bang = Some(Box::new(move |m| kill_tree(m, pid, start)));
        m.ask(c);
        return;
    }
    let mut c = Confirm::new(format!("Send SIGTERM to {pid}?"), move |m| {
        m.later(
            move || rush_sys::actions::terminate(pid).map_err(|e| e.to_string()),
            move |m, r| {
                match r {
                    Ok(()) => m.flash(&format!("sent SIGTERM to {pid}")),
                    Err(e) => m.flash_err(&e),
                }
                Out::Draw
            },
        );
        Out::Draw
    })
    .detail(trim_cmd(&r.cmd, 80));
    c.bang_text = "SIGKILL it and everything under it".into();
    c.on_bang = Some(Box::new(move |m| kill_tree(m, pid, start)));
    m.ask(c);
}

/// X on System: every orphaned tree ended, once asked.
fn end_all_orphans(m: &mut Model) {
    let mc = &data(m).machine;
    if mc.orphans == 0 {
        return;
    }
    let freed = mc.orphan_mem;
    let ends: Vec<ProcRow> = proc_rows(m)
        .into_iter()
        .filter(|p| p.role == Role::Orphan && p.other)
        .collect();
    let q = format!(
        "End all {} orphaned process trees and free about {}?",
        ends.len(),
        mem(freed)
    );
    m.ask(
        Confirm::new(q, move |m| end_orphans(m, ends, "you ended them all"))
            .detail("SIGTERM, then SIGKILL after 3s"),
    );
}

/// SIGKILLs pid and everything under it.
fn kill_tree(m: &mut Model, pid: i32, start: std::time::SystemTime) -> Out {
    m.later(
        move || rush_sys::actions::kill_tree(pid, start).map_err(|e| e.to_string()),
        |m, r| {
            match r {
                Ok(n) => m.flash(&format!("killed {n} processes")),
                Err(e) => m.flash_err(&e),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// Ends each row's tree, SIGTERM then SIGKILL after 3s, and says what it
/// freed.
fn end_orphans(m: &mut Model, rows: Vec<ProcRow>, why: &'static str) -> Out {
    m.later(
        move || {
            let (mut n, mut freed, mut errs) = (0, 0u64, Vec::new());
            for r in &rows {
                match rush_sys::actions::end_tree(r.pid, r.start, std::time::Duration::from_secs(3))
                {
                    Ok(k) => {
                        n += k;
                        freed += r.mem;
                    }
                    Err(e) => errs.push(e.to_string()),
                }
            }
            rush_sys::events::log(
                "orphans-ended",
                serde_json::json!({
                    "pids": rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
                    "ended": n, "freed": freed, "why": why, "err": errs.join("; "),
                }),
            );
            if n == 0 && !errs.is_empty() {
                Err(errs.join("; "))
            } else {
                Ok(format!("ended {n} processes · freed about {}", mem(freed)))
            }
        },
        |m, r| {
            match r {
                Ok(s) => m.flash(&s),
                Err(e) => m.flash_err(&e),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// Asks before deleting the thing a row stands for.
pub(crate) fn ask_remove_path(m: &mut Model, p: PathRow) {
    let st = data(m);
    let size = st
        .sizes
        .get(&p.path)
        .map(|n| format!(", {}", disk(*n)))
        .unwrap_or_default();
    let base = p.path.rsplit('/').next().unwrap_or(&p.path).to_string();
    if !p.temp.is_empty() {
        let Some(g) = agent_by_key(m, &p.temp) else {
            return;
        };
        let name = g.a.display_name.clone();
        if g.a.pid != 0 {
            m.flash_err(&format!(
                "{name} is still running; its temp work may be in use · stop it first"
            ));
            return;
        }
        let dirs: Vec<String> = st
            .temp
            .get(&p.temp)
            .map(|t| t.dirs.iter().map(|d| d.path.clone()).collect())
            .unwrap_or_default();
        let roots: Vec<String> = m
            .reg
            .all()
            .iter()
            .filter_map(|a| a.scratcher().map(|s| s.scratch_root()))
            .collect();
        let path = p.path.clone();
        let from = format!("from {name}'s temp work · the rest of it stays");
        m.ask(
            Confirm::new(format!("Delete {base}{size}?"), move |m| {
                removed(m, p, move || remove_temp_entry(&dirs, &roots, &path, &name))
            })
            .detail(from),
        );
        return;
    }
    if !p.scratch.is_empty() {
        let Some(proj) = picked_project(m, &p.scratch) else {
            return;
        };
        if proj.agents.iter().any(|g| g.a.pid != 0) {
            let title = proj.title.clone();
            m.flash_err(&format!(
                "an agent in {title} is running and may be using its scratch · stop it first"
            ));
            return;
        }
        let detail = format!(
            "{} · what agents kept as scratch here; the folder stays",
            peek_line(m, &p.path, m.now)
        );
        let folders = m.reg.project_folders();
        let path = p.path.clone();
        m.ask(
            Confirm::new(format!("Empty {}?", tildify(&p.path)), move |m| {
                removed(m, p, move || empty_scratch(&path, &folders))
            })
            .detail(detail),
        );
    }
}

/// Runs a deletion off the UI, then forgets what was known of the path.
fn removed(
    m: &mut Model,
    p: PathRow,
    rm: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Out {
    let freed = data(m).sizes.get(&p.path).copied().unwrap_or(0);
    m.later(rm, move |m, r| {
        let st = m.kept::<Data>();
        st.sizes.remove(&p.path);
        st.peeks.remove(&p.path);
        if let Some((ents, _)) = st.lists.get_mut(&p.temp) {
            ents.retain(|e| e.path != p.path);
        }
        if let Err(e) = r {
            m.flash_err(&e);
            return Out::Draw;
        }
        if !p.temp.is_empty()
            && freed > 0
            && let Some(t) = st.temp.get_mut(&p.temp)
        {
            t.bytes = (t.bytes - freed).max(0);
        }
        m.flash(&format!("deleted {}", tildify(&p.path)));
        Out::Draw
    });
    Out::Draw
}

/// Deletes one thing at the top of an agent's temp work, and only there
/// (fleet/temp.go:105).
fn remove_temp_entry(
    dirs: &[String],
    roots: &[String],
    path: &str,
    name: &str,
) -> Result<(), String> {
    let p = std::path::Path::new(path);
    for d in dirs {
        if !delete::deletable(d, roots) {
            continue;
        }
        if p.parent() == Some(std::path::Path::new(d.trim_end_matches('/'))) {
            return remove_all(p);
        }
    }
    Err(format!(
        "won't delete {path}: it isn't in {name}'s temp work"
    ))
}

fn remove_all(p: &std::path::Path) -> Result<(), String> {
    let r = if p.is_dir() && !p.is_symlink() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    match r {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// Empties a project's agent scratch folder, keeping it: only ever a tmp
/// folder in one of the agents' project folders, or one of rush's own.
pub(crate) fn empty_scratch(dir: &str, project_folders: &[String]) -> Result<(), String> {
    let p = std::path::Path::new(dir);
    let base = |q: Option<&std::path::Path>| {
        q.and_then(|q| q.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let in_project = base(Some(p)) == "tmp" && project_folders.contains(&base(p.parent()));
    let rush_tmp = crate::config::dir().join("tmp");
    if !in_project && p.parent() != Some(rush_tmp.as_path()) {
        return Err(format!("won't empty {dir}"));
    }
    for e in std::fs::read_dir(p).map_err(|e| e.to_string())?.flatten() {
        remove_all(&e.path())?;
    }
    Ok(())
}
