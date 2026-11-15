//! Every other model rush runs, as Claude Code's own subagent types: each
//! is a --agents definition, and rush answers the Agent call for one
//! itself. Claude Code has no way to run a subagent on another provider's
//! endpoint, so the definition is a stand-in on haiku that says one word;
//! a PreToolUse hook rush registers at initialize keeps the call's task
//! and hands the stand-in a one-line one instead, and the PostToolUse
//! hook runs the task on the model asked for (the Delegate: a rush-hosted
//! child) and puts its answer in the Agent tool's own output
//! (updatedToolOutput), which is what the session reads. A call in the
//! background starts the model in the background and returns at once;
//! its answer comes as rush's message. See docs/native-agents.md.
//!
//! Rust-only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard};

use rush_core::adapter::{DelegateCall, DelegateRef, OtherAgent};
use rush_core::wire::RawJson;
use serde_json::{Value, json};

/// The ids rush's hooks are registered under.
pub const PRE: &str = "rush-others-pre";
pub const POST: &str = "rush-others-post";

/// How long the hook that waits on another model's answer may: past
/// rush's own wait for one (subtool's FOREGROUND), which then says it's
/// still working.
pub const POST_TIMEOUT_SECS: u64 = 3600;

/// The stand-in's model: the cheapest of Claude Code's own, mapped to a
/// riding provider's smallest.
pub const STAND_IN_MODEL: &str = "haiku";

/// What the stand-in is, and the task it's handed in place of the real
/// one: it only has to end its turn.
pub const STAND_IN_PROMPT: &str = "Reply with the single word: ok. Do nothing else.";
pub const STAND_IN_TASK: &str = "Reply with the single word: ok.";

/// Adds each of others to agents as a stand-in definition, unless one of
/// that name is there already (the session's own, a plugin's).
pub fn stand_ins(others: &[OtherAgent], agents: &mut BTreeMap<String, RawJson>) {
    for o in others {
        if agents.contains_key(&o.name) {
            continue;
        }
        let v = json!({
            "description": o.description,
            "model": STAND_IN_MODEL,
            "prompt": STAND_IN_PROMPT,
            "tools": [],
            "maxTurns": 1,
        });
        if let Ok(b) = RawJson::new(&v.to_string()) {
            agents.insert(o.name.clone(), b);
        }
    }
}

/// The hooks initialize registers: before and after each Agent call.
pub fn hooks() -> Value {
    json!({
        "PreToolUse": [{"matcher": "Agent|Task", "hookCallbackIds": [PRE], "timeout": 10}],
        "PostToolUse": [{"matcher": "Agent|Task", "hookCallbackIds": [POST], "timeout": POST_TIMEOUT_SECS}],
    })
}

/// A session's other models, and the calls for them under way.
pub struct Native {
    delegate: DelegateRef,
    names: HashSet<String>,
    /// Calls rush took, by tool_use_id, from before the call to after it:
    /// where their model's answer comes.
    calls: Mutex<HashMap<String, Receiver<Result<String, String>>>>,
    /// Calls whose model is still at work.
    open: Arc<AtomicUsize>,
}

impl Native {
    /// The session's, when it has a delegate and models to offer.
    pub fn of(others: &[OtherAgent], delegate: Option<DelegateRef>) -> Option<Native> {
        let delegate = delegate?;
        if others.is_empty() {
            return None;
        }
        Some(Native {
            delegate,
            names: others.iter().map(|o| o.name.clone()).collect(),
            calls: Mutex::new(HashMap::new()),
            open: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn calls(&self) -> MutexGuard<'_, HashMap<String, Receiver<Result<String, String>>>> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a call is under way: a host can't hand the session on now.
    pub fn busy(&self) -> bool {
        self.open.load(Ordering::SeqCst) > 0 || !self.calls().is_empty()
    }

    /// The PreToolUse hook's output for an Agent call (its hook input):
    /// for one of rush's types, the stand-in's input; else no word. The
    /// model asked for starts now, beside the stand-in, so the run rush
    /// hosts is there as soon as Claude Code's (which the UI then hides
    /// behind it) and its answer is no later for the stand-in's turn.
    pub fn pre(&self, input: &Value) -> Value {
        let ti = &input["tool_input"];
        let ty = ti["subagent_type"].as_str().unwrap_or_default();
        if !self.names.contains(ty) {
            return json!({});
        }
        let id = input["tool_use_id"].as_str().unwrap_or_default();
        let call = DelegateCall {
            agent: ty.to_string(),
            prompt: ti["prompt"].as_str().unwrap_or_default().to_string(),
            background: ti["run_in_background"].as_bool().unwrap_or(false),
            step: id.to_string(),
        };
        let (tx, rx) = channel();
        self.calls().insert(id.to_string(), rx);
        let (delegate, open) = (self.delegate.clone(), self.open.clone());
        open.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let out = delegate.run(call);
            open.fetch_sub(1, Ordering::SeqCst);
            let _ = tx.send(out);
        });
        let mut stand_in = ti.clone();
        if let Some(m) = stand_in.as_object_mut() {
            m.insert("prompt".into(), STAND_IN_TASK.into());
            // It ends at once: the model's own call goes in the
            // background, if asked, not the stand-in.
            m.insert("run_in_background".into(), false.into());
            for k in ["model", "isolation"] {
                m.remove(k);
            }
        }
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": stand_in,
        }})
    }

    /// The PostToolUse hook's output for an Agent call (its hook input):
    /// for one rush took, the model's answer as the call's output, once
    /// it has one; else no word. It waits as long as the model works.
    pub fn post(&self, input: &Value) -> Value {
        let id = input["tool_use_id"].as_str().unwrap_or_default();
        let Some(rx) = self.calls().remove(id) else {
            return json!({});
        };
        let text = match rx.recv() {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => e,
            Err(_) => "the model's run ended without an answer".to_string(),
        };
        json!({"hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": answered(&input["tool_response"], &text),
        }})
    }
}

