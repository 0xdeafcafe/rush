//! The Projects page's rows: the list (a table, or in the rail look a tree
//! of every repository's worktrees), and what's picked on the right — a
//! project whole, Temporary, System or every project's pull requests.
//! Go: internal/ui/projects.go (prLine :192, summaryBar :326,
//! projectsSummary :340, pickedRows :371, projectList :392,
//! classicProjectList :401, agentCounts :452, pickedProject :478,
//! projectDetail :498, wtCols :650, worktreeHead :655, worktreeLine :662,
//! tempRows :708, tempStatus :758, procs :775, systemRows :803),
//! projecttree.go (projectTree :28, projectTreeRows :53, repoWorktrees
//! :119, treeName :146, treeMark :159, treeWhat :182, treeVerdict :216,
//! wtTreeLine :234), workstreams.go (workSession :176, workSaid :236),
//! folders.go (folderMeta :131, gitBits :161), projects_paths.go
//! (peekLine :64, tempEntryRows :192, sessionEnded :220), prs.go
//! (prsOf :158, prCounts :181, prWantsYou :193, prsListLine :202,
//! prsRows :215, ghPRLine :243, prReviewWord :275) and view.go:3031
//! (roleName); projectDetail's memory (projects.go:522) is
//! projectmem.rs's.

use rush_core::text::{Color, Line, Span, width};
use rush_core::time::Ms;
use rush_fleet::GitState;
use rush_fleet::Predicates;
use rush_fleet::Role;
use rush_fleet::{
    MADE_BY_CLAUDE, MADE_BY_OTHER, MADE_BY_RUSH, MADE_BY_SUBAGENT, VerdictKind, Worktree,
};
use rush_gh::ghpr::PR;

use super::data::*;
use super::rows::*;
use crate::model::Model;
use crate::style::*;

const PROJ_NAME_W: usize = 22;
const PROJ_GIT_W: usize = 24;
const TREE_NAME_W: usize = 22;
const TREE_BRANCH_W: usize = 24;
const TREE_SIZE_W: usize = 7;
const TREE_VERD_W: usize = 34;

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn spin(m: &Model) -> Span {
    sp(SPINNER[m.tick % SPINNER.len()], ORANGE)
}

/// The page's top line: the machine's load and what can go without
/// losing anything, at the right.
pub(crate) fn summary_bar(m: &Model, w: usize) -> Line {
    let running = projects(m)
        .iter()
        .flat_map(|p| p.agents.iter())
        .filter(|g| g.live() || g.busy())
        .count();
    spread(Line::new(), projects_summary(m, running), w)
}

/// The machine's load and how much can go without losing anything.
pub(crate) fn projects_summary(m: &Model, running: usize) -> Line {
    let d = data(m);
    let mc = &d.machine;
    let mut safe: i64 = 0;
    for w in &d.wts {
        if w.verdict().can_go() && running_of(m, &w.agents).is_none() {
            safe += w.size;
        }
    }
    for g in agents(m) {
        if g.a.pid == 0 {
            let t = d.temp_of(&g.a.key);
            if t >= TEMP_SHOWN {
                safe += t;
            }
        }
    }
    safe += d.tmp.stale;
    let mut l = ln([
        sp(format!("● {running} running"), ORANGE),
        sp(
            format!(" · {} ram · {:.0}% cpu", mem(mc.total_mem), mc.total_cpu),
            DIM,
        ),
    ]);
    if d.checking && d.checked == 0 {
        l.spans
            .extend([sp(" · ", DIM), spin(m), sp(" asking git…", DIM)]);
    } else if safe > 0 {
        l.spans.extend([
            sp(" · ", DIM),
            sp(disk(safe), GREEN),
            sp(" can go ", DIM),
            sp("c", SUB),
        ]);
    }
    l
}

fn running_of<'a>(m: &'a Model, keys: &[String]) -> Option<Ag<'a>> {
    running(m, keys)
}

/// What the list's pick shows on the right.
pub(crate) fn picked_rows(m: &Model, sel: &str, w: usize) -> Vec<WorkRow> {
    match sel {
        PANE_TEMP => temp_rows(m, w),
        PANE_SYSTEM => system_rows(m, w),
        PANE_PRS => prs_rows(m, w),
        _ => match picked_project(m, sel) {
            Some(p) => project_detail(m, &p, w),
            None => Vec::new(),
        },
    }
}

/// The page's list: in the rail look a tree, else a table.
pub(crate) fn project_list(m: &Model, w: usize) -> Vec<WorkRow> {
    if data(m).cfg.rail() {
        project_tree(m, w)
    } else {
        classic_project_list(m, w)
    }
}

fn kind_of(p: &Project) -> Kind {
    if p.repo { Kind::Project } else { Kind::Folder }
}

/// Every project in one table: repositories, then the folders that aren't.
pub(crate) fn classic_project_list(m: &Model, w: usize) -> Vec<WorkRow> {
    let d = data(m);
    let list = projects(m);
    let now = m.now;
    let mut rows = vec![WorkRow::text(thead(&[
        ("  NAME", PROJ_NAME_W),
        ("GIT", PROJ_GIT_W),
        ("AGENTS", 10),
    ]))];
    if list.is_empty() {
        rows = vec![WorkRow::text(ln([sp(
            "no agent has worked anywhere in the last day",
            DIM,
        )]))];
    }
    for (i, p) in list.iter().enumerate() {
        if i > 0 && p.repo != list[i - 1].repo {
            rows.push(WorkRow::blank());
            rows.push(WorkRow::text(psection(
                "Other folders",
                "◇ not a repository",
                w,
            )));
        }
        let mut git = Line::new();
        match d.folders.get(&p.key) {
            Some(f) if p.repo => {
                git.spans.push(sp(f.git.branch.clone(), SUB));
                if f.git.changed > 0 {
                    git.spans.push(sp(format!(" ±{}", f.git.changed), YELLOW));
                }
                if f.worktrees > 0 {
                    git.spans.push(sp(format!(" ⎇{}", f.worktrees), BLUE));
                }
            }
            _ if p.repo => git.spans.push(sp("asking git…", FAINT)),
            _ => {}
        }
        let mut l = ln([kind_mark(kind_of(p)), plain(" ")]);
        join(
            &mut l,
            fit(ln([spb(p.title.clone(), TEXT)]), PROJ_NAME_W - 2),
        );
        join(&mut l, fit(git, PROJ_GIT_W));
        join(&mut l, agent_counts(&p.agents, now));
        rows.push(WorkRow {
            id: format!("p{}", p.key),
            what: What::Proj(p.key.clone()),
            line: l,
            owner: String::new(),
        });
    }
    // What's no project's: temp work, and processes.
    let temp: i64 = agents(m)
        .map(|g| d.temp_of(&g.a.key))
        .filter(|&t| t >= TEMP_SHOWN)
        .sum::<i64>()
        + d.tmp.size;
    let mut procs = ln([sp(format!("{} processes", proc_rows(m).len()), DIM)]);
    if d.machine.orphans > 0 {
        procs = cat(
            ln([
                sp(format!("{} orphaned", d.machine.orphans), YELLOW),
                sp(" · ", DIM),
            ]),
            procs,
        );
    }
    let name = |s: &str| fit(ln([spb(s, TEXT)]), PROJ_NAME_W - 2);
    rows.push(WorkRow::blank());
    rows.push(WorkRow::text(psection("Every project's", "", w)));
    rows.push(WorkRow {
        id: PANE_PRS.into(),
        what: What::Pane,
        line: prs_list_line(m),
        owner: String::new(),
    });
    rows.push(WorkRow::blank());
    rows.push(WorkRow::text(psection("No project's", "", w)));
    rows.push(WorkRow {
        id: PANE_TEMP.into(),
        what: What::Pane,
        line: cat(
            cat(ln([kind_mark(Kind::Temp), plain(" ")]), name("Temporary")),
            ln([sp(disk(temp), DIM)]),
        ),
        owner: String::new(),
    });
    rows.push(WorkRow {
        id: PANE_SYSTEM.into(),
        what: What::Pane,
        line: cat(cat(ln([sp("⚙", FAINT), plain(" ")]), name("System")), procs),
        owner: String::new(),
    });
    rows
}

