//! A live Claude Code session's events as rush's own, which any agent's
//! session sends. Port of internal/adapters/claude/headless/neutral.go:1-148
//! and 190-341 (Event, ungated, question, taskKind, partKind, turnReason,
//! alwaysLabel, workflowProgress); its messages go through crate::neutral, the same reader
//! transcripts use. claude/account.go:127-150 (LiveUsage) and quota.go
//! (Quota) for a rate_limit_event's windows.

use std::time::Duration;

use rush_core::event::{
    self, ApprovalOption, Ask, BackgroundTask, Choice, Event, OptionKind, PartKind, TaskKind,
};
use rush_core::time::Ms;
use rush_core::tool::Kind;
use rush_core::usage::{self, Billing, Quota, TokenUsage};
use rush_core::wire::RawJson;
use serde::Deserialize;

use super::event::{self as h, PermissionRequest};
use crate::json::nd;
use crate::message::Usage;
use crate::neutral::Neutral;
use crate::tools;

impl Neutral {
    /// ev as rush's own events: none for the traffic only the host sees
    /// (control replies, MCP messages), one for most, and more than one
    /// where Claude says two things at once.
    pub fn event(&mut self, ev: h::Event, now: Ms) -> Vec<Event> {
        match ev {
            h::Event::Init(e) => {
                let out = event::Init {
                    session_id: e.session_id,
                    model: e.model,
                    cwd: e.cwd,
                    mode: e.permission_mode,
                    version: e.version,
                    tools: e.tools,
                    commands: e.slash_commands,
                    mcp: e
                        .mcp_servers
                        .into_iter()
                        .map(|s| event::McpServer {
                            name: s.name,
                            status: s.status,
                        })
                        .collect(),
                    ..event::Init::default()
                };
                let mut v = vec![Event::Init(Box::new(out))];
                if !e.api_key_source.is_empty() && e.api_key_source != "none" {
                    // A sign-in's is told by its first request: plan or
                    // extra usage.
                    v.push(Event::Billing(event::Billing {
                        billing: Billing::Metered,
                    }));
                }
                v
            }
            h::Event::MessageStart(e) => vec![Event::MessageStart(event::MessageStart {
                id: e.message_id,
                model: e.model,
            })],
            h::Event::BlockStart(e) => vec![Event::PartStart(event::PartStart {
                index: e.index,
                kind: part_kind(&e.kind),
            })],
            h::Event::Delta(e) => {
                let kind = if e.thinking {
                    PartKind::Thinking
                } else if e.input {
                    PartKind::ToolCall
                } else {
                    PartKind::Text
                };
                vec![Event::Delta(event::Delta {
                    index: e.index,
                    kind,
                    text: e.text,
                })]
            }
            h::Event::Message(m) => vec![Event::Message(self.message(m))],
            h::Event::PermissionRequest(e) => vec![self.permission(&e)],
            h::Event::PermissionCancelled(id) => {
                vec![Event::ApprovalCancelled(event::ApprovalCancelled { id })]
            }
            h::Event::PermissionDenied(e) => vec![Event::Denied(event::Denied {
                call_id: e.tool_use_id,
                tool: e.tool,
                reason: e.reason,
            })],
            h::Event::Status(s) => vec![Event::Status(event::Status {
                busy: !s.is_empty(),
                text: s,
            })],
            h::Event::Compact(e) => vec![Event::Compacted(event::Compacted {
                trigger: e.trigger,
                before: e.pre_tokens,
                after: e.post_tokens,
            })],
            h::Event::Result(e) => {
                let mut end = event::TurnEnd {
                    reason: turn_reason(&e.subtype),
                    cost: e.cost_usd,
                    tokens: tokens(&e.usage),
                    duration: Duration::from_millis(e.duration_ms.max(0) as u64),
                    turns: e.num_turns,
                    ..event::TurnEnd::default()
                };
                if e.is_error {
                    end.err = e.text;
                } else {
                    end.text = e.text;
                }
                vec![Event::TurnEnd(end)]
            }
            h::Event::TaskStarted(e) => vec![Event::TaskStarted(event::TaskStarted {
                id: e.id,
                call_id: e.tool_use_id,
                kind: task_kind(&e.kind),
                label: first_of(e.description, e.workflow),
                agent: e.subagent_type,
                background: e.backgrounded,
            })],
            h::Event::TaskUpdated(e) => vec![Event::TaskUpdated(event::TaskUpdated {
                id: e.id,
                status: e.status,
                label: e.description,
                background: e.backgrounded,
                err: e.error,
            })],
            h::Event::TaskProgress(e) => {
                let (phases, agents) = workflow_progress(&e.workflow);
                vec![Event::TaskProgress(event::TaskProgress {
                    id: e.id,
                    summary: first_of(e.summary, e.description),
                    last_tool: e.last_tool,
                    tokens: e.tokens,
                    tool_uses: e.tool_uses,
                    phases,
                    agents,
                })]
            }
            h::Event::TaskDone(e) => vec![Event::TaskDone(event::TaskDone {
                id: e.id,
                call_id: e.tool_use_id,
                status: e.status,
                output_file: e.output_file,
                summary: e.summary,
            })],
            h::Event::BackgroundTasks(ts) => vec![Event::Background(event::Background {
                tasks: ts
                    .into_iter()
                    .map(|t| BackgroundTask {
                        id: t.id,
                        kind: task_kind(&t.kind),
                        kind_word: t.kind,
                        label: t.description,
                        parent: t.parent_task,
                    })
                    .collect(),
            })],
            h::Event::RateLimit(e) => rate_limit(&e, now),
            h::Event::ControlReply(_) | h::Event::McpRequest(_) => Vec::new(),
            h::Event::Other(o) => {
                if o.kind == "system"
                    && o.subtype == "api_retry"
                    && let Some(r) = o.raw.as_ref().and_then(|r| retry(r.get()))
                {
                    return vec![Event::Retry(r)];
                }
                vec![Event::Other(event::Other {
                    adapter: "claude".into(),
                    kind: format!("{}/{}", o.kind, o.subtype),
                    raw: o.raw,
                })]
            }
        }
    }

