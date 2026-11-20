//! The Projects place itself: the list on the left, what's picked on the
//! right, the keys and the bottom row's hint. Go: internal/ui/projects.go
//! (endProc :148, pickInProjects :311, projectsOpen :317, projectsBody
//! :849, projCur :900, projectsHint :910 and its memory row :927,
//! projectsKey :955 and enter on a memory row :1003),
//! projects_paths.go (askRemovePath :228, removed :262, emptyScratch
//! :287), keys.go (endOrphans :1226, killTree :1254) and picker.go:301
//! (browse).

use rush_core::text::Line;
use rush_fleet::Role;

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
pub struct Projects {
    pub proj_in: bool,
    pub proj_pos: usize,
    pub proj_sel: String,
    pub in_pos: usize,
    pub in_sel: String,
}

/// Picks the list's row id ("" for the first project), from outside what's
/// picked: taken up the next time the place draws or has a key.
pub fn pick_in_projects(m: &mut Model, id: &str) {
    let d = m.kept::<Data>();
    d.pick = Some(id.to_string());
    // What it's in opens, so it's there to pick: This Mac for Temporary,
    // System and the plugins' items, a worktree's repository for it.
    if id == PANE_TEMP || id == PANE_SYSTEM || id.starts_with('k') {
        d.unfolded.insert(GROUP_MAC.to_string());
    }
    if let Some(path) = id.strip_prefix('w')
        && let Some(repo) = d
            .wts
            .iter()
            .find(|t| t.path == path)
            .map(|t| t.repo.clone())
    {
        d.folded.remove(&repo);
    }
}