/// A project's agents in a few cells: working, needing you, your turn,
/// then how many in all.
pub(crate) fn agent_counts(agents: &[Ag], now: Ms) -> Line {
    let (mut working, mut waiting, mut turn) = (0, 0, 0);
    for g in agents {
        if g.a.needs_you() || g.waiting() || g.a.halted() {
            waiting += 1;
        } else if g.a.your_turn(now) {
            turn += 1;
        } else if g.live() || g.busy() || g.a.checking {
            working += 1;
        }
    }
    let mut parts = Vec::new();
    if working > 0 {
        parts.push(ln([sp(format!("✱{working}"), ORANGE)]));
    }
    if waiting > 0 {
        parts.push(ln([spb(format!("?{waiting}"), YELLOW)]));
    }
    if turn > 0 {
        parts.push(ln([sp(format!("◆{turn}"), GREEN)]));
    }
    parts.push(ln([sp(format!("{} in all", agents.len()), DIM)]));
    joined(parts, plain(" "))
}

/// A folder section's count of what its agents are doing.
pub(crate) fn folder_meta(agents: &[Ag], now: Ms) -> Line {
    let (mut working, mut waiting, mut turn) = (0, 0, 0);
    let mut cost = 0.0;
    for g in agents {
        cost += g.a.spend.cost;
        if g.a.needs_you() || g.waiting() || g.a.halted() {
            waiting += 1;
        } else if g.a.your_turn(now) {
            turn += 1;
        } else if g.live() || g.busy() || g.a.checking {
            working += 1;
        }
    }
    let mut parts = Vec::new();
    if working > 0 {
        parts.push(ln([sp(format!("{working} working"), ORANGE)]));
    }
    if waiting > 0 {
        parts.push(ln([spb(format!("{waiting} need you"), YELLOW)]));
    }
    if turn > 0 {
        parts.push(ln([sp(format!("{turn} your turn"), GREEN)]));
    }
    let meta = if cost > 0.0 {
        format!("{} · {}", agents.len(), money(cost))
    } else {
        agents.len().to_string()
    };
    parts.push(ln([sp(meta, DIM)]));
    joined(parts, sp(" · ", DIM))
}

/// The project the list's cursor is on, else the first.
pub(crate) fn picked_project<'a>(m: &'a Model, sel: &str) -> Option<Project<'a>> {
    if sel.starts_with('~') {
        return None;
    }
    let key = sel.strip_prefix('p').unwrap_or(sel);
    let list = projects(m);
    if let Some(p) = list.iter().find(|p| p.key == key) {
        return Some(p.clone());
    }
    list.into_iter().next()
}

/// A checkout's state in a few cells: branch, distance from upstream,
/// what's uncommitted.
pub(crate) fn git_bits(s: &GitState) -> Line {
    if !s.err.is_empty() {
        return ln([sp("git failed", FAINT)]);
    }
    let mut parts = Vec::new();
    if !s.branch.is_empty() {
        let mut b = ln([sp(s.branch.clone(), SUB)]);
        if s.ahead > 0 {
            b.spans.push(sp(format!(" ↑{}", s.ahead), ORANGE));
        }
        if s.behind > 0 {
            b.spans.push(sp(format!(" ↓{}", s.behind), YELLOW));
        }
        parts.push(b);
    }
    if s.changed > 0 {
        parts.push(ln([sp(format!("{} changed", s.changed), DIM)]));
    } else {
        parts.push(ln([sp("clean", FAINT)]));
    }
    joined(parts, sp(" · ", DIM))
}

/// What a session last said: its summary, unless only a synthetic reply.
fn work_said(g: &Ag) -> String {
    let d = one_line(&g.a.job.detail);
    if d != "No response requested." {
        d
    } else {
        String::new()
    }
}