    /// A permission request as an approval, or, for AskUserQuestion, the
    /// question it asks.
    fn permission(&mut self, e: &PermissionRequest) -> Event {
        let (input, gated) = ungated(&e.tool, e.input.as_ref());
        let call = tools::call(&e.tool_use_id, &e.tool, input.as_ref().map(|r| &*r.0));
        self.calls.insert(e.tool_use_id.clone(), call.kind);
        if call.kind == Kind::Question {
            return Event::Question(question(e));
        }
        let mut options = vec![ApprovalOption {
            id: "allow".into(),
            label: "Allow".into(),
            kind: OptionKind::AllowOnce,
        }];
        // Always allowing a gated call would allow rush gate run, so
        // whatever it wraps.
        if let Some(s) = &e.suggestions
            && s.get() != "null"
            && !gated
        {
            options.push(ApprovalOption {
                id: "always".into(),
                label: always_label(s.get()),
                kind: OptionKind::AllowAlways,
            });
        }
        options.push(ApprovalOption {
            id: "deny".into(),
            label: "Deny".into(),
            kind: OptionKind::RejectOnce,
        });
        Event::Approval(Box::new(event::Approval {
            id: e.id.clone(),
            call,
            reason: first_of(e.reason.clone(), e.description.clone()),
            path: e.blocked_path.clone(),
            options,
        }))
    }
}

/// A rate_limit_event as the account's windows, how the request was paid
/// for, and, when it was refused, the limit it hit.
fn rate_limit(e: &h::RateLimit, now: Ms) -> Vec<Event> {
    let mut out = Vec::new();
    if let Some(mut q) = e.raw.as_ref().and_then(|r| live_quota(r.get(), now)) {
        q.source = usage::Source::Live;
        out.push(Event::Quota(q));
    }
    out.push(Event::Billing(event::Billing {
        billing: if e.overage {
            Billing::Overage
        } else {
            Billing::Plan
        },
    }));
    if e.status == "rejected" {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R {
            #[serde(rename = "rateLimitType", deserialize_with = "nd")]
            typ: String,
            #[serde(rename = "resetsAt", deserialize_with = "nd")]
            resets_at: i64,
        }
        let r: R = e
            .raw
            .as_ref()
            .and_then(|r| serde_json::from_str(r.get()).ok())
            .unwrap_or_default();
        out.push(Event::Limited(event::Limited {
            window: r.typ,
            resets_at: if r.resets_at > 0 {
                r.resets_at * 1000
            } else {
                0
            },
        }));
    }
    out
}

