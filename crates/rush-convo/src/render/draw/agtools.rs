//! rush's own agent tools in the conversation, drawn by what they do
//! whichever harness called them: agent_result as waiting on the agent it
//! asks after, with what that agent is doing now under it; agent_send as
//! a message to it; start_agent as the session it started. A spawn_agent
//! call is drawn as any subagent is (spawn.rs). Keyed on the tool's name
//! and its input alone, as every harness's call is put in the same words.

use super::*;
use crate::model::runs::{
    AGENT_RESULT, AGENT_SEND, RETIRE, START_AGENT, agent_id, destination, input_str, rush_tool,
    rush_words,
};
use crate::render::spawn::Child;

/// An agent's name, or "an agent" when nothing names it.
fn or_an_agent(n: String) -> String {
    if n.is_empty() { "an agent".into() } else { n }
}

/// What came after the agent's name in an agent_result's answer, to its
/// first full stop: "is still working (reading view.go)".
fn standing(out: &str, id: &str) -> String {
    let line = crate::text::one_line(out);
    let rest = line
        .split_once(&format!("({id}) "))
        .map_or(line, |(_, r)| r);
    let rest = rest.split_once(". ").map_or(rest, |(a, _)| a);
    rest.trim_end_matches('.').to_string()
}

impl<'a> Drawer<'a> {
    /// The agent rush started as id: its name as the session's call named
    /// it, and its own session when it's found under the step that
    /// started it.
    pub(crate) fn agent_by_id(&self, id: &str) -> (String, Option<&'a Child>) {
        let o: &'a Options = self.o;
        let found = o
            .kids
            .iter()
            .find_map(|(step, cs)| cs.iter().find(|c| c.id == id).map(|c| (step, c)));
        let mut name = self.s.agent_called(id);
        if name.is_empty()
            && let Some((step, _)) = found
            && let Some(at) = self.s.step_by_call(step)
        {
            name = input_str(self.s.step(at), "agent");
        }
        if name.is_empty()
            && let Some((_, c)) = found
        {
            name.clone_from(&c.spawn.name);
        }
        (name, found.map(|(_, c)| c))
    }