/// A session in Projects: its state, then how full its context, then how
/// long since it changed.
pub(crate) fn work_session(m: &Model, g: &Ag, w: usize, now: Ms) -> Line {
    let a = g.a;
    let (marker, state): (Span, Line) = if a.halted() {
        (
            sp("✗", RED),
            ln([sp(
                format!("stopped · {}", one_line(&a.halt_reason(now))),
                RED,
            )]),
        )
    } else if a.needs_you() || g.waiting() {
        let mut q = one_line(&a.job.needs);
        if q.is_empty() {
            q = one_line(&a.job.detail);
        }
        (sp("?", YELLOW), ln([sp("asks: ", YELLOW), sp(q, TEXT)]))
    } else if a.your_turn(now) {
        (
            sp("◆", GREEN),
            ln([sp("your turn · ", GREEN), sp(work_said(g), SUB)]),
        )
    } else if g.live() {
        let mut s = work_said(g);
        if let Some(p) = m.previews.get(&a.key)
            && !p.doing.is_empty()
        {
            s = one_line(&p.doing);
        }
        if s.is_empty() {
            s = "working".into();
        }
        (
            sp(SPINNER[(m.tick + a.job.id.len()) % SPINNER.len()], ORANGE),
            ln([sp(s, TEXT)]),
        )
    } else if g.busy() {
        let n = a.job.background.len();
        (
            sp("◎", BLUE),
            ln([sp(format!("{n} in the background"), BLUE)]),
        )
    } else {
        (sp("◦", DIM), ln([sp("idle", DIM)]))
    };
    let mut tail = Vec::new();
    if a.spend.context > 0 {
        let win = m.reg.context_window(a.kind.as_str(), &a.spend.model);
        if win > 0 {
            let pc = (a.spend.context * 100 / win) as usize;
            let c = if pc >= 80 { YELLOW } else { DIM };
            tail.push(ln([sp(format!("ctx {pc}%"), c)]));
        }
    }
    tail.push(ln([sp(right(&age(now - a.job.updated_at), 5), FAINT)]));
    let t = joined(tail, plain("  "));
    let left = ln([
        plain(" "),
        marker,
        plain(" "),
        spb(fit_str(&one_line(&a.display_name), 26), TEXT),
        plain(" "),
    ]);
    let room = w.saturating_sub(left.width() + t.width() + 2);
    let mut l = cat(left, fit(state, room));
    l.spans.push(plain("  "));
    cat(l, t)
}

/// A folder's top in words: how many things, when last touched; "looking…"
/// until the scan has looked.
pub(crate) fn peek_line(m: &Model, dir: &str, now: Ms) -> String {
    let Some(p) = data(m).peeks.get(dir) else {
        return "looking…".into();
    };
    let mut s = format!("{} item{}", p.items, plural(p.items));
    if p.modified != 0 {
        s += &format!(" · touched {} ago", age(now - p.modified));
    }
    s
}

/// One project whole: where it is, git, its agents and where each works,
/// its worktrees, scratch, last commits and pull requests.
pub(crate) fn project_detail(m: &Model, p: &Project, w: usize) -> Vec<WorkRow> {
    let d = data(m);
    let now = m.now;
    let f = d.folders.get(&p.key);
    let mut rows: Vec<WorkRow> = Vec::new();
    let text = |rows: &mut Vec<WorkRow>, l: Line| {
        rows.push(WorkRow {
            line: l,
            owner: p.key.clone(),
            ..WorkRow::default()
        })
    };
    if p.key.starts_with('/') {
        let mut wh = ln([sp(tildify(&p.key), SUB)]);
        if let Some(f) = f
            && !f.remote.is_empty()
        {
            wh.spans.push(sp("  →  ", DIM));
            wh.spans.push(sp(f.remote.clone(), BLUE));
        }
        text(&mut rows, wh);
    }
    match f {
        _ if !p.repo => text(
            &mut rows,
            ln([
                sp("◇ not a repository", DIM),
                sp(" · ", DIM),
                sp(peek_line(m, &p.key, now), SUB),
            ]),
        ),
        None => text(&mut rows, ln([sp("asking git…", FAINT)])),
        Some(f) => {
            let mut g = git_bits(&f.git);
            if !f.git.branch.is_empty() && !f.git.upstream && f.git.err.is_empty() {
                g.spans.push(sp(" · ", DIM));
                g.spans.push(sp("not pushed anywhere", FAINT));
            }
            text(&mut rows, g);
        }
    }
    super::projectmem::project_memory(m, p, w, &mut rows);
    // Agents, each with where it works.
    text(&mut rows, Line::new());
    let goes = p.agents.iter().filter(|g| can_go(d, g, now)).count();
    let mut meta = p.agents.len().to_string();
    if goes > 0 {
        meta += &format!(
            " · {goes} can go: stopped, untouched {}, nothing left only in them · x closes one",
            age(goes_after(d))
        );
    }
    text(&mut rows, psection("Agents", &meta, w));
    text(
        &mut rows,
        thead(&[(" WHERE", 20), ("   AGENT", 30), ("DOING", 10)]),
    );
    for g in &p.agents {
        let t = g.tree();
        let wh = if t.is_empty() {
            ln([sp("main checkout", DIM)])
        } else {
            ln([sp("⎇ ", BLUE), sp(base(t), SUB)])
        };
        let mut l = cat(ln([plain(" ")]), fit(wh, 19));
        join(&mut l, work_session(m, g, w.saturating_sub(20), now));
        let temp = d.temp_of(&g.a.key);
        if can_go(d, g, now) {
            l = fit(l, w.saturating_sub(24));
            l.spans.push(plain("  "));
            l.spans.push(sp("✓ can go", GREEN));
        } else if temp >= TEMP_SHOWN {
            l = fit(l, w.saturating_sub(24));
            l.spans.push(plain("  "));
            l.spans.push(temp_status(m, g, now));
        }
        rows.push(WorkRow {
            id: format!("a{}", g.a.key),
            what: What::Agent(g.a.key.clone()),
            owner: p.key.clone(),
            line: l,
        });
    }
    // Worktrees, each with its size.
    let trees = project_trees(p, f);
    if p.repo && !trees.is_empty() {
        let mut size = 0;
        let mut wrows = Vec::with_capacity(trees.len());
        for t in &trees {
            let running = p
                .agents
                .iter()
                .filter(|g| g.tree() == t && (g.open() || g.busy()))
                .count();
            let wt = d.worktree_at(t);
            size += wt.size;
            let st = f.and_then(|f| f.trees.get(t));
            let l = match st {
                Some(st) => worktree_line(t, st, &wt, running, w),
                None => {
                    let (nw, ..) = wt_cols(w);
                    cat(
                        fit(
                            ln([kind_mark(Kind::Worktree), plain(" "), sp(base(t), DIM)]),
                            nw,
                        ),
                        ln([sp("asking git…", FAINT)]),
                    )
                }
            };
            wrows.push(WorkRow {
                id: format!("w{t}"),
                what: What::Wt(Box::new(wt)),
                owner: p.key.clone(),
                line: l,
            });
        }
        let mut meta = trees.len().to_string();
        if size > 0 {
            meta += &format!(" · {}", disk(size));
        }
        text(&mut rows, Line::new());
        text(&mut rows, psection("Worktrees", &meta, w));
        text(&mut rows, worktree_head(w));
        rows.extend(wrows);
    }
    // What agents keep as scratch in it: .claude/tmp.
    let mut scratch = Vec::new();
    for dir in agent_tmp_dirs(m, &p.key) {
        if let Some(&n) = d.agent_tmp.get(&dir) {
            let (nw, bw, sw, zw) = wt_cols(w);
            let name = dir
                .strip_prefix(&format!("{}/", p.key))
                .unwrap_or(&dir)
                .to_string();
            let mut l = fit(
                ln([kind_mark(Kind::Temp), plain(" "), sp(name, SUB)]),
                nw + bw,
            );
            join(&mut l, fit(ln([sp(peek_line(m, &dir, now), DIM)]), sw));
            l.spans.push(sp(right(&disk(n), zw), DIM));
            scratch.push(WorkRow {
                id: format!("e{dir}"),
                what: What::Path(PathRow {
                    path: dir.clone(),
                    temp: String::new(),
                    scratch: p.key.clone(),
                }),
                owner: p.key.clone(),
                line: l,
            });
        }
    }
    if !scratch.is_empty() {
        text(&mut rows, Line::new());
        text(
            &mut rows,
            psection(
                "Agents' scratch",
                "files agents kept while they worked here · x empties it",
                w,
            ),
        );
        rows.extend(scratch);
    }
    if let Some(f) = f
        && !f.recent.is_empty()
    {
        text(&mut rows, Line::new());
        text(&mut rows, psection("Recent commits", "", w));
        for c in &f.recent {
            text(
                &mut rows,
                ln([
                    sp("● ", FAINT),
                    sp(format!("{}  ", right(&age(now - c.at), 4)), DIM),
                    sp(c.subject.clone(), TEXT),
                ]),
            );
        }
    }
    // Pull requests: GitHub's that concern you.
    let gh = prs_of(m, &p.key, p.repo, w);
    let shown = gh.iter().filter(|r| matches!(r.what, What::Pr(_))).count();
    if !gh.is_empty() {
        text(&mut rows, Line::new());
        text(&mut rows, psection("Pull requests", &shown.to_string(), w));
        rows.extend(gh);
    }
    rows
}