/// Claude's plan windows, by the names Anthropic gives them (quota.go:10).
const WINDOWS: [(&str, &str, &str, u64); 2] = [
    ("five_hour", "5h", "5-hour", 5 * 3600),
    ("seven_day", "7d", "weekly", 7 * 24 * 3600),
];

/// The plan usage a Claude Code session reports as it runs (a
/// rate_limit_event's info) as a reading of its windows: the same the
/// usage endpoint gives, fresh with every request. None when it has
/// neither window.
pub(crate) fn live_quota(info: &str, at: Ms) -> Option<Quota> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        #[serde(rename = "unifiedWindows")]
        windows: Option<std::collections::HashMap<String, Option<W>>>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct W {
        utilization: Option<f64>,
        #[serde(rename = "resetsAt", deserialize_with = "nd")]
        resets_at: i64,
    }
    let r: R = serde_json::from_str(info).ok()?;
    let ws = r.windows.unwrap_or_default();
    let mut q = Quota {
        fetched_at: at,
        source: usage::Source::Fetched,
        ..Quota::default()
    };
    for (id, label, name, span) in WINDOWS {
        let Some(Some(w)) = ws.get(id) else { continue };
        let Some(u) = w.utilization else { continue };
        q.windows.push(usage::Window {
            id: id.into(),
            label: label.into(),
            name: name.into(),
            span: Duration::from_secs(span),
            percent: u * 100.0,
            resets_at: if w.resets_at > 0 {
                w.resets_at * 1000
            } else {
                0
            },
            ..usage::Window::default()
        });
    }
    (!q.windows.is_empty()).then_some(q)
}

fn retry(raw: &str) -> Option<event::Retry> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct R {
        #[serde(deserialize_with = "nd")]
        attempt: i64,
        #[serde(deserialize_with = "nd")]
        max_retries: i64,
        #[serde(deserialize_with = "nd")]
        retry_delay_ms: i64,
        #[serde(deserialize_with = "nd")]
        error_status: i64,
        #[serde(deserialize_with = "nd")]
        error: String,
    }
    let r: R = serde_json::from_str(raw).ok()?;
    Some(event::Retry {
        attempt: r.attempt,
        max: r.max_retries,
        delay: Duration::from_millis(r.retry_delay_ms.max(0) as u64),
        status: r.error_status,
        err: r.error,
    })
}

/// A Bash call's input as Claude wrote it, before the gate's hook put it
/// under rush gate run, and whether it did.
fn ungated(tool: &str, input: Option<&RawJson>) -> (Option<RawJson>, bool) {
    let same = (input.cloned(), false);
    if tool != "Bash" {
        return same;
    }
    let Some(mut m) = input.and_then(|r| {
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(r.get()).ok()
    }) else {
        return same;
    };
    let Some(cmd) = m.get("command").and_then(|c| c.as_str()) else {
        return same;
    };
    let orig = gate_unwrap(cmd);
    if orig == cmd {
        return same;
    }
    m.insert("command".into(), serde_json::Value::String(orig));
    match RawJson::new(&serde_json::Value::Object(m).to_string()) {
        Ok(r) => (Some(r), true),
        Err(_) => same,
    }
}

/// What the gate's hook writes in a command it wrapped (gate/hook.go:15).
const WRAPPED: &str = " gate run --name ";

/// The command the gate wrapped, or command as it is. A copy of
/// rush_gate::hook::unwrap (gate/hook.go:67-91), which this crate doesn't
/// depend on.
fn gate_unwrap(command: &str) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    let Some(i) = command.find(WRAPPED) else {
        return command.to_string();
    };
    let mut sep = " -- bash -c ";
    let j = match command[i..].find(&format!("{sep}'")) {
        Some(j) => j,
        None => {
            sep = " -- /bin/bash -c ";
            match command[i..].find(&format!("{sep}'")) {
                Some(j) => j,
                None => return command.to_string(),
            }
        }
    };
    let q = &command[i + j + sep.len()..];
    if q.len() < 2 || !q.ends_with('\'') {
        return command.to_string();
    }
    let s = q[1..q.len() - 1].replace(r"'\''", "'");
    if quote(&s) != q {
        return command.to_string();
    }
    s
}

