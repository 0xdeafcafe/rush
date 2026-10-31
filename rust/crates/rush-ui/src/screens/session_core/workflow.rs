//! A workflow runs agents in phases. Claude Code says how each stands as
//! it goes; for one rush didn't hear run, its files on disk say (read by
//! subs.rs off the UI thread). Its row shows the phases; its agents, each
//! a run of its own. After Go's internal/ui/workflow.go (workflowOf :91,
//! phasesOf :105, workflowJob :127, workflowAgent :140, workflowGroup
//! :163, workflowGroups :184, jobLabelPlain :227, phaseMark :232,
//! agentMark :259, workflowStrip :276, workflowAgentRow :344,
//! workflowRows :294, workflowNow :385, dockFlow :502, dockFlows :515, flowRow :542,
//! isWorkflowRef :573, openWorkflow :586).

use std::collections::HashMap;
use std::path::Path;

use rush_convo::{Job, Session, Subagent};
use rush_core::event::WorkflowAgent;
use rush_core::text::{Line, Span};
use rush_core::time::Ms;

use super::dock::{picked1, spread};
use super::overview::pretty_model;
use super::paint::{dim, faint, tokens};
use super::subs::Runs;
use crate::style::*;

/// One phase of a workflow and the agents it ran (workflowPhase).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Phase {
    pub title: String,
    pub agents: Vec<WorkflowAgent>,
}

impl Phase {
    /// Its agents, an agent tried again once: of the tries with one label,
    /// the one running, else the last listed, where the first stood. A
    /// workflow resumed after a restart runs its agents again.
    pub fn current(&self) -> Vec<&WorkflowAgent> {
        let mut out: Vec<&WorkflowAgent> = Vec::new();
        for a in &self.agents {
            let same = out
                .iter()
                .position(|c| !a.label.is_empty() && c.label == a.label);
            match same {
                Some(i) if !(out[i].state == "running" && a.state != "running") => out[i] = a,
                Some(_) => {}
                None => out.push(a),
            }
        }
        out
    }

    /// The tries before a, of its label.
    pub fn earlier(&self, a: &WorkflowAgent) -> Vec<&WorkflowAgent> {
        self.agents
            .iter()
            .filter(|b| !a.label.is_empty() && b.label == a.label && b.id != a.id)
            .collect()
    }
}

/// One workflow run as the dock and the subagents view show it: its
/// phases, and its agents' runs (workflowGroup).
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub run: String,
    pub title: String,
    /// Its task's id; "" for one rush didn't hear start.
    pub job: String,
    pub background: bool,
    pub status: String,
    pub tokens: i64,
    pub tool_uses: i64,
    pub phases: Vec<Phase>,
    /// Its agents' runs, by agent id.
    pub subs: HashMap<String, Subagent>,
    pub live: bool,
    pub start: Ms,
    pub end: Ms,
}

impl Group {
    /// What picks the group's own row (ref).
    pub fn r#ref(&self) -> String {
        if self.job.is_empty() {
            format!("wf:{}", self.run)
        } else {
            format!("job:{}", self.job)
        }
    }
}

/// A workflow working in the background, as the dock shows it: its row,
/// then its agents still working (dockFlow).
#[derive(Clone, Debug, Default)]
pub struct DockFlow {
    pub g: Group,
    pub agents: Vec<Subagent>,
}

/// Each agent under its phase, in the phases' order; ended, one not heard
/// to finish stopped with its workflow (phasesOf).
pub fn phases_of(phases: &[String], agents: &[WorkflowAgent], ended: bool) -> Vec<Phase> {
    let mut out: Vec<Phase> = phases
        .iter()
        .map(|p| Phase {
            title: p.clone(),
            agents: Vec::new(),
        })
        .collect();
    for a in agents {
        let mut a = a.clone();
        if ended && (a.state == "running" || a.state == "queued") {
            a.state = "stopped".into();
        }
        let i = match out.iter().position(|p| p.title == a.phase) {
            Some(i) => i,
            None => {
                out.push(Phase {
                    title: a.phase.clone(),
                    agents: Vec::new(),
                });
                out.len() - 1
            }
        };
        out[i].agents.push(a);
    }
    out
}

/// How workflow task j stands: its phases, each with its agents, as
/// Claude Code last said, else as its files do (workflowOf).
pub fn workflow_of(r: &Runs, j: &Job) -> Vec<Phase> {
    let f = r.flow_files(r.flow_dir(j));
    let mut phases = &j.phases;
    let mut agents = &j.agents;
    if let Some(f) = f {
        if f.phases.len() > phases.len() {
            phases = &f.phases;
        }
        if agents.is_empty() {
            agents = &f.agents;
        }
    }
    phases_of(phases, agents, !j.running())
}