/// Where the agents keep scratch in the project at root: each one's own
/// folder's tmp (.claude/tmp).
pub(crate) fn agent_tmp_dirs(m: &Model, root: &str) -> Vec<String> {
    if !root.starts_with('/') {
        return Vec::new();
    }
    m.reg
        .project_folders()
        .iter()
        .map(|f| format!("{root}/{f}/tmp"))
        .collect()
}

/// A worktree table's column widths at w: name, branch, state, size.
pub(crate) fn wt_cols(w: usize) -> (usize, usize, usize, usize) {
    let (name, branch, size) = ((w / 5).min(28), (w / 5).min(26), 8);
    let state = 16.max(
        w as isize - name as isize - branch as isize - size as isize - 3 - (w / 6).min(30) as isize,
    ) as usize;
    (name, branch, state, size)
}

pub(crate) fn worktree_head(w: usize) -> Line {
    let (nw, bw, sw, zw) = wt_cols(w);
    let size = right("SIZE", zw);
    thead(&[
        ("  WORKTREE", nw),
        ("BRANCH", bw),
        ("STATE", sw),
        (&size, zw),
        ("   CAME FROM", 12),
    ])
}

/// A worktree in a table row: name, branch, what it has of its own or
/// that it's clean, size, and what it came from.
pub(crate) fn worktree_line(
    t: &str,
    st: &GitState,
    wt: &Worktree,
    running: usize,
    w: usize,
) -> Line {
    let mut work = Vec::new();
    if running > 0 {
        work.push(ln([sp(format!("● {running} running"), ORANGE)]));
    }
    if !st.err.is_empty() {
        work.push(ln([sp(st.err.clone(), RED)]));
    }
    let n = own_commits(st);
    if n > 0 {
        work.push(ln([sp(format!("{n} commit{}", plural(n)), ORANGE)]));
    }
    if st.changed > 0 {
        work.push(ln([sp(format!("{} changed", st.changed), YELLOW)]));
    }
    if !wt.err.is_empty() {
        work.push(ln([sp(wt.err.clone(), RED)]));
    }
    let mut name = sp(base(t), TEXT);
    if work.is_empty() && st.err.is_empty() && wt.err.is_empty() {
        work.push(ln([sp("✓ clean", GREEN)]));
        name = sp(base(t), DIM);
    }
    let mut from = String::new();
    if !st.base.is_empty() {
        from = st.base.clone();
        if st.base_behind > 0 {
            from += &format!(" · {} behind", st.base_behind);
        }
    }
    let size = if wt.size > 0 {
        disk(wt.size)
    } else {
        String::new()
    };
    let mut branch = st.branch.as_str();
    let b = base(t);
    if branch == b || branch == format!("worktree-{b}") {
        branch = ""; // it says nothing the name doesn't
    }
    let (nw, bw, sw, zw) = wt_cols(w);
    let mut l = fit(ln([kind_mark(Kind::Worktree), plain(" "), name]), nw);
    join(&mut l, fit(ln([sp(branch, DIM)]), bw));
    join(&mut l, fit(joined(work, sp(" · ", DIM)), sw));
    l.spans.push(sp(right(&size, zw), DIM));
    l.spans.push(plain("   "));
    l.spans.push(sp(from, FAINT));
    l
}

// ---- the tree, in the rail look ----

/// The Projects list as a tree: each repository on an edge, its main
/// checkout and worktrees hung off it; then the pull requests, Temporary
/// and System.
pub(crate) fn project_tree(m: &Model, w: usize) -> Vec<WorkRow> {
    let list = projects(m);
    let mut rows = Vec::new();
    if list.is_empty() {
        rows.push(WorkRow::text(ln([sp(
            "no agent has worked anywhere in the last day",
            DIM,
        )])));
    }
    for (i, p) in list.iter().enumerate() {
        if i > 0 && p.repo != list[i - 1].repo {
            rows.push(WorkRow::blank());
            rows.push(WorkRow::text(psection(
                "Other folders",
                "◇ not a repository",
                w,
            )));
        }
        rows.extend(project_tree_rows(m, p, w));
    }
    let tail = classic_project_list(m, w);
    if let Some(i) = tail.iter().position(|r| r.id == PANE_PRS) {
        rows.push(WorkRow::blank());
        rows.extend(tail.into_iter().skip(i));
    }
    rows
}