/// An AskUserQuestion request as the question it asks.
fn question(r: &PermissionRequest) -> event::Question {
    let (title, qs) = r.questions();
    event::Question {
        id: r.id.clone(),
        call_id: r.tool_use_id.clone(),
        title,
        asks: qs
            .into_iter()
            .map(|q| Ask {
                header: q.header,
                text: q.question,
                multi: q.multi_select,
                options: q
                    .options
                    .into_iter()
                    .map(|o| Choice {
                        label: o.label,
                        description: o.description,
                        preview: o.preview,
                        ..Choice::default()
                    })
                    .collect(),
                ..Ask::default()
            })
            .collect(),
    }
}

/// What a task of Claude Code's type is.
fn task_kind(t: &str) -> TaskKind {
    match t {
        "local_bash" => TaskKind::Shell,
        "local_agent" | "remote_agent" | "in_process_teammate" => TaskKind::Subagent,
        "monitor_mcp" | "monitor_ws" => TaskKind::Monitor,
        "local_workflow" => TaskKind::Workflow,
        _ => TaskKind::Other,
    }
}

fn part_kind(t: &str) -> PartKind {
    match t {
        "thinking" | "redacted_thinking" => PartKind::Thinking,
        "tool_use" => PartKind::ToolCall,
        _ => PartKind::Text,
    }
}

/// How a turn of Claude Code's subtype ended: done, or what went wrong in
/// its words (max_turns, during_execution).
fn turn_reason(subtype: &str) -> String {
    match subtype {
        "success" | "" => "done".into(),
        s => match s.strip_prefix("error_").unwrap_or(s) {
            "" => "error".into(),
            r => r.into(),
        },
    }
}

/// A turn's usage as rush counts it.
fn tokens(u: &Usage) -> TokenUsage {
    TokenUsage {
        input: u.input,
        output: u.output,
        cache_read: u.cache_read,
        cache_write_5m: u.cache_write_5m,
        cache_write_1h: u.cache_write_1h,
        reasoning: 0,
    }
}

/// A workflow's phases and agents from its progress lines: an agent's
/// later lines say more of it (neutral.go:306 workflowProgress).
pub(crate) fn workflow_progress(
    lines: &[h::WorkflowEntry],
) -> (Vec<String>, Vec<event::WorkflowAgent>) {
    let (mut phases, mut agents) = (Vec::<String>::new(), Vec::new());
    for l in lines {
        match l.kind.as_str() {
            "workflow_phase" => {
                if !(1..=64).contains(&l.index) {
                    continue;
                }
                let i = l.index as usize;
                if phases.len() < i {
                    phases.resize(i, String::new());
                }
                phases[i - 1] = l.title.clone();
            }
            "workflow_agent" => {
                let mut a = event::WorkflowAgent {
                    index: l.index,
                    label: l.label.clone(),
                    phase: l.phase_title.clone(),
                    id: l.agent_id.clone(),
                    model: l.model.clone(),
                    last_tool: l.last_tool.clone(),
                    doing: l.doing.clone(),
                    result: l.result.clone(),
                    error: l.error.clone(),
                    tokens: l.tokens,
                    tool_calls: l.tool_calls,
                    took: Duration::from_millis(l.duration_ms.max(0) as u64),
                    ..Default::default()
                };
                if l.started_at > 0 {
                    a.start = l.started_at;
                }
                a.state = match l.state.as_str() {
                    "start" if l.started_at > 0 => "running",
                    "start" => "queued",
                    "progress" => "running",
                    "done" => "done",
                    "error" => "failed",
                    _ => "",
                }
                .into();
                agents = event::merge_agents(agents, [a]);
            }
            _ => {}
        }
    }
    (phases, agents)
}

pub(crate) fn first_of(a: String, b: String) -> String {
    if a.is_empty() { b } else { a }
}