/// The folder name of a run's directory.
fn base(d: &str) -> &str {
    Path::new(d)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// The task of the workflow run whose folder's name is run, or None when
/// rush didn't hear it start (workflowJob).
pub fn workflow_job<'a>(r: &Runs, s: &'a Session, run: &str) -> Option<&'a Job> {
    s.jobs()
        .iter()
        .find(|j| s.job_kind(j) == "workflow" && base(r.flow_dir(j)) == run)
}

/// The word of a workflow's agent sa: from its workflow's task, and
/// whether that still runs; else from its files, once they say it
/// finished. None when neither knows (workflowAgent).
pub fn agent_of(r: &Runs, s: &Session, sa: &Subagent) -> Option<(WorkflowAgent, bool)> {
    if let Some(j) = workflow_job(r, s, &sa.workflow) {
        for p in workflow_of(r, j) {
            if let Some(a) = p.agents.into_iter().find(|a| a.id == sa.id) {
                return Some((a, j.running()));
            }
        }
        // Not in what Claude Code says now: a try it made before.
    }
    let dir = Path::new(&sa.path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    r.flow_files(&dir)?
        .agents
        .iter()
        .find(|a| a.id == sa.id && (a.state == "done" || a.state == "failed"))
        .map(|a| (a.clone(), false))
}

/// A task's label in a line, without colour (jobLabelPlain).
pub fn job_label_plain(j: &Job) -> String {
    let l = if !j.label.is_empty() {
        &j.label
    } else if !j.summary.is_empty() {
        &j.summary
    } else {
        "workflow"
    };
    one_line(l)
}

/// The session's workflow runs, newest first: those rush heard start, and
/// those whose agents' runs are on disk (workflowGroups).
pub fn groups(r: &Runs, s: &Session, live_of: &dyn Fn(&Subagent) -> bool) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for j in s.jobs() {
        if s.job_kind(j) != "workflow" {
            continue;
        }
        let dir = r.flow_dir(j);
        let run = if dir.is_empty() {
            format!("task:{}", j.id)
        } else {
            base(dir).to_string()
        };
        at.insert(run.clone(), out.len());
        out.push(Group {
            run,
            title: job_label_plain(j),
            job: j.id.clone(),
            background: j.background,
            status: j.status.clone(),
            tokens: j.tokens,
            tool_uses: j.tool_uses,
            phases: workflow_of(r, j),
            subs: HashMap::new(),
            live: j.running(),
            start: j.start,
            end: j.end,
        });
    }
    for sa in &r.subs {
        if sa.workflow.is_empty() {
            continue;
        }
        let i = match at.get(&sa.workflow) {
            Some(&i) => i,
            None => {
                let dir = Path::new(&sa.path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let f = r.flow_files(&dir).cloned().unwrap_or_default();
                at.insert(sa.workflow.clone(), out.len());
                out.push(Group {
                    run: sa.workflow.clone(),
                    title: if f.title.is_empty() {
                        sa.workflow.clone()
                    } else {
                        f.title.clone()
                    },
                    phases: phases_of(&f.phases, &f.agents, f.done),
                    start: sa.born,
                    ..Group::default()
                });
                out.len() - 1
            }
        };
        let g = &mut out[i];
        g.subs.insert(sa.id.clone(), sa.clone());
        if g.job.is_empty() {
            if live_of(sa) {
                g.live = true;
            }
            if sa.modified > g.end {
                g.end = sa.modified;
            }
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.start));
    out
}

/// The workflows working beside the turn, newest first, each with its
/// agents still working. One the turn waits on shows under its step in
/// the conversation instead (dockFlows).
pub fn dock_flows(
    r: &Runs,
    s: &Session,
    running: &[Subagent],
    live_of: &dyn Fn(&Subagent) -> bool,
) -> Vec<DockFlow> {
    groups(r, s, live_of)
        .into_iter()
        .filter(|g| g.live && (g.job.is_empty() || g.background))
        .map(|g| {
            let agents = running
                .iter()
                .filter(|sa| sa.workflow == g.run)
                .cloned()
                .collect();
            DockFlow { g, agents }
        })
        .collect()
}

/// The spinner's frame n, orange.
pub fn spin(n: usize) -> Span {
    sp(SPINNER[n % SPINNER.len()], ORANGE)
}

/// How a phase stands: not started, running, failed or done (phaseMark).
pub fn phase_mark(p: &Phase, tick: usize) -> Span {
    let agents = p.current();
    let run = agents
        .iter()
        .filter(|a| a.state == "running" || a.state == "queued")
        .count();
    let done = agents.iter().filter(|a| a.state == "done").count();
    let failed = agents.iter().filter(|a| a.state == "failed").count();
    if agents.is_empty() {
        sp("·", FAINT)
    } else if run > 0 {
        spin(tick)
    } else if failed > 0 {
        sp("✗", RED)
    } else if done == agents.len() {
        sp("✓", GREEN)
    } else {
        dim("⏹")
    }
}

/// How a workflow's agent stands (agentMark).
pub fn agent_mark(a: &WorkflowAgent, tick: usize) -> Span {
    match a.state.as_str() {
        "running" => spin(tick),
        "queued" => sp("◌", FAINT),
        "done" => sp("✓", GREEN),
        "failed" => sp("✗", RED),
        _ => dim("⏹"),
    }
}

fn title_of(p: &Phase) -> &str {
    if p.title.is_empty() {
        "agents"
    } else {
        &p.title
    }
}

/// A workflow's phases on a line: each one's mark and title, and how many
/// of its agents are done while it runs (workflowStrip).
pub fn workflow_strip(phases: &[Phase], tick: usize) -> Line {
    let mut out = Line::new();
    for (i, p) in phases.iter().enumerate() {
        if i > 0 {
            out.spans.push(sp("  ›  ", FAINT));
        }
        let agents = p.current();
        let done = agents.iter().filter(|a| a.state == "done").count();
        out.spans.push(phase_mark(p, tick));
        out.spans.push(plain(" "));
        if agents.is_empty() {
            out.spans.push(sp(title_of(p), FAINT));
        } else if done < agents.len() {
            out.spans.push(spb(title_of(p), TEXT));
            out.spans.push(dim(format!(" {done}/{}", agents.len())));
        } else {
            out.spans.push(sp(title_of(p), SUB));
        }
    }
    out
}

/// A running workflow's dock line: its phase now, and the agent last
/// heard from (workflowNow). Empty when every phase is done.
pub fn workflow_now(phases: &[Phase], tick: usize) -> Line {
    for p in phases {
        let agents = p.current();
        let done = agents.iter().filter(|a| a.state == "done").count();
        let running = agents
            .iter()
            .filter(|a| a.state == "running")
            .map(|a| a.label.as_str())
            .next_back()
            .unwrap_or("");
        if agents.is_empty() || done == agents.len() {
            continue;
        }
        let mut l = line([
            phase_mark(p, tick),
            plain(" "),
            sp(title_of(p), TEXT),
            dim(format!(" {done}/{}", agents.len())),
        ]);
        if !running.is_empty() {
            l.spans.push(dim(format!(" · {running}")));
        }
        return l;
    }
    Line::new()
}

/// One agent's row: its mark and label, its model, how long it ran and
/// what it used, then what it's doing or what it gave back
/// (workflowAgentRow).
pub fn agent_row(a: &WorkflowAgent, label_w: usize, tick: usize, w: usize, now: Ms) -> Line {
    let mut took = a.took.as_millis() as Ms;
    if took == 0 && a.start > 0 && a.state == "running" {
        took = now - a.start;
    }
    let mut facts = Vec::new();
    if !a.model.is_empty() {
        facts.push(pretty_model(&a.model));
    }
    if took > 0 {
        facts.push(dur(round_s(took)));
    }
    if a.tokens > 0 {
        facts.push(tokens(a.tokens));
    }
    let what: Vec<Span> = match a.state.as_str() {
        "running" if !a.last_tool.is_empty() => vec![
            sp("›", ORANGE),
            plain(" "),
            sp(a.last_tool.clone(), TEXT),
            plain(" "),
            dim(one_line(&a.doing)),
        ],
        "queued" => vec![dim("waiting to start")],
        "failed" => vec![sp(one_line(&a.error), RED)],
        "done" => vec![dim(one_line(&a.result))],
        "stopped" => vec![dim("stopped with its workflow")],
        _ => Vec::new(),
    };
    let label = rush_draw::truncate(&a.label, label_w, "…").into_owned();
    let pad = label_w.saturating_sub(rush_core::text::width(&label));
    let mut row = line([
        plain("        "),
        agent_mark(a, tick),
        plain(" "),
        sp(label, TEXT),
        plain(blanks(pad)),
    ]);
    if !facts.is_empty() {
        row.spans.push(plain("  "));
        row.spans.push(dim(facts.join(" · ")));
    }
    if !what.is_empty() {
        row.spans.push(plain("  "));
        row.spans.extend(what);
    }
    rush_draw::truncate_line(&row, w.saturating_sub(2), "…")
}

/// Workflow phases' rows under their task's row in the background view:
/// its phases on a line; opened, each phase and the agents it ran, each
/// agent's row its own to pick and open. refs are each row's: an agent's
/// "run:<id>" when it has one, else the task's ref (workflowRows).
pub fn workflow_rows(
    phases: &[Phase],
    r#ref: &str,
    open: bool,
    w: usize,
    tick: usize,
    now: Ms,
) -> (Vec<Line>, Vec<String>) {
    if phases.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let room = w.saturating_sub(2);
    if !open {
        let mut l = line([plain("      ")]);
        join(&mut l, workflow_strip(phases, tick));
        return (
            vec![rush_draw::truncate_line(&l, room, "…")],
            vec![r#ref.to_string()],
        );
    }
    let label_w = phases
        .iter()
        .flat_map(|p| &p.agents)
        .map(|a| rush_core::text::width(&a.label).min(28))
        .fold(8, usize::max);
    let (mut rows, mut refs) = (Vec::new(), Vec::new());
    for (pi, p) in phases.iter().enumerate() {
        let mut head = line([
            plain("      "),
            phase_mark(p, tick + pi),
            plain(" "),
            spb(title_of(p), TEXT),
        ]);
        let n = p.agents.len();
        head.spans.push(if n > 0 {
            dim(format!("  {n} agent{}", if n == 1 { "" } else { "s" }))
        } else {
            dim("  not started")
        });
        rows.push(rush_draw::truncate_line(&head, room, "…"));
        refs.push(r#ref.to_string());
        for (ai, a) in p.agents.iter().enumerate() {
            rows.push(agent_row(a, label_w, tick + ai, w, now));
            refs.push(if a.id.is_empty() {
                r#ref.to_string()
            } else {
                format!("run:{}", a.id)
            });
        }
    }
    (rows, refs)
}

/// d rounded to the second, as Go's Round(time.Second).
pub fn round_s(d: Ms) -> Ms {
    (d + 500) / 1000 * 1000
}

/// A workflow's row in the dock: what it's for, the phase it's in and
/// its agent last heard from, how many of its agents are done (flowRow).
pub fn flow_row(
    g: &Group,
    i: usize,
    w: usize,
    tick: usize,
    now: Ms,
    sel: &str,
    focus: bool,
) -> Line {
    let (mut done, mut all) = (0, 0);
    for p in &g.phases {
        for a in p.current() {
            all += 1;
            if a.state == "done" {
                done += 1;
            }
        }
    }
    let mut facts = Vec::new();
    if all > 0 {
        facts.push(format!("{done}/{all} agents done"));
    }
    if g.start > 0 {
        facts.push(dur(round_s(now - g.start)));
    }
    let right = line([dim(facts.join(" · ")), plain("  ")]);
    let title = rush_draw::truncate(&g.title, (w / 3).clamp(16, 40), "…").into_owned();
    let mut left = line([
        plain("  "),
        spin(tick + i),
        plain(" "),
        sp("⋔", QUEUE),
        plain(" "),
        spb(title, TEXT),
    ]);
    let at = workflow_now(&g.phases, tick + i);
    if !at.spans.is_empty() {
        left.spans.push(plain("  "));
        left.spans.push(sp("›", ORANGE));
        left.spans.push(plain(" "));
        join(&mut left, at);
    }
    let keep = (w as isize - right.width() as isize - 2).max(20) as usize;
    let row = spread(rush_draw::truncate_line(&left, keep, "…"), right, w);
    if sel == g.r#ref() {
        return picked1(row, w, focus);
    }
    row
}

/// Whether ref picks a workflow: its task, or a run of one rush didn't
/// hear start (isWorkflowRef).
pub fn is_workflow_ref(s: &Session, r: &str) -> bool {
    if r.starts_with("wf:") {
        return true;
    }
    r.strip_prefix("job:")
        .and_then(|id| s.job(id))
        .is_some_and(|j| s.job_kind(j) == "workflow")
}

/// A faint "·" between facts.
pub fn sep() -> Span {
    faint(" · ")
}
