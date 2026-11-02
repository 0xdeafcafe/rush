//! What a workflow leaves on disk, for one rush didn't hear run: the
//! session's own, before rush was there, or one it ran before a restart.
//! Claude Code keeps each run's agents beside the session's transcript, in
//! subagents/workflows/<run>/: a journal of each agent started and what it
//! gave back, and each agent's transcript. Once the run ends, it writes the
//! whole of its progress to workflows/<run>.json.
//!
//! Port of internal/adapters/claude/headless/workflow.go:1-163 (Workflow,
//! WorkflowDir, ReadWorkflow, scriptMeta, readJournal with its failed
//! tries, resultText).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use regex::bytes::Regex as BytesRegex;
use rush_core::event::{WorkflowAgent, merge_agents};
use serde::Deserialize;
use serde_json::value::RawValue;

use super::event::WorkflowEntry;
use super::neutral::{first_of, workflow_progress};
use crate::json::nd;

/// A workflow run's phases and agents, as its files have them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workflow {
    /// What it's for: its script's description, else its name.
    pub title: String,
    pub phases: Vec<String>,
    pub agents: Vec<WorkflowAgent>,
    /// Read from what it wrote as it ended: it won't change.
    pub done: bool,
}

/// Finds the run's folder in what the Workflow tool gave back.
static WORKFLOW_DIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^Transcript dir: (.+)$").unwrap());

/// The folder of the run the Workflow tool started, from what it gave
/// back; "" when it doesn't say.
pub fn workflow_dir(output: &str) -> String {
    WORKFLOW_DIR_RE
        .captures(output)
        .map(|m| m[1].trim().to_string())
        .unwrap_or_default()
}

/// Reads the run whose folder is dir: what it wrote as it ended, else its
/// journal so far.
pub fn read_workflow(dir: &Path) -> Workflow {
    let run = dir
        .file_name()
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_default();
    // <session>/subagents/workflows/<run>
    let session: PathBuf = dir
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if let Ok(b) = fs::read(session.join("workflows").join(format!("{run}.json"))) {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Phase {
            #[serde(deserialize_with = "nd")]
            title: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct R {
            #[serde(deserialize_with = "nd")]
            phases: Vec<Phase>,
            #[serde(rename = "workflowProgress", deserialize_with = "nd")]
            progress: Vec<WorkflowEntry>,
            #[serde(deserialize_with = "nd")]
            summary: String,
            #[serde(rename = "workflowName", deserialize_with = "nd")]
            name: String,
        }
        if let Ok(r) = serde_json::from_slice::<R>(&b)
            && !r.progress.is_empty()
        {
            let (mut phases, agents) = workflow_progress(&r.progress);
            if phases.is_empty() {
                phases = r.phases.into_iter().map(|p| p.title).collect();
            }
            return Workflow {
                title: first_of(r.summary, r.name),
                phases,
                agents,
                done: true,
            };
        }
    }
    let mut w = read_journal(&dir.join("journal.jsonl"));
    // The phases it will go through are in its script's meta.
    if let Some(script) = script_of(&session.join("workflows").join("scripts"), &run) {
        let (title, planned) = script_meta(&script);
        if planned.len() >= w.phases.len() {
            w.phases = planned;
        }
        w.title = title;
    }
    w
}

/// The first script in dir whose name ends <run>.js, as Go's Glob of
/// "*<run>.js" finds it.
fn script_of(dir: &Path, run: &str) -> Option<PathBuf> {
    let suffix = format!("{run}.js");
    let mut names: Vec<String> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(&suffix))
        .collect();
    names.sort();
    names.first().map(|n| dir.join(n))
}

/// A quoted string in a script: '...', "..." or `...`.
const QUOTED: &str = r#"(?:'([^']*)'|"([^"]*)"|`([^`]*)`)"#;

/// A workflow script's meta phases, and each one's title; its description
/// and name.
static PHASES_RE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?s)phases\s*:\s*\[(.*?)\]\s*,?\s*\}").unwrap());
static DESC_RE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(&format!(r"description\s*:\s*{QUOTED}")).unwrap());
static NAME_RE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(&format!(r"name\s*:\s*{QUOTED}")).unwrap());
static TITLE_RE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(&format!(r"title\s*:\s*{QUOTED}")).unwrap());

/// The quoted string a match caught, whichever quotes it had.
fn quoted(c: &regex::bytes::Captures) -> String {
    (1..=3)
        .filter_map(|i| c.get(i))
        .map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned())
        .collect()
}

/// What a workflow's script says it's for, and the phases it has, in
/// order.
fn script_meta(path: &Path) -> (String, Vec<String>) {
    let Ok(b) = fs::read(path) else {
        return (String::new(), Vec::new());
    };
    let b = match memchr::memmem::find(&b, b"export const meta") {
        Some(i) => &b[i..],
        None => &b[..],
    };
    let mut title = String::new();
    for re in [&*DESC_RE, &*NAME_RE] {
        if title.is_empty()
            && let Some(c) = re.captures(b)
        {
            title = quoted(&c);
        }
    }
    let Some(m) = PHASES_RE.captures(b) else {
        return (title, Vec::new());
    };
    let phases = TITLE_RE.captures_iter(&m[1]).map(|c| quoted(&c)).collect();
    (title, phases)
}