/// The row picked on the Projects page, by id (the command bar's spot).
pub fn projects_sel(m: &Model) -> String {
    let d = data(m);
    d.pick.clone().unwrap_or_else(|| d.sel.clone())
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
        first_project(&rows, &mut self.proj_pos, &mut self.proj_sel);
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
        // The tree, capped so a row's verdict sits near its name
        // (Rust-only: Go's rail look always stacked).
        let mut lw = (w * 3 / 5).clamp(72, 112);
        let mut rw = w.saturating_sub(lw + 1);
        let stack = w < 130;
        if stack {
            (lw, rw) = (w, w);
        }
        let list = project_list(m, lw.saturating_sub(4));
        first_project(&list, &mut self.proj_pos, &mut self.proj_sel);
        let pick = pick_row(&list, &mut self.proj_pos, &mut self.proj_sel);
        // A worktree's row, or its subagents' fold, shows its repository.
        // This Mac's Temporary and System show themselves.
        let shown = match pick.map(|i| &list[i]) {
            Some(r) if r.owner == GROUP_MAC && r.what == What::Pane => r.id.clone(),
            Some(r) if r.owner == GROUP_MAC => GROUP_MAC.to_string(),
            Some(r) if !r.owner.is_empty() => format!("p{}", r.owner),
            _ => self.proj_sel.clone(),
        };
        // The boxes run to the bottom of the screen, whatever is in them.
        let fill = h.saturating_sub(out.len() + 3);
        let detail = picked_rows(m, &shown, rw.saturating_sub(4));
        let dpick = if self.proj_in {
            pick_row(&detail, &mut self.in_pos, &mut self.in_sel)
        } else {
            None
        };
        let left = draw_rows(&list, pick, lw.saturating_sub(4), !self.proj_in);
        let right = draw_rows(&detail, dpick, rw.saturating_sub(4), self.proj_in);
        let bold = |s: &str| ln([spb(s, TEXT)]);
        let (title, meta) = match shown.as_str() {
            PANE_TEMP => (
                cat(ln([kind_mark(Kind::Temp), plain(" ")]), bold("Temporary")),
                ln([sp("/tmp and all sessions’ temp work", DIM)]),
            ),
            PANE_SYSTEM => (bold("System"), ln([sp("processes no project owns", DIM)])),
            GROUP_MAC => (
                bold("This Mac"),
                ln([sp("temp work, caches, processes", DIM)]),
            ),
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
        projects_hint(m, &r, me.proj_in, m.w.saturating_sub(4))
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

/// The bottom row's keys for the row r the cursor is on, in the list or
/// gone into what's picked (inside): only what works there. enter opens
/// or folds, x cleans up the row, c everything that can go.
pub fn projects_hint(m: &Model, r: &WorkRow, inside: bool, w: usize) -> Line {
    let d = data(m);
    let fold = |open: bool| if open { "fold" } else { "unfold" };
    let mut k: Vec<(&str, &str)> = vec![("↑↓", "move")];
    match &r.what {
        What::Pr(_) => k.extend([
            ("enter", "open it"),
            ("o", "on GitHub"),
            ("r", "ask GitHub again"),
        ]),
        What::Proc(_) => {
            k.extend([("x", "end"), ("!", "SIGKILL tree")]);
            if d.machine.orphans > 0 {
                k.push(("X", "end all orphans"));
            }
        }
        What::Wt(_) if inside => k.extend([("x", "clean up…"), ("r", "check again")]),
        What::Wt(_) => k.extend([
            ("enter", "details"),
            ("x", "clean up…"),
            ("r", "check again"),
        ]),
        What::Tmp => k.extend([("x", "clean up…"), ("r", "look again")]),
        What::Temp(key) => k.extend([
            (
                "enter",
                if d.opened.contains(key) {
                    "close it"
                } else {
                    "open it"
                },
            ),
            ("x", "clean up…"),
        ]),
        What::Path(_) => k.push(("x", "clean up…")),
        What::Proj(key) if !inside && repo_folds(m, key) => k.extend([
            ("enter", fold(!d.folded.contains(key))),
            ("→", "details"),
            ("x", "clean up…"),
        ]),
        What::Proj(_) => k.push(("enter", "details")),
        What::Pane if r.id == PANE_PRS => k.push(("enter", "details")),
        What::Pane => k.extend([("enter", "details"), ("x", "clean up…")]),
        What::Agent(_) => k.extend([
            ("enter", "open"),
            ("x", "close it…"),
            ("ctrl+y", "its PR"),
            ("ctrl+b", "keep going"),
        ]),
        What::Fold(key) | What::Group(key) => k.extend([
            ("enter", fold(d.unfolded.contains(key))),
            ("x", "clean up…"),
        ]),
        What::Kit(_) => k.push(("x", "clean up…")),
        What::Mem(_) => k.push(("enter", "review its memory")),
        What::Text => k.clear(),
    }
    k.push(("c", "clean up all that can go"));
    k.push(if inside {
        ("←", "the list")
    } else {
        ("esc", "back")
    });
    keys_fit(w, &k)
}

/// Whether a project's head folds: a repository with worktrees under it.
fn repo_folds(m: &Model, key: &str) -> bool {
    picked_project(m, &format!("p{key}"))
        .is_some_and(|p| p.repo && !repo_worktrees(data(m), &p).is_empty())
}

/// What x on r cleans up: the row's repository, worktree, or This Mac.
fn scope_of(m: &Model, r: &WorkRow) -> Option<clean::Scope> {
    let repo = |key: &str| clean::Scope::Repo {
        key: key.to_string(),
        wts: data(m)
            .wts
            .iter()
            .filter(|t| t.repo == key)
            .map(|t| t.path.clone())
            .collect(),
    };
    match &r.what {
        What::Proj(key) if repo_folds(m, key) => Some(repo(key)),
        What::Fold(key) => Some(repo(key)),
        What::Wt(t) => Some(clean::Scope::Worktree(t.path.clone())),
        What::Group(_) => Some(clean::Scope::Mac),
        What::Pane if r.id != PANE_PRS => Some(clean::Scope::Mac),
        What::Kit(k) if k.item.worktree.is_empty() => Some(clean::Scope::Mac),
        What::Kit(k) => Some(clean::Scope::Worktree(k.item.worktree.clone())),
        _ => None,
    }
}

impl Projects {
    fn key_on(&mut self, m: &mut Model, k: &Press) -> Out {
        let (rows, i) = self.cur(m);
        let r = i.map(|i| rows[i].clone()).unwrap_or_default();
        match k.s.as_str() {
            "left" if !self.proj_in && self.fold_back(m, &rows, &r) => {}
            "esc" | "left" => {
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
            // enter opens: a repository or group folds and unfolds, a
            // worktree shows its details, an agent its chat, a PR its
            // sheet. It never removes anything.
            "enter" => match &r.what {
                What::Proj(key) if !self.proj_in && repo_folds(m, key) => {
                    let d = m.kept::<Data>();
                    if !d.folded.remove(key) {
                        d.folded.insert(key.clone());
                    }
                }
                What::Fold(key) | What::Group(key) => {
                    let d = m.kept::<Data>();
                    if !d.unfolded.remove(key) {
                        d.unfolded.insert(key.clone());
                    }
                }
                What::Proj(_) | What::Pane | What::Wt(_) if !self.proj_in => self.go_in(m, &r),
                What::Agent(key) => return open_agent(m, key),
                What::Temp(key) => scan::open_temp(m, key),
                What::Pr(p) => super::prsheet::open_pr_sheet(m, &p.root, &p.pr),
                What::Mem(s) => {
                    let title = std::path::Path::new(&s.root)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    super::memreview::open_mem_review(m, s.p.clone(), &s.root, &title)
                }
                What::Kit(k) => m.flash(&format!("{} · x cleans it up", k.item.does)),
                _ => return Out::Idle,
            },
            // → goes in: a folded repository or group opens first.
            "right" => match &r.what {
                What::Proj(key) if m.kept::<Data>().folded.remove(key) => {}
                What::Fold(key) | What::Group(key) => {
                    m.kept::<Data>().unfolded.insert(key.clone());
                }
                What::Proj(_) | What::Pane | What::Wt(_) if !self.proj_in => self.go_in(m, &r),
                What::Agent(key) => return open_agent(m, key),
                _ => return Out::Idle,
            },
            "o" => match &r.what {
                What::Pr(p) => browse(m, &p.pr.url),
                _ => return Out::Idle,
            },
            // projects.go:1018: its pull request, or that there's none.
            "ctrl+y" => {
                let a = match &r.what {
                    What::Agent(key) => m.agents.iter().find(|a| a.key == **key).cloned(),
                    _ => None,
                };
                return m.ext.calls.open_pr()(m, a.as_ref());
            }
            "ctrl+b" | "alt+g" | "g" => match &r.what {
                // The agent's own keep going, as the list's ctrl+b
                // (projects.go:1020 keepGoing): g is Projects' short key
                // for it.
                What::Agent(key) => {
                    return list_key(m, key, &Press::of("ctrl+b").unwrap());
                }
                _ => return Out::Idle,
            },
            // x cleans up the row: one sheet, scoped to it.
            "x" | "ctrl+x" | "backspace" | "delete" => match &r.what {
                What::Proc(p) => end_proc(m, (**p).clone()),
                What::Tmp => delete::ask_clear_scratch(m),
                What::Temp(key) => delete::ask_clean(m, key),
                What::Path(p) => ask_remove_path(m, p.clone()),
                // Always asked: hide, stop, restart or delete
                // (projects.go:1034 askClose).
                What::Agent(key) => {
                    let a = m.agents.iter().find(|a| a.key == **key).cloned();
                    return m.ext.calls.ask_close()(m, a);
                }
                _ => match scope_of(m, &r) {
                    Some(scope) => clean::open_clean_sheet_for(m, scope),
                    None => return Out::Idle,
                },
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
            "X" if matches!(r.what, What::Proc(_)) => end_all_orphans(m),
            "c" => clean::open_clean_sheet(m),
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

impl Projects {
    /// Goes into what r is: a project or pane's rows on the right, or a
    /// worktree's, the cursor on its own row there.
    fn go_in(&mut self, m: &mut Model, r: &WorkRow) {
        self.proj_in = true;
        self.in_pos = 0;
        self.in_sel = match &r.what {
            What::Wt(t) => format!("w{}", t.path),
            _ => String::new(),
        };
        let (rows, j) = self.cur(m);
        if j.is_none_or(|j| rows[j].id.is_empty()) {
            self.proj_in = false; // nothing in it to pick
        }
    }

    /// ← on an unfolded fold, or on a worktree inside one, folds it and
    /// puts the cursor on the fold; false when ← isn't that.
    fn fold_back(&mut self, m: &mut Model, rows: &[WorkRow], r: &WorkRow) -> bool {
        // A repository folds, and This Mac, from its head or a row in it.
        let group = match &r.what {
            What::Proj(key) if !m.kept::<Data>().folded.contains(key) => {
                Some((key.clone(), format!("p{key}"), true))
            }
            What::Group(key) => Some((key.clone(), key.clone(), false)),
            _ if r.owner == GROUP_MAC => {
                Some((GROUP_MAC.to_string(), GROUP_MAC.to_string(), false))
            }
            _ => None,
        };
        if let Some((key, id, repo)) = group {
            let d = m.kept::<Data>();
            let changed = if repo {
                d.folded.insert(key)
            } else {
                d.unfolded.remove(&key)
            };
            if !changed {
                return false;
            }
            if let Some(i) = rows.iter().position(|r| r.id == id) {
                self.proj_pos = i;
            }
            self.proj_sel = id;
            return true;
        }
        let key = match &r.what {
            What::Fold(key) => key.clone(),
            What::Wt(t) if t.made == rush_fleet::MADE_BY_SUBAGENT => r.owner.clone(),
            _ => return false,
        };
        if !m.kept::<Data>().unfolded.remove(&key) {
            return false;
        }
        let id = fold_id(&key);
        if let Some(i) = rows.iter().position(|r| r.id == id) {
            self.proj_pos = i;
        }
        self.proj_sel = id;
        true
    }
}

/// With nothing picked yet, the list's first project, not the panes the
/// tree puts above the projects.
fn first_project(rows: &[WorkRow], pos: &mut usize, sel: &mut String) {
    if !sel.is_empty() {
        return;
    }
    if let Some(i) = rows.iter().position(|r| matches!(r.what, What::Proj(_))) {
        *pos = i;
        *sel = rows[i].id.clone();
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
pub fn browse(m: &mut Model, url: &str) {
    let url = url.to_string();
    m.later(
        {
            let url = url.clone();
            move || {
                std::process::Command::new("open")
                    .arg(&url)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .map_err(|e| e.to_string())
                    .and_then(|o| {
                        let s = o.status;
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
                Err(e) => m.fail(&format!("couldn't open {url}: {e}")),
            }
            Out::Draw
        },
    );
}

/// Asks before ending a System process: an orphan's whole tree, or one
/// process with SIGTERM.
pub fn end_proc(m: &mut Model, r: ProcRow) {
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
                    Err(e) => m.fail(&e),
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
                Err(e) => m.fail(&e),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// Ends each row's tree, SIGTERM then SIGKILL after 3s, and says what it
/// freed.
pub fn end_orphans(m: &mut Model, rows: Vec<ProcRow>, why: &'static str) -> Out {
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
                Err(e) => m.fail(&e),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// Asks before deleting the thing a row stands for.
pub fn ask_remove_path(m: &mut Model, p: PathRow) {
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
            m.fail(&e);
            return Out::Draw;
        }
        if !p.temp.is_empty()
            && freed > 0
            && let Some(t) = st.temp.get_mut(&p.temp)
        {
            (t.bytes, t.known) = ((t.bytes - freed).max(0), true);
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

/// Removes p, made writable first where it must be, never what rush never
/// touches (rush_fleet::remove_tree).
fn remove_all(p: &std::path::Path) -> Result<(), String> {
    rush_fleet::remove_tree(&p.to_string_lossy())
}

/// Empties a project's agent scratch folder, keeping it: only ever a tmp
/// folder in one of the agents' project folders, or one of rush's own.
pub fn empty_scratch(dir: &str, project_folders: &[String]) -> Result<(), String> {
    let p = std::path::Path::new(dir);
    let base = |q: Option<&std::path::Path>| {
        q.and_then(|q| q.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let in_project = base(Some(p)) == "tmp" && project_folders.contains(&base(p.parent()));
    let in_rush_tmp = p
        .parent()
        .is_some_and(|q| rush_state::scratch_roots().iter().any(|r| r == q));
    if !in_project && !in_rush_tmp {
        return Err(format!("won't empty {dir}"));
    }
    for e in std::fs::read_dir(p).map_err(|e| e.to_string())?.flatten() {
        remove_all(&e.path())?;
    }
    Ok(())
}