/// One repository's: its heading, its main checkout, then its worktrees.
fn project_tree_rows(m: &Model, p: &Project, w: usize) -> Vec<WorkRow> {
    let d = data(m);
    let edge = sp("┃", if p.open { ORANGE } else { FAINT });
    let f = d.folders.get(&p.key);
    let trees = repo_worktrees(d, p);
    let total: i64 = trees.iter().map(|t| t.size).sum();
    let mut head = ln([edge.clone(), plain(" ")]);
    if !p.repo {
        head.spans.push(kind_mark(Kind::Folder));
        head.spans.push(plain(" "));
    }
    head.spans.push(spb(p.title.clone(), TEXT));
    head.spans.push(plain("  "));
    head.spans.push(sp(tildify(&p.key), DIM));
    match f {
        Some(f) if p.repo => {
            head.spans.push(sp(format!(" · {}", f.git.branch), DIM));
            if f.git.behind > 0 {
                head.spans.push(sp(" · ", DIM));
                head.spans
                    .push(sp(format!("{} behind", f.git.behind), YELLOW));
            }
        }
        None if p.repo => head.spans.push(sp(" · asking git…", FAINT)),
        _ => {}
    }
    let mut rt = Line::new();
    if total > 0 {
        rt.spans.push(sp(disk(total), DIM));
    }
    rt.spans.push(plain(" "));
    let mut rows = vec![WorkRow {
        id: format!("p{}", p.key),
        what: What::Proj(p.key.clone()),
        line: spread(head, rt, w),
        owner: String::new(),
    }];
    if !p.repo {
        return rows;
    }
    let n = trees.len() + 1;
    let branch = |i: usize| if i == n - 1 { "└─" } else { "├─" };
    let git = f.map(|f| f.git.clone()).unwrap_or_default();
    let main = match f {
        None => ln([sp("…", FAINT)]),
        Some(_) if git.changed > 0 => ln([sp(format!("{} changed", git.changed), YELLOW)]),
        Some(_) => ln([sp("clean", FAINT)]),
    };
    let verdict = if f.is_some() && git.behind > 0 {
        ln([sp("needs a pull", YELLOW)])
    } else {
        ln([sp("keep", FAINT)])
    };
    rows.push(WorkRow::text(wt_tree_line(
        &edge,
        branch(0),
        plain(" "),
        sp("main checkout", DIM),
        &git.branch,
        main,
        0,
        verdict,
        w,
    )));
    for (i, t) in trees.into_iter().enumerate() {
        let l = wt_tree_line(
            &edge,
            branch(i + 1),
            tree_mark(m, &t),
            sp(tree_name(&t), TEXT),
            &t.branch,
            tree_what(m, &t),
            t.size,
            tree_verdict(&t),
            w,
        );
        rows.push(WorkRow {
            id: format!("w{}", t.path),
            owner: p.key.clone(),
            line: l,
            what: What::Wt(Box::new(t)),
        });
    }
    rows
}

/// A repository's worktrees as clean-up's scan found them: rush's own
/// and Claude Code's first, a subagent's next, those made by hand last.
pub(crate) fn repo_worktrees(d: &Data, p: &Project) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = d.wts.iter().filter(|t| t.repo == p.key).cloned().collect();
    for path in project_trees(p, d.folders.get(&p.key)) {
        if !out.iter().any(|t| t.path == path) {
            let mut t = d.worktree_at(&path);
            t.repo = p.key.clone();
            out.push(t);
        }
    }
    let rank = |m: &str| match m {
        MADE_BY_RUSH | MADE_BY_CLAUDE | "" => 0,
        MADE_BY_SUBAGENT => 1,
        MADE_BY_OTHER => 2,
        _ => 0,
    };
    out.sort_by(|a, b| {
        rank(a.made)
            .cmp(&rank(b.made))
            .then(tree_name(a).cmp(&tree_name(b)))
    });
    out
}

/// A worktree's name in the tree: its folder's, with where it sits when
/// rush didn't make it.
pub(crate) fn tree_name(t: &Worktree) -> String {
    let name = base(&t.path);
    if t.made == MADE_BY_OTHER {
        let dir = t.path.rsplit_once('/').map_or("", |(d, _)| base(d));
        if !dir.is_empty() && dir != "." {
            return format!("{dir}/{name}");
        }
    }
    name.to_string()
}

/// A worktree's session's state: working, waiting on you, or finished.
fn tree_mark(m: &Model, t: &Worktree) -> Span {
    let now = m.now;
    for &i in &m.order {
        let g = Ag {
            a: &m.agents[i],
            root: m.roots.get(i).map_or("", String::as_str),
        };
        if g.tree() != t.path {
            continue;
        }
        return if g.live() || g.busy() {
            sp("✱", ORANGE)
        } else if g.a.needs_you() || g.waiting() || g.a.halted() {
            sp("●", GREEN)
        } else if g.a.your_turn(now) || g.a.just_finished(now) {
            sp("●", BLUE)
        } else {
            sp("·", FAINT)
        };
    }
    plain(" ")
}

/// The middle of a worktree's line: its pull request, else its session's
/// title, else who made it.
fn tree_what(m: &Model, t: &Worktree) -> Line {
    if let Some(pr) = &t.pr {
        let n = format!("PR #{}", pr.number);
        let mut s = if pr.merged() {
            ln([sp(format!("{n} merged"), DIM)])
        } else if pr.checks == "fail" {
            ln([
                sp(format!("{n} ✗"), RED),
                plain(" "),
                sp(pr.failing.clone(), DIM),
            ])
        } else if pr.checks == "pass" || pr.review == "approved" {
            ln([sp(format!("{n} ✓"), GREEN)])
        } else {
            ln([sp(n, BLUE)])
        };
        s.spans.push(plain(" "));
        s.spans.push(sp(one_line(&pr.title), TEXT));
        return s;
    }
    for &i in &m.order {
        let g = Ag {
            a: &m.agents[i],
            root: m.roots.get(i).map_or("", String::as_str),
        };
        if g.tree() == t.path {
            let mut s = ln([sp(one_line(&g.a.display_name), TEXT)]);
            if t.changed > 0 {
                s.spans
                    .push(sp(format!(" · {} uncommitted", t.changed), DIM));
            }
            return s;
        }
    }
    ln([sp(
        match t.made {
            MADE_BY_SUBAGENT => "a subagent's",
            MADE_BY_OTHER => "not made by rush",
            _ => "no session in it",
        },
        DIM,
    )])
}

/// What clean-up may do with a worktree, in a few words.
pub(crate) fn tree_verdict(t: &Worktree) -> Line {
    let v = t.verdict();
    match v.kind {
        VerdictKind::KeepInUse => ln([sp("in use", DIM)]),
        VerdictKind::KeepPrOpen => ln([sp("PR open", DIM)]),
        VerdictKind::CanGo if !t.own => ln([sp("can go, by hand", BLUE)]),
        VerdictKind::CanGo => ln([sp("can go", BLUE)]),
        VerdictKind::WouldLose => ln([sp(v.reason, YELLOW)]),
    }
}