/// What always allowing saves, as Claude Code's suggested rules name it:
/// "Always allow Bash(pnpm lint:*)".
fn always_label(suggestions: &str) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Sug {
        #[serde(deserialize_with = "nd")]
        rules: Vec<Rule>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Rule {
        #[serde(rename = "toolName", deserialize_with = "nd")]
        tool: String,
        #[serde(rename = "ruleContent", deserialize_with = "nd")]
        content: String,
    }
    let mut rules = Vec::new();
    if let Ok(sug) = serde_json::from_str::<Vec<Sug>>(suggestions) {
        for s in sug {
            for r in s.rules {
                if !r.content.is_empty() {
                    rules.push(format!("{}({})", r.tool, r.content));
                } else if !r.tool.is_empty() {
                    rules.push(r.tool);
                }
            }
        }
    }
    if rules.is_empty() {
        return "Always allow".into();
    }
    format!("Always allow {}", rules.join(", "))
}

#[cfg(test)]
mod tests {
    use super::super::event::tests::*;
    use super::*;

    fn n(n: &mut Neutral, line: &str) -> Vec<Event> {
        n.event(dec(line), 0)
    }

    #[test]
    fn follows_a_turn() {
        let mut nt = Neutral::new();
        let Event::Init(i) = &n(&mut nt, INIT)[0] else {
            panic!()
        };
        assert_eq!(
            (
                i.session_id.as_str(),
                i.mode.as_str(),
                i.version.as_str(),
                i.commands.len()
            ),
            ("s-1", "default", "2.1.280", 1)
        );
        let Event::Delta(d) = &n(&mut nt, THINK)[0] else {
            panic!()
        };
        assert_eq!((d.kind, d.text.as_str()), (PartKind::Thinking, "hmm"));
        let Event::Message(m) = &n(&mut nt, TOOL_USE)[0] else {
            panic!()
        };
        let c = m.parts[0].call.as_ref().unwrap();
        assert_eq!((c.kind, c.input.command.as_str()), (Kind::Shell, "echo hi"));
        assert_eq!(m.tokens.unwrap().cache_read, 13689);
        let Event::Approval(a) = &n(&mut nt, ASK)[0] else {
            panic!()
        };
        assert_eq!((a.id.as_str(), a.call.kind), ("req-1", Kind::Shell));
        assert_eq!(a.options.len(), 3);
        assert_eq!(a.options[1].kind, OptionKind::AllowAlways);
        assert_eq!(a.options[1].label, "Always allow Bash(echo hi)");
        let Event::Message(r) = &n(&mut nt, TOOL_RESULT)[0] else {
            panic!()
        };
        let o = r.parts[0].output.as_ref().unwrap();
        assert_eq!((o.call_id.as_str(), o.exit), ("toolu_1", Some(0)));
        let Event::ApprovalCancelled(c) = &n(&mut nt, CANCEL)[0] else {
            panic!()
        };
        assert_eq!(c.id, "req-1");
        let Event::TurnEnd(e) = &n(&mut nt, RESULT)[0] else {
            panic!()
        };
        assert_eq!(e.reason, "done");
        assert_eq!(e.cost, 0.0247);
        assert_eq!(e.duration, Duration::from_millis(6900));
        assert_eq!(e.tokens.output, 289);
        let Event::Other(o) = &n(&mut nt, HOOK)[0] else {
            panic!()
        };
        assert_eq!(o.adapter, "claude");
        assert_eq!(o.kind, "system/hook_started");
    }