    /// The agent a call of rush's agent tools is about, once found: the
    /// one it names by id, else the one it started.
    fn agent_of(&self, st: &Step) -> Option<&'a Child> {
        match agent_id(st) {
            Some(id) => self.agent_by_id(&id).1,
            None => self.child(st),
        }
    }

    /// The row of an agent_result, agent_send or start_agent call: what it
    /// does, to which agent. None for any other step; a spawn_agent call
    /// is drawn as a subagent.
    pub(crate) fn agent_tool_label(&self, st: &Step, lbl: &dyn Fn(&str) -> Txt) -> Option<Txt> {
        let t = rush_tool(&st.tool)?;
        let g = |s: &str| glyph_color(s).and_raw(" ");
        let running = st.status == Status::Running;
        let called = || {
            or_an_agent(
                agent_id(st)
                    .map(|id| self.agent_by_id(&id).0)
                    .unwrap_or_default(),
            )
        };
        Some(match t {
            START_AGENT => {
                let n = or_an_agent(input_str(st, "agent"));
                let did = if running { "starting" } else { "started" };
                g("⧉")
                    .and(lbl(&format!("{did} {n} as its own session")))
                    .and_raw("  ")
                    .and(faint(first_line(&input_str(st, "prompt"))))
            }
            // Emeritus: the baton handed over, by where the task is
            // going; opened, the baton itself (retire_body).
            RETIRE => {
                let did = if running || st.status == Status::Ok {
                    "handing over to a fresh agent"
                } else {
                    "couldn't hand over"
                };
                g("↻")
                    .and(lbl(did))
                    .and_raw("  ")
                    .and(faint(&destination(&input_str(st, "baton"))))
            }
            AGENT_SEND => g("→")
                .and(lbl(&called()))
                .and_raw("  ")
                .and(faint(first_line(&input_str(st, "prompt")))),
            AGENT_RESULT => {
                let n = called();
                if running || st.status == Status::Failed {
                    return Some(g("◌").and(lbl(&format!("waiting on {n}"))));
                }
                let id = agent_id(st).unwrap_or_default();
                let out = &*st.output;
                match out.split_once("finished:") {
                    // Its answer, the first line of it on the row: the
                    // rest opens under it.
                    Some((_, answer)) => g("↩")
                        .and(lbl(&format!("{n} answered")))
                        .and_raw("  ")
                        .and(faint(first_line(answer))),
                    None => g("◌")
                        .and(lbl(&format!("checked on {n}")))
                        .and_raw("  ")
                        .and(faint(&standing(out, &id))),
                }
            }
            _ => return None,
        })
    }

    /// An opened retire call: the baton, drawn as the Markdown it was
    /// written in, then what rush said back when that wasn't the usual
    /// "retiring after this turn". Whether it was one.
    pub(crate) fn retire_body(&mut self, id: StepId, indent: usize) -> bool {
        let st = self.s.step(id);
        if rush_tool(&st.tool) != Some(RETIRE) {
            return false;
        }
        let baton = input_str(st, "baton");
        let failed = st.status == Status::Failed;
        let out = st.output.to_string();
        if self.peek && !baton.trim().is_empty() {
            // Open only as the latest step: a line, not the whole baton.
            let n = baton.trim().matches('\n').count() + 1;
            let l = self
                .sp(indent - 1)
                .and(dim(&format!("its baton · {} ", plural(n, "line"))))
                .and(faint("▸ open the row to read it"));
            self.add("", Color::Default, l, Txt::new());
        } else if !baton.trim().is_empty() {
            self.prose(&baton, indent, C_SUB);
        }
        if failed && !out.trim().is_empty() {
            self.output(&out, indent, true);
        }
        true
    }

    /// Under an agent_result call that waits, what the agent it waits on
    /// is doing now, as a running shell step's processes are said
    /// (live_rows): its working line, else the last thing it said.
    pub(crate) fn agent_now(&mut self, id: StepId, indent: usize) {
        let st = self.s.step(id);
        if st.status != Status::Running || rush_tool(&st.tool) != Some(AGENT_RESULT) {
            return;
        }
        let Some(s) = self.agent_of(st).and_then(|c| c.sess.as_deref()) else {
            return;
        };
        let (doing, _) = s.doing();
        let what = if doing.is_empty() {
            s.last_words()
        } else {
            doing
        };
        if what.is_empty() {
            return;
        }
        let room = (self.cw as isize - indent as isize - 4).max(8) as usize;
        let l = self
            .sp(indent - 1)
            .and(dim(&truncate_cells(&format!("now · {what}"), room)));
        self.add("", Color::Default, l, Txt::new());
    }

    /// What a running step does, for the line under the conversation that
    /// says what the agent is doing: rush's tools by what they do and to
    /// which agent, its harness when that's another ("waiting on astra
    /// (Codex)"); another MCP server's tool by its name and server;
    /// Claude Code loading a tool as that; anything else as its tool.
    pub(crate) fn running_words(&self, st: &Step) -> String {
        if let Some(w) = rush_words(self.s, st, true) {
            let other = self.agent_of(st).filter(|c| {
                !c.spawn.name.is_empty()
                    && crate::runname::other_harness(&c.spawn.kind, &self.o.kind)
            });
            return match other {
                Some(c) => format!("{w} ({})", c.spawn.name),
                None => w,
            };
        }
        if tool_load(st) {
            return "loading tools".into();
        }
        if let Some((server, tool)) = st
            .tool
            .strip_prefix("mcp__")
            .and_then(|r| r.split_once("__"))
        {
            return format!("running {tool} · {server}");
        }
        format!("running {}", one_line(&st.tool))
    }
}