/// One line under a repository: branch, mark, name, branch name, what it
/// is, size, verdict.
#[allow(clippy::too_many_arguments)]
fn wt_tree_line(
    edge: &Span,
    branch: &str,
    mark: Span,
    name: Span,
    git_branch: &str,
    what: Line,
    size: i64,
    verdict: Line,
    w: usize,
) -> Line {
    let mut left = ln([
        edge.clone(),
        plain(" "),
        sp(branch, FAINT),
        plain(" "),
        mark,
        plain(" "),
    ]);
    join(&mut left, fit(ln([name]), TREE_NAME_W));
    join(&mut left, fit(ln([sp(git_branch, DIM)]), TREE_BRANCH_W));
    let sz = if size > 0 { disk(size) } else { String::new() };
    let mut tail = ln([sp(right(&sz, TREE_SIZE_W), FAINT), plain("  ")]);
    join(&mut tail, fit(verdict, TREE_VERD_W));
    let room = w as isize - left.width() as isize - tail.width() as isize - 1;
    let mut l = cat(left, fit(what, room.max(0) as usize));
    l.spans.push(plain(" "));
    cat(l, tail)
}

// ---- Temporary ----

/// What agents leave behind that's no project's: yours in /tmp, and
/// finished agents' temp work.
pub(crate) fn temp_rows(m: &Model, _w: usize) -> Vec<WorkRow> {
    const NAME_W: usize = 40;
    const SIZE_W: usize = 9;
    let d = data(m);
    let now = m.now;
    let mut rows = vec![
        WorkRow::text(ln([sp(
            "Session scratch and caches, including running and older sessions. Stop a running session before cleaning its files.",
            DIM,
        )])),
        WorkRow::blank(),
        WorkRow::text(thead(&[
            ("  WHAT", NAME_W),
            ("     SIZE", SIZE_W),
            ("   STATUS / LOCATION", 20),
        ])),
    ];
    let row = |name: Line, size: &str, status: Line| {
        let mut l = ln([kind_mark(Kind::Temp), plain(" ")]);
        join(&mut l, fit(name, NAME_W - 2));
        l.spans.push(sp(right(size, SIZE_W), DIM));
        l.spans.push(plain("   "));
        cat(l, status)
    };
    if d.measuring {
        rows.push(WorkRow::text(ln([sp(
            "measuring session storage… sizes appear as each folder finishes",
            DIM,
        )])));
    }
    let n = rows.len();
    let s = d.tmp.clone();
    if s.checked != 0 && s.items > 0 {
        let status = if s.stale_items > 0 {
            ln([
                sp(
                    format!(
                        "✓ {} in {} untouched for a day",
                        disk(s.stale),
                        s.stale_items
                    ),
                    GREEN,
                ),
                sp(" · x clears them", DIM),
            ])
        } else {
            ln([sp("all touched in the last day", DIM)])
        };
        rows.push(WorkRow {
            id: "t".into(),
            what: What::Tmp,
            line: row(
                ln([sp("/tmp", TEXT), sp(format!("  {} of yours", s.items), DIM)]),
                &disk(s.size),
                status,
            ),
            owner: String::new(),
        });
    }
    let mut temp: Vec<Ag> = agents(m)
        .filter(|g| d.temp_of(&g.a.key) >= TEMP_SHOWN)
        .collect();
    temp.sort_by_key(|g| std::cmp::Reverse(d.temp_of(&g.a.key)));
    for g in temp {
        let a = g.a;
        let open = d.opened.contains(&a.key);
        rows.push(WorkRow {
            id: format!("T{}", a.key),
            what: What::Temp(a.key.clone()),
            line: row(
                ln([
                    sp(one_line(&a.display_name), SUB),
                    sp(if open { " ▾" } else { " ▸" }, DIM),
                ]),
                &disk(d.temp_of(&a.key)),
                ln([temp_status(m, &g, now)]),
            ),
            owner: String::new(),
        });
        let mut who = ln([session_ended(&g, now)]);
        if let Some(ad) = m.reg.get(a.kind.as_str()) {
            who = cat(ln([sp(format!("{} · ", ad.name()), DIM)]), who);
        }
        for dir in d
            .temp
            .get(&a.key)
            .map(|t| t.dirs.as_slice())
            .unwrap_or_default()
        {
            let mut l = fit(
                ln([sp(format!("    {}", tildify(&dir.path)), DIM)]),
                NAME_W + SIZE_W + 1,
            );
            l.spans.push(plain("   "));
            rows.push(WorkRow::text(cat(l, who.clone())));
        }
        if open {
            rows.extend(temp_entry_rows(m, &a.key, NAME_W, SIZE_W, now));
        }
    }
    if rows.len() == n && !d.measuring {
        return vec![WorkRow::text(ln([sp(
            "no measured scratch in /tmp or session temp folders",
            DIM,
        )]))];
    }
    rows
}

/// When an agent's temp work goes.
pub(crate) fn temp_status(m: &Model, g: &Ag, now: Ms) -> Span {
    if g.a.pid != 0 {
        return sp("running · stop before cleaning", YELLOW);
    }
    match due_in(m, data(m), std::slice::from_ref(&g.a.key), now) {
        Some(0) => sp("at the next tidy-up", GREEN),
        Some(due) => sp(format!("in {}", dur(due / 60_000 * 60_000)), DIM),
        None => sp("x removes it", DIM),
    }
}

/// When a session stopped, or that it runs.
pub(crate) fn session_ended(g: &Ag, now: Ms) -> Span {
    if g.a.pid != 0 {
        return sp("running", YELLOW);
    }
    sp(format!("ended {} ago", age(g.a.age(now))), DIM)
}

/// How many of a session's things show, opened.
const TEMP_SHOWN_ENTRIES: usize = 40;