/// A run's agents from its journal: each one started, and what each gave
/// back or that it failed. An agent is known there by its id, and numbered
/// in the order it started.
fn read_journal(path: &Path) -> Workflow {
    let Ok(f) = fs::File::open(path) else {
        return Workflow::default();
    };
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct L {
        #[serde(rename = "type", deserialize_with = "nd")]
        kind: String,
        #[serde(deserialize_with = "nd")]
        agent_id: String,
        #[serde(deserialize_with = "nd")]
        label: String,
        #[serde(deserialize_with = "nd")]
        phase: String,
        result: Option<Box<RawValue>>,
    }
    let mut w = Workflow::default();
    let mut at: HashMap<String, i64> = HashMap::new();
    for line in BufReader::new(f).split(b'\n').map_while(Result::ok) {
        let Ok(l) = serde_json::from_slice::<L>(line.trim_ascii()) else {
            continue;
        };
        if l.agent_id.is_empty() {
            continue;
        }
        match l.kind.as_str() {
            "started" => {
                if !l.phase.is_empty() && !w.phases.contains(&l.phase) {
                    w.phases.push(l.phase.clone());
                }
                let next = w.agents.len() as i64 + 1;
                let i = *at.entry(l.agent_id.clone()).or_insert(next);
                w.agents = merge_agents(
                    std::mem::take(&mut w.agents),
                    [WorkflowAgent {
                        index: i,
                        id: l.agent_id,
                        label: l.label,
                        phase: l.phase,
                        state: "running".into(),
                        ..Default::default()
                    }],
                );
            }
            "result" | "failed" => {
                let Some(&i) = at.get(&l.agent_id) else {
                    continue;
                };
                let a = if l.kind == "result" {
                    WorkflowAgent {
                        index: i,
                        state: "done".into(),
                        result: result_text(l.result.as_deref()),
                        ..Default::default()
                    }
                } else {
                    WorkflowAgent {
                        index: i,
                        state: "failed".into(),
                        ..Default::default()
                    }
                };
                w.agents = merge_agents(std::mem::take(&mut w.agents), [a]);
            }
            _ => {}
        }
    }
    w
}

/// The start of what an agent gave back: its words, or the summary of
/// what it gave back as an object.
fn result_text(v: Option<&RawValue>) -> String {
    let raw = v.map_or("null", RawValue::get);
    let mut s = match serde_json::from_str::<String>(raw) {
        Ok(s) => s,
        Err(_) => {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct O {
                #[serde(deserialize_with = "nd")]
                summary: String,
            }
            serde_json::from_str::<O>(raw)
                .map(|o| o.summary)
                .unwrap_or_default()
        }
    };
    if s.len() > 400 {
        // Cut at 400 bytes, dropping a character split there.
        let mut n = 400;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // workflow_test.go:45 (TestReadWorkflowJournal): a run rush didn't
    // hear: its journal says which agents started, which gave back and
    // which failed, and its script the phases it will go through.
    #[test]
    fn read_workflow_journal() {
        let t = tempfile::tempdir().unwrap();
        let session = t.path();
        let dir = session.join("subagents/workflows/wf_1");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(session.join("workflows/scripts")).unwrap();
        fs::write(
            dir.join("journal.jsonl"),
            r#"{"type":"launched"}
{"type":"started","key":"k1","agentId":"a1","label":"port:hl","phase":"Port"}
{"type":"started","key":"k2","agentId":"a2","label":"port:draw","phase":"Port"}
{"type":"result","key":"k1","agentId":"a1","result":{"summary":"ported hl","tests":3}}
{"type":"started","key":"k3","agentId":"a3","label":"port:check","phase":"Port"}
{"type":"failed","key":"k3","agentId":"a3"}
"#,
        )
        .unwrap();
        fs::write(
            session.join("workflows/scripts/port-wf_1.js"),
            r#"export const meta = {
  name: 'port',
  description: 'Port it',
  phases: [
    { title: 'Port', detail: 'in parallel' },
    { title: "Check" },
  ],
}
phase('Port')
"#,
        )
        .unwrap();
        let w = read_workflow(&dir);
        assert!(!w.done);
        assert_eq!(w.phases, ["Port", "Check"]);
        assert_eq!(w.title, "Port it");
        let st: Vec<(&str, &str, &str)> = w
            .agents
            .iter()
            .map(|a| (a.id.as_str(), a.state.as_str(), a.result.as_str()))
            .collect();
        assert_eq!(
            st,
            [
                ("a1", "done", "ported hl"),
                ("a2", "running", ""),
                ("a3", "failed", "")
            ]
        );

        // Ended, it wrote its whole progress: that's what it says.
        fs::write(
            session.join("workflows/wf_1.json"),
            r#"{"phases":[{"title":"Port"}],"workflowProgress":[
		{"type":"workflow_agent","index":1,"label":"port:hl","phaseTitle":"Port","agentId":"a1","state":"done","tokens":10},
		{"type":"workflow_agent","index":2,"label":"port:draw","phaseTitle":"Port","agentId":"a2","state":"error","error":"boom"}]}"#,
        )
        .unwrap();
        let w = read_workflow(&dir);
        assert!(w.done);
        assert_eq!(w.phases, ["Port"]);
        assert_eq!(w.agents.len(), 2);
        assert_eq!(
            (w.agents[1].state.as_str(), w.agents[1].error.as_str()),
            ("failed", "boom")
        );
    }

    // workflow_test.go:91 (TestWorkflowDir): the Workflow tool says where
    // its run's folder is.
    #[test]
    fn workflow_dir_from_output() {
        let out = "Workflow launched in background. Task ID: w1\nSummary: x\nTranscript dir: /p/s/subagents/workflows/wf_1\nScript file: /p/s/workflows/scripts/x.js\n";
        assert_eq!(workflow_dir(out), "/p/s/subagents/workflows/wf_1");
    }
}