    #[test]
    fn questions_and_limits() {
        let mut nt = Neutral::new();
        let ask = r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","header":"Pick","multiSelect":false,"options":[{"label":"A","description":"a"},{"label":"B","description":"b"}]}]},"tool_use_id":"toolu_9"}}"#;
        let Event::Question(q) = &n(&mut nt, ask)[0] else {
            panic!()
        };
        assert_eq!((q.id.as_str(), q.call_id.as_str()), ("req-2", "toolu_9"));
        assert_eq!(q.asks.len(), 1);
        assert_eq!(q.asks[0].text, "Which?");
        assert_eq!(q.asks[0].options.len(), 2);
        let limit = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1790000000,"unifiedWindows":{"five_hour":{"utilization":1,"resetsAt":1790000000}}}}"#;
        let evs = n(&mut nt, limit);
        assert_eq!(evs.len(), 3, "{evs:?}");
        let Event::Quota(q) = &evs[0] else { panic!() };
        assert_eq!(q.used(""), 100.0);
        assert_eq!(q.source, usage::Source::Live);
        let Event::Billing(b) = &evs[1] else { panic!() };
        assert_eq!(b.billing, Billing::Plan);
        let Event::Limited(l) = &evs[2] else { panic!() };
        assert_eq!(
            (l.window.as_str(), l.resets_at),
            ("five_hour", 1_790_000_000_000)
        );
    }

    #[test]
    fn billing() {
        let mut nt = Neutral::new();
        let over = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","isUsingOverage":true,"unifiedWindows":{"five_hour":{"utilization":1,"resetsAt":1790000000}}}}"#;
        let evs = n(&mut nt, over);
        assert_eq!(evs.len(), 2);
        assert!(matches!(&evs[1], Event::Billing(b) if b.billing == Billing::Overage));
        // A sign-in's session is billed by what its requests say; an API
        // key's from the start.
        for (src, want) in [("none", 1), ("ANTHROPIC_API_KEY", 2)] {
            let evs = n(
                &mut nt,
                &format!(
                    r#"{{"type":"system","subtype":"init","session_id":"s","apiKeySource":"{src}"}}"#
                ),
            );
            assert_eq!(evs.len(), want, "{src}");
            if want == 2 {
                assert!(matches!(&evs[1], Event::Billing(b) if b.billing == Billing::Metered));
            }
        }
    }

    #[test]
    fn tasks() {
        let mut nt = Neutral::new();
        let Event::TaskStarted(st) = &n(
            &mut nt,
            r#"{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"tu1","task_type":"local_bash","description":"npm test","is_backgrounded":true}"#,
        )[0] else {
            panic!()
        };
        assert_eq!(
            (
                st.id.as_str(),
                st.call_id.as_str(),
                st.kind,
                st.label.as_str(),
                st.background
            ),
            ("b1", "tu1", TaskKind::Shell, "npm test", true)
        );
        let Event::Background(bg) = &n(
            &mut nt,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash","description":"npm test"}]}"#,
        )[0] else {
            panic!()
        };
        assert_eq!(bg.tasks.len(), 1);
        assert_eq!(
            (
                bg.tasks[0].kind,
                bg.tasks[0].kind_word.as_str(),
                bg.tasks[0].label.as_str()
            ),
            (TaskKind::Shell, "local_bash", "npm test")
        );
        // A subagent's command is in the list too, with the subagent's task.
        let Event::Background(sub) = &n(
            &mut nt,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b2","task_type":"local_bash","description":"bench","parent_task_id":"a1"}]}"#,
        )[0] else {
            panic!()
        };
        assert_eq!(sub.tasks[0].parent, "a1");
        let Event::Background(none) = &n(
            &mut nt,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#,
        )[0] else {
            panic!()
        };
        assert!(none.tasks.is_empty());
        let Event::TaskDone(d) = &n(
            &mut nt,
            r#"{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"tu1","status":"completed","output_file":"/t/b1.out"}"#,
        )[0] else {
            panic!()
        };
        assert_eq!(
            (d.id.as_str(), d.status.as_str(), d.output_file.as_str()),
            ("b1", "completed", "/t/b1.out")
        );
        let Event::TurnEnd(end) = &n(
            &mut nt,
            r#"{"type":"result","subtype":"success","result":"All green.","total_cost_usd":0.5}"#,
        )[0] else {
            panic!()
        };
        assert_eq!(
            (end.text.as_str(), end.err.as_str(), end.reason.as_str()),
            ("All green.", "", "done")
        );
        let Event::TaskUpdated(u) = &n(
            &mut nt,
            r#"{"type":"system","subtype":"task_updated","task_id":"b1","patch":{"is_backgrounded":true}}"#,
        )[0] else {
            panic!()
        };
        assert_eq!(u.background, Some(true));
    }

    #[test]
    fn retry_reads_as_retry() {
        let evs = n(
            &mut Neutral::new(),
            r#"{"type":"system","subtype":"api_retry","attempt":3,"max_retries":10,"retry_delay_ms":4200,"error_status":529,"error":"overloaded_error"}"#,
        );
        assert_eq!(evs.len(), 1);
        let Event::Retry(r) = &evs[0] else { panic!() };
        assert_eq!(
            (r.attempt, r.max, r.delay, r.status),
            (3, 10, Duration::from_millis(4200), 529)
        );
    }

    #[test]
    fn always_label_says_what_it_saves() {
        assert_eq!(
            always_label(
                r#"[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"pnpm lint:*"}],"behavior":"allow","destination":"localSettings"}]"#
            ),
            "Always allow Bash(pnpm lint:*)"
        );
        assert_eq!(
            always_label(r#"[{"type":"setMode","mode":"acceptEdits"}]"#),
            "Always allow"
        );
    }

    #[test]
    fn turn_reasons() {
        for (sub, want) in [
            ("success", "done"),
            ("", "done"),
            ("error_max_turns", "max_turns"),
            ("error_during_execution", "during_execution"),
            ("error_", "error"),
            ("odd", "odd"),
        ] {
            assert_eq!(turn_reason(sub), want, "{sub}");
        }
    }

    #[test]
    fn a_gated_call_reads_as_written_and_is_never_always() {
        let cmd = "'/usr/local/bin/rush' gate run --name 'pnpm' --dir '/w' -- bash -c 'pnpm test'";
        let line = format!(
            r#"{{"type":"control_request","request_id":"g","request":{{"subtype":"can_use_tool","tool_name":"Bash","input":{{"command":{}}},"permission_suggestions":[{{"rules":[{{"toolName":"Bash"}}]}}],"tool_use_id":"t"}}}}"#,
            serde_json::to_string(cmd).unwrap()
        );
        let Event::Approval(a) = &n(&mut Neutral::new(), &line)[0] else {
            panic!()
        };
        assert_eq!(a.call.input.command, "pnpm test");
        assert_eq!(a.options.len(), 2, "no always for a gated call");
    }

    // workflow_test.go:15 (TestNeutralWorkflowProgress): a workflow's
    // progress says its phases and how each agent stands: an agent queued,
    // then running, then done, reads as the last word of it, with what the
    // earlier ones said kept.
    #[test]
    fn neutral_workflow_progress() {
        let mut nt = Neutral::new();
        let line = r#"{"type":"system","subtype":"task_progress","task_id":"w1","usage":{"total_tokens":900,"tool_uses":4},"workflow_progress":[
		{"type":"workflow_phase","index":1,"title":"Port"},
		{"type":"workflow_phase","index":2,"title":"Check"},
		{"type":"workflow_agent","index":1,"label":"port:hl","phaseIndex":1,"phaseTitle":"Port","model":"claude-sonnet-5-5","state":"start","queuedAt":1},
		{"type":"workflow_agent","index":1,"agentId":"a1","state":"progress","startedAt":2000,"lastToolName":"Bash","lastToolSummary":"cargo build"},
		{"type":"workflow_agent","index":2,"label":"port:draw","phaseTitle":"Port","state":"start","queuedAt":1},
		{"type":"workflow_agent","index":3,"label":"port:claude","phaseTitle":"Port","agentId":"a3","state":"done","startedAt":1000,"durationMs":5000,"tokens":300,"resultPreview":"ported"},
		{"type":"workflow_log","index":1}
	]}"#;
        let Event::TaskProgress(p) = &n(&mut nt, line)[0] else {
            panic!("not progress")
        };
        assert_eq!(p.phases, ["Port", "Check"]);
        assert_eq!(p.agents.len(), 3, "{:?}", p.agents);
        let a = &p.agents[0];
        assert_eq!(
            (
                a.label.as_str(),
                a.id.as_str(),
                a.state.as_str(),
                a.model.as_str(),
                a.last_tool.as_str(),
                a.doing.as_str()
            ),
            (
                "port:hl",
                "a1",
                "running",
                "claude-sonnet-5-5",
                "Bash",
                "cargo build"
            ),
            "a queued agent that started"
        );
        assert_eq!(p.agents[1].state, "queued", "one not started");
        let a = &p.agents[2];
        assert_eq!(
            (a.state.as_str(), a.result.as_str(), a.tokens, a.took),
            ("done", "ported", 300, Duration::from_secs(5)),
            "one done"
        );
    }
}