/// A session's opened temp folder, a row per thing, biggest first once
/// measured, else newest.
fn temp_entry_rows(m: &Model, key: &str, name_w: usize, size_w: usize, now: Ms) -> Vec<WorkRow> {
    let d = data(m);
    let Some((ents, _)) = d.lists.get(key) else {
        return vec![WorkRow::text(ln([sp("    listing…", DIM)]))];
    };
    if ents.is_empty() {
        return vec![WorkRow::text(ln([sp("    empty", DIM)]))];
    }
    let mut ents = ents.clone();
    ents.sort_by(|a, b| {
        let (x, y) = (
            d.sizes.get(&a.path).copied().unwrap_or(0),
            d.sizes.get(&b.path).copied().unwrap_or(0),
        );
        y.cmp(&x).then(b.modified.cmp(&a.modified))
    });
    let mut rows = Vec::new();
    for e in ents.iter().take(TEMP_SHOWN_ENTRIES) {
        let mut name = base(&e.path).to_string();
        if e.dir {
            name.push('/');
        }
        let size = d.sizes.get(&e.path).map_or("…".to_string(), |&n| disk(n));
        let mut l = ln([plain("    ")]);
        join(&mut l, fit(ln([sp(name, SUB)]), name_w - 4));
        l.spans.push(sp(right(&size, size_w), DIM));
        l.spans.push(plain("   "));
        l.spans
            .push(sp(format!("touched {} ago", age(now - e.modified)), DIM));
        rows.push(WorkRow {
            id: format!("e{}", e.path),
            what: What::Path(PathRow {
                path: e.path.clone(),
                temp: key.to_string(),
                scratch: String::new(),
            }),
            line: l,
            owner: String::new(),
        });
    }
    if ents.len() > TEMP_SHOWN_ENTRIES {
        rows.push(WorkRow::text(ln([sp(
            format!(
                "    … {} more, the newest and biggest above",
                ents.len() - TEMP_SHOWN_ENTRIES
            ),
            DIM,
        )])));
    }
    rows
}

// ---- System ----

/// The processes no project owns: orphans, with the few holding most of
/// each one's memory, then Claude processes that belong to no agent.
pub(crate) fn proc_rows(m: &Model) -> Vec<ProcRow> {
    let d = data(m);
    let Some(tab) = &d.prev_table else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in d.machine.rows.iter().filter(|r| r.role == Role::Orphan) {
        out.push(ProcRow {
            pid: r.pid,
            depth: 0,
            n: r.procs,
            label: r.label.clone(),
            cmd: r.cmd.clone(),
            mem: r.mem,
            cpu: r.cpu,
            start: proc_start(Some(tab), r.pid, r.start),
            role: r.role,
            other: true,
        });
        let mut kids: Vec<_> = tab.tree(r.pid).into_iter().skip(1).collect();
        kids.sort_by_key(|k| std::cmp::Reverse(k.footprint));
        for k in kids.iter().take(3) {
            if k.footprint >= 32 << 20 {
                out.push(ProcRow {
                    pid: k.pid,
                    depth: 1,
                    n: 1,
                    label: String::new(),
                    cmd: d
                        .kid_words
                        .get(&k.pid)
                        .cloned()
                        .unwrap_or_else(|| k.comm.to_string()),
                    mem: k.footprint,
                    cpu: k.cpu,
                    start: k.start,
                    role: Role::Orphan,
                    other: false,
                });
            }
        }
    }
    for r in &d.machine.rows {
        if r.role == Role::Worker || r.role == Role::Orphan {
            continue;
        }
        out.push(ProcRow {
            pid: r.pid,
            depth: 0,
            n: r.procs,
            label: r.label.clone(),
            cmd: r.cmd.clone(),
            mem: r.mem,
            cpu: r.cpu,
            start: proc_start(Some(tab), r.pid, r.start),
            role: r.role,
            other: true,
        });
    }
    out
}

/// A role's heading in System.
pub(crate) fn role_name(r: Role) -> &'static str {
    match r {
        Role::Daemon => "Daemon",
        Role::View => "Agent views",
        Role::Worker => "Agents",
        Role::Spare => "Spares — pre-warmed, ready for the next session",
        Role::Orphan => "Orphaned — the session that started these has ended",
        Role::Other => "Other Claude Code",
    }
}

fn ms_since(t: std::time::SystemTime, now: Ms) -> Ms {
    now - ms_of(t)
}

pub(crate) fn system_rows(m: &Model, w: usize) -> Vec<WorkRow> {
    const LABEL_W: usize = 28;
    const CPU_W: usize = 9;
    const MEM_W: usize = 8;
    const PROC_W: usize = 7;
    let d = data(m);
    let now = m.now;
    let procs = proc_rows(m);
    if procs.is_empty() {
        return vec![WorkRow::text(ln([sp(
            "no process here belongs to no one",
            DIM,
        )]))];
    }
    let cmd_w = 20.max(w.saturating_sub(LABEL_W + CPU_W + MEM_W + PROC_W + 2));
    let cols = |r: &ProcRow| {
        let p = if r.n > 1 {
            r.n.to_string()
        } else {
            String::new()
        };
        ln([
            cpu_color(r.cpu, right(&format!("{:.1}%", r.cpu), CPU_W)),
            mem_color(r.mem, right(&mem(r.mem), MEM_W)),
            sp(right(&p, PROC_W), DIM),
        ])
    };
    let mut rows = Vec::new();
    let mc = &d.machine;
    if mc.orphans > 0 {
        let fate = if d.cfg.keep_orphans {
            " · their sessions ended; they run until you end them · "
        } else {
            " · their sessions ended; each is ended two minutes after · "
        };
        rows.push(WorkRow::text(ln([
            spb(format!("{} orphaned", mc.orphans), YELLOW),
            sp(format!(" · holding {}", mem(mc.orphan_mem)), YELLOW),
            sp(fate, DIM),
            sp("x", ORANGE),
            sp(" ends one, ", DIM),
            sp("X", ORANGE),
            sp(" all", DIM),
        ])));
        rows.push(WorkRow::blank());
    }
    let (cpu, memh, proch) = (
        right("CPU", CPU_W),
        right("MEM", MEM_W),
        right("PROCS", PROC_W),
    );
    rows.push(WorkRow::text(thead(&[
        ("  PROCESS", LABEL_W),
        ("COMMAND", cmd_w),
        (&cpu, CPU_W),
        (&memh, MEM_W),
        (&proch, PROC_W),
    ])));
    let mut last: Option<Role> = None;
    for r in procs {
        let l = if r.other && r.role == Role::Orphan {
            let lbl = fit_str(
                &format!("{} · {} old", r.label, age(ms_since(r.start, now))),
                LABEL_W - 2,
            );
            cat(
                ln([
                    sp(format!("! {lbl}"), YELLOW),
                    sp(fit_str(&trim_cmd(&r.cmd, cmd_w - 2), cmd_w), TEXT),
                ]),
                cols(&r),
            )
        } else if r.other {
            if last != Some(r.role) {
                last = Some(r.role);
                rows.push(WorkRow::text(psection(role_name(r.role), "", w)));
            }
            cat(
                ln([
                    plain("  "),
                    sp(fit_str(&r.label, LABEL_W - 2), SUB),
                    sp(fit_str(&trim_cmd(&r.cmd, cmd_w - 2), cmd_w), FAINT),
                ]),
                cols(&r),
            )
        } else {
            rows.push(WorkRow::text(cat(
                ln([
                    plain(blanks(LABEL_W)),
                    sp(
                        fit_str(&format!("└ {}", trim_cmd(&r.cmd, cmd_w - 4)), cmd_w),
                        FAINT,
                    ),
                ]),
                cols(&r),
            )));
            continue;
        };
        rows.push(WorkRow {
            id: format!("s{}", r.pid),
            line: l,
            what: What::Proc(Box::new(r)),
            owner: String::new(),
        });
    }
    rows
}