/// The Agent tool's output with text as its answer: the stand-in's
/// output whole, its words replaced.
pub fn answered(out: &Value, text: &str) -> Value {
    let content = json!([{"type": "text", "text": text}]);
    match out {
        Value::Object(m) => {
            let mut m = m.clone();
            m.insert("content".into(), content);
            Value::Object(m)
        }
        _ => json!({"status": "completed", "content": content}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Echo(Mutex<Vec<DelegateCall>>);

    impl rush_core::adapter::Delegate for Echo {
        fn run(&self, call: DelegateCall) -> Result<String, String> {
            let out = format!("{} did: {}", call.agent, call.prompt);
            self.0.lock().unwrap().push(call);
            if out.contains("fail") {
                return Err("kimi couldn't start: no".into());
            }
            Ok(out)
        }
    }

    fn native() -> (Native, Arc<Echo>) {
        let e = Arc::new(Echo(Mutex::new(Vec::new())));
        let others = vec![
            OtherAgent {
                name: "kimi".into(),
                description: "Moonshot's.".into(),
            },
            OtherAgent {
                name: "top".into(),
                description: "The top tier.".into(),
            },
        ];
        (Native::of(&others, Some(e.clone())).unwrap(), e)
    }

    fn call(ty: &str, prompt: &str, background: bool) -> Value {
        json!({"hook_event_name": "PreToolUse", "tool_name": "Agent", "tool_use_id": format!("tu-{ty}"),
            "tool_input": {"subagent_type": ty, "prompt": prompt, "description": "x",
                "run_in_background": background, "model": "opus"}})
    }

    #[test]
    fn stand_ins_leave_the_sessions_own_types() {
        let mut agents = BTreeMap::new();
        agents.insert(
            "opus".to_string(),
            RawJson::new(r#"{"model":"opus"}"#).unwrap(),
        );
        stand_ins(
            &[
                OtherAgent {
                    name: "opus".into(),
                    description: "x".into(),
                },
                OtherAgent {
                    name: "kimi".into(),
                    description: "Moonshot's.".into(),
                },
            ],
            &mut agents,
        );
        assert_eq!(agents["opus"].get(), r#"{"model":"opus"}"#);
        let k: Value = serde_json::from_str(agents["kimi"].get()).unwrap();
        assert_eq!(k["description"], "Moonshot's.");
        assert_eq!(k["model"], STAND_IN_MODEL);
        assert_eq!(k["maxTurns"], 1);
        assert_eq!(k["tools"], json!([]));
    }

    /// A call for one of rush's types: before it, the stand-in's input;
    /// after it, the model's answer as the Agent tool's output, the rest
    /// of that output kept. Any other call is left alone.
    #[test]
    fn a_call_for_another_model_is_answered_by_it() {
        let (n, e) = native();
        for (ty, prompt, bg, want) in [
            (
                "kimi",
                "review main.rs",
                false,
                Some("kimi did: review main.rs"),
            ),
            ("top", "design it", true, Some("top did: design it")),
            ("kimi", "fail", false, Some("kimi couldn't start: no")),
            ("general-purpose", "x", false, None),
        ] {
            let pre = n.pre(&call(ty, prompt, bg));
            let Some(answer) = want else {
                assert_eq!(pre, json!({}), "{ty}");
                let mut post = call(ty, prompt, bg);
                post["tool_response"] = json!({"status": "completed"});
                assert_eq!(n.post(&post), json!({}), "{ty}");
                continue;
            };
            let h = &pre["hookSpecificOutput"];
            assert_eq!(h["permissionDecision"], "allow");
            let u = &h["updatedInput"];
            assert_eq!(u["prompt"], STAND_IN_TASK);
            assert_eq!(u["run_in_background"], false);
            assert_eq!(u["subagent_type"], ty);
            assert!(u.get("model").is_none(), "{u}");
            assert!(n.busy());
            let mut post = call(ty, STAND_IN_TASK, false);
            post["hook_event_name"] = "PostToolUse".into();
            post["tool_response"] = json!({"status": "completed", "agentId": "a1",
                "content": [{"type": "text", "text": "ok"}], "totalTokens": 9});
            let out = n.post(&post);
            let o = &out["hookSpecificOutput"]["updatedToolOutput"];
            assert_eq!(o["content"][0]["text"], answer, "{ty}");
            assert_eq!(
                (o["agentId"].as_str(), o["totalTokens"].as_i64()),
                (Some("a1"), Some(9))
            );
            assert!(!n.busy());
        }
        let got: Vec<String> =
            e.0.lock()
                .unwrap()
                .iter()
                .map(|c| format!("{} {} {} {}", c.agent, c.prompt, c.background, c.step))
                .collect();
        assert_eq!(
            got,
            [
                "kimi review main.rs false tu-kimi",
                "top design it true tu-top",
                "kimi fail false tu-kimi",
            ]
        );
    }

    #[test]
    fn no_delegate_or_none_to_offer_is_none() {
        assert!(Native::of(&[], Some(Arc::new(Echo(Mutex::new(Vec::new()))))).is_none());
        assert!(
            Native::of(
                &[OtherAgent {
                    name: "kimi".into(),
                    description: String::new()
                }],
                None
            )
            .is_none()
        );
    }
}