// ---- Pull requests ----

/// The colour of a pull request's state.
fn pr_state_color(p: &PR) -> Color {
    match p.state.as_str() {
        "MERGED" => BLUE,
        "CLOSED" => DIM,
        _ if p.draft => DIM,
        _ => GREEN,
    }
}

/// A project's pull requests as rows, or a line saying why there are none.
pub(crate) fn prs_of(m: &Model, root: &str, repo: bool, w: usize) -> Vec<WorkRow> {
    let d = data(m);
    let t = |l: Line| WorkRow {
        line: l,
        owner: root.to_string(),
        ..WorkRow::default()
    };
    let Some(r) = d.prs.get(root) else {
        if repo && d.asking.contains(root) {
            return vec![t(ln([spin(m), sp(" asking GitHub…", FAINT)]))];
        }
        return Vec::new();
    };
    if !r.err.is_empty() && r.prs.is_empty() {
        return vec![t(ln([sp(format!("⇡ {}", r.err), DIM)]))];
    }
    let mut rows: Vec<WorkRow> = r
        .prs
        .iter()
        .map(|pr| WorkRow {
            id: format!("r{root}#{}", pr.number),
            line: gh_pr_line(pr, w),
            what: What::Pr(Box::new(PrRow {
                root: root.to_string(),
                pr: pr.clone(),
            })),
            owner: root.to_string(),
        })
        .collect();
    if !r.err.is_empty() {
        rows.push(t(ln([sp(
            format!("as GitHub last said · asking again failed: {}", r.err),
            FAINT,
        )])));
    }
    rows
}

/// How many pull requests every project has open that concern you, and
/// how many want something of you.
pub(crate) fn pr_counts(m: &Model) -> (usize, usize) {
    let (mut open, mut want) = (0, 0);
    for r in data(m).prs.values() {
        for p in &r.prs {
            open += 1;
            if pr_wants_you(p) {
                want += 1;
            }
        }
    }
    (open, want)
}

fn pr_wants_you(p: &PR) -> bool {
    if !p.mine {
        return p.asked_you;
    }
    let (_, failed, _) = p.counts();
    failed > 0 || p.decision == "CHANGES_REQUESTED"
}

/// The list's Pull requests row.
pub(crate) fn prs_list_line(m: &Model) -> Line {
    let (open, want) = pr_counts(m);
    let mut s = ln([sp(format!("{open} open"), DIM)]);
    if want > 0 {
        s.spans.push(sp(" · ", DIM));
        s.spans.push(sp(format!("{want} want you"), YELLOW));
    }
    if data(m).prs.is_empty() {
        s = ln([sp("asking GitHub…", FAINT)]);
    }
    let mut l = ln([sp("⇡", BLUE), plain(" ")]);
    join(
        &mut l,
        fit(ln([spb("Pull requests", TEXT)]), PROJ_NAME_W - 2),
    );
    cat(l, s)
}

/// Every project's pull requests, project by project.
pub(crate) fn prs_rows(m: &Model, w: usize) -> Vec<WorkRow> {
    let d = data(m);
    let mut rows = Vec::new();
    for p in projects(m).iter().filter(|p| p.repo) {
        let prs = prs_of(m, &p.key, true, w);
        if prs.is_empty() {
            continue;
        }
        let meta = d
            .prs
            .get(&p.key)
            .filter(|r| !r.prs.is_empty())
            .map_or(String::new(), |r| r.prs.len().to_string());
        if !rows.is_empty() {
            rows.push(WorkRow::blank());
        }
        rows.push(WorkRow::text(psection(&p.title, &meta, w)));
        rows.extend(prs);
    }
    if rows.is_empty() {
        return vec![WorkRow::text(ln([sp(
            "no open pull request of yours or asking for your review",
            DIM,
        )]))];
    }
    rows
}

/// A pull request in a line: number, checks, where review is, title.
pub(crate) fn gh_pr_line(pr: &PR, w: usize) -> Line {
    let (ok, failed, pending) = pr.counts();
    let mut s = ln([sp(
        fit_str(&format!("⇡ #{}", pr.number), 8),
        pr_state_color(pr),
    )]);
    let mut checks = Line::new();
    let n = ok + failed + pending;
    if n > 0 {
        let c = if failed > 0 {
            RED
        } else if pending > 0 {
            SUB
        } else {
            GREEN
        };
        checks.spans.push(sp(format!("{ok}/{n}"), c));
        if failed > 0 {
            checks.spans.push(sp(format!(" ✗{failed}"), RED));
        }
        if pending > 0 {
            checks.spans.push(sp(format!(" …{pending}"), DIM));
        }
    }
    join(&mut s, fit(checks, 12));
    join(&mut s, fit(pr_review_word(pr), 26));
    let mut title = Line::new();
    if pr.draft {
        title.spans.push(sp("draft ", DIM));
    }
    title.spans.push(sp(pr.title.clone(), TEXT));
    if !pr.mine {
        title.spans.push(sp(format!(" · @{}", pr.author), DIM));
    }
    let room = 10.max(w.saturating_sub(s.width()));
    cat(s, fit(title, room))
}

/// Where a pull request's review is, in a few words.
pub(crate) fn pr_review_word(pr: &PR) -> Line {
    if pr.state == "MERGED" {
        ln([sp("merged", BLUE)])
    } else if pr.state == "CLOSED" {
        ln([sp("closed", DIM)])
    } else if pr.asked_you && !pr.mine {
        ln([spb("your review", ORANGE)])
    } else if pr.decision == "APPROVED" {
        ln([sp("✓ approved", GREEN)])
    } else if pr.decision == "CHANGES_REQUESTED" {
        ln([sp("changes asked", RED)])
    } else if !pr.waiting.is_empty() {
        ln([sp("waiting on ", YELLOW), sp(pr.waiting.join(", "), SUB)])
    } else if pr.decision == "REVIEW_REQUIRED" {
        ln([sp("needs a review", YELLOW)])
    } else {
        Line::new()
    }
}

/// The cells a string takes.
#[allow(dead_code)]
pub(crate) fn cells(s: &str) -> usize {
    width(s)
}
