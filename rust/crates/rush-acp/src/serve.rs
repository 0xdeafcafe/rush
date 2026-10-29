//! The agent's requests of rush: permission for a tool call, a form for the
//! user to fill in, and its files read and written. Those that wait on the
//! user are answered later, through answer and answer_question. Port of
//! internal/adapters/acp/serve.go:1-299.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use rush_core::event::{self, Event, OptionKind};
use rush_core::tool;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::protocol::{PermissionOption, ToolCall};
use crate::rpc::{
    CODE_CANCELLED, CODE_INTERNAL, CODE_INVALID_PARAMS, CODE_METHOD_NOT_FOUND, CODE_NOT_FOUND,
    Error, RpcError,
};
use crate::session::{Inner, go_quote, lock};

/// A form the agent asked the user to fill in, as a Question.
pub(crate) struct Elicitation {
    pub id: Box<RawValue>,
    fields: HashMap<String, Field>,
}

struct Field {
    /// string, number, integer, boolean, array
    typ: String,
    /// A choice's label → the value it sends.
    values: HashMap<String, String>,
}

/// A request id as rush's approval and question ids.
fn request_key(id: &RawValue) -> String {
    id.get().trim_matches('"').to_string()
}

impl Inner {
    pub(crate) fn serve(&self, id: Box<RawValue>, method: &str, params: Option<&RawValue>) {
        let no_fs = lock(&self.mu).o.no_fs;
        let params = params.map_or("null", RawValue::get);
        match method {
            "session/request_permission" => self.permission(id, params),
            "elicitation/create" => self.elicit(id, params),
            "fs/read_text_file" | "fs/write_text_file" if !no_fs => {
                let method = method.to_string();
                let params = params.to_string();
                // The reader goes on reading while the disk is busy; the
                // reply is written from here.
                let Some(me) = self.me.upgrade() else {
                    return;
                };
                std::thread::spawn(move || {
                    let (res, err) = match fs_call(&method, &params) {
                        Ok(r) => (r, None),
                        Err(e) => (None, Some(e)),
                    };
                    let _ = me.rpc.reply(&id, res.as_ref(), err.as_ref());
                });
            }
            _ => {
                let _ = self.rpc.reply(
                    &id,
                    None,
                    Some(&RpcError::new(
                        CODE_METHOD_NOT_FOUND,
                        format!("method not found: {method}"),
                    )),
                );
            }
        }
    }

    fn permission(&self, id: Box<RawValue>, params: &str) {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct P {
            #[serde(rename = "toolCall")]
            tool_call: ToolCall,
            options: Option<Vec<PermissionOption>>,
        }
        let p = match serde_json::from_str::<P>(params) {
            Ok(p) => p,
            Err(e) => {
                let _ = self.rpc.reply(
                    &id,
                    None,
                    Some(&RpcError::new(CODE_INVALID_PARAMS, e.to_string())),
                );
                return;
            }
        };
        let options = p.options.unwrap_or_default();
        let mut st = lock(&self.mu);
        // The call may not have been announced yet; the approval needs it
        // to hang off.
        let cid = self.track(&mut st, p.tool_call);
        let c = st.calls[&cid].c.clone();
        if trusted(&st.o.trusted, &c) {
            for o in &options {
                if o.kind == "allow_once" || o.kind == "allow_always" {
                    let _ = self.rpc.reply(
                        &id,
                        Some(&json!({"outcome": {"optionId": o.option_id, "outcome": "selected"}})),
                        None,
                    );
                    return;
                }
            }
        }
        let key = request_key(&id);
        lock(&self.amu).approvals.insert(key.clone(), id);
        let a = event::Approval {
            id: key,
            path: c.input.path.clone(),
            call: c,
            options: options
                .into_iter()
                .map(|o| event::ApprovalOption {
                    id: o.option_id,
                    label: o.name,
                    kind: option_kind(&o.kind),
                })
                .collect(),
            ..event::Approval::default()
        };
        self.emit(Event::Approval(Box::new(a)));
    }

    /// The agent taking back a request of its own.
    pub(crate) fn withdraw(&self, id: &RawValue) {
        let key = request_key(id);
        let (approval, question) = {
            let mut w = lock(&self.amu);
            (w.approvals.remove(&key), w.questions.remove(&key))
        };
        let cancelled = RpcError::new(CODE_CANCELLED, "cancelled");
        let rid = match (approval, question) {
            (Some(rid), _) => rid,
            (None, Some(q)) => q.id,
            (None, None) => return,
        };
        let _ = self.rpc.reply(&rid, None, Some(&cancelled));
        let _st = lock(&self.mu);
        self.emit(Event::ApprovalCancelled(event::ApprovalCancelled {
            id: key,
        }));
    }

    /// Turns a form elicitation into a Question: one Ask a field. A URL
    /// elicitation isn't advertised, so is declined.
    fn elicit(&self, id: Box<RawValue>, params: &str) {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Titled {
            #[serde(rename = "const")]
            konst: String,
            title: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Items {
            #[serde(rename = "enum")]
            enums: Vec<String>,
            #[serde(rename = "anyOf")]
            any_of: Vec<Titled>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Prop {
            #[serde(rename = "type")]
            typ: String,
            title: String,
            #[serde(rename = "enum")]
            enums: Vec<String>,
            #[serde(rename = "oneOf")]
            one_of: Vec<Titled>,
            items: Option<Items>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Schema {
            properties: BTreeMap<String, Prop>,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct P {
            mode: String,
            message: String,
            #[serde(rename = "toolCallId")]
            tool_call_id: String,
            #[serde(rename = "requestedSchema")]
            requested_schema: Schema,
        }
        let p = match serde_json::from_str::<P>(params) {
            Ok(p) if p.mode == "form" => p,
            _ => {
                let _ = self
                    .rpc
                    .reply(&id, Some(&json!({"action": "decline"})), None);
                return;
            }
        };
        let key = request_key(&id);
        let mut q = event::Question {
            id: key.clone(),
            call_id: p.tool_call_id.clone(),
            title: p.message,
            ..event::Question::default()
        };
        {
            let st = lock(&self.mu);
            if st
                .calls
                .get(&p.tool_call_id)
                .is_some_and(|c| c.c.input.background)
            {
                // Asked in the background, its call returns before it's
                // answered: it stands on its own, not on a call whose result
                // would close it.
                q.call_id.clear();
            }
        }
        let mut fields = HashMap::new();
        for (name, ps) in p.requested_schema.properties {
            let mut f = Field {
                typ: ps.typ.clone(),
                values: HashMap::new(),
            };
            let mut ask = event::Ask {
                id: name.clone(),
                header: ps.title.clone(),
                text: if ps.title.is_empty() {
                    name.clone()
                } else {
                    ps.title.clone()
                },
                multi: ps.typ == "array",
                ..event::Ask::default()
            };
            let mut choose = |value: &str, label: &str| {
                let label = if label.is_empty() { value } else { label };
                f.values.insert(label.to_string(), value.to_string());
                ask.options.push(event::Choice {
                    label: label.to_string(),
                    ..event::Choice::default()
                });
            };
            let (enums, one) = match &ps.items {
                Some(i) => (&i.enums, &i.any_of),
                None => (&ps.enums, &ps.one_of),
            };
            for v in enums {
                choose(v, v);
            }
            for o in one {
                choose(&o.konst, &o.title);
            }
            if ps.typ == "boolean" {
                choose("true", "Yes");
                choose("false", "No");
            }
            fields.insert(name, f);
            q.asks.push(ask);
        }
        lock(&self.amu)
            .questions
            .insert(key, Elicitation { id, fields });
        let _st = lock(&self.mu);
        self.emit(Event::Question(q));
    }

    /// Fills in the form a Question came from. No answers at all declines
    /// it.
    pub(crate) fn answer_question(
        &self,
        id: &str,
        answers: &HashMap<String, Vec<String>>,
    ) -> Result<(), Error> {
        let Some(e) = lock(&self.amu).questions.remove(id) else {
            return Err(Error::other(format!(
                "acp: no question {} waiting",
                go_quote(id)
            )));
        };
        if answers.is_empty() {
            return self
                .rpc
                .reply(&e.id, Some(&json!({"action": "decline"})), None);
        }
        let mut content = Map::new();
        for (name, got) in answers {
            let Some(f) = e.fields.get(name) else {
                continue;
            };
            if got.is_empty() {
                continue;
            }
            let vals: Vec<String> = got
                .iter()
                .map(|g| f.values.get(g).cloned().unwrap_or_else(|| g.clone()))
                .collect();
            let v = match f.typ.as_str() {
                "array" => json!(vals),
                "boolean" => Value::Bool(vals[0] == "true"),
                "number" | "integer" => {
                    let n: f64 = vals[0].trim().parse().map_err(|_| {
                        Error::other(format!(
                            "acp: {name}: strconv.ParseFloat: parsing {}: invalid syntax",
                            go_quote(&vals[0])
                        ))
                    })?;
                    go_number(n)
                }
                _ => Value::String(vals[0].clone()),
            };
            content.insert(name.clone(), v);
        }
        self.rpc.reply(
            &e.id,
            Some(&json!({"action": "accept", "content": content})),
            None,
        )
    }
}

/// A float as Go writes one: whole numbers without a point.
fn go_number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// A call to a tool that never asks, by its name or a title that starts
/// with it: "mcp__rush__spawn_agent: …".
fn trusted(names: &[String], c: &tool::Call) -> bool {
    names.iter().any(|n| {
        c.name == *n
            || c.title == *n
            || c.title
                .strip_prefix(n.as_str())
                .is_some_and(|r| r.starts_with(':'))
    })
}

fn option_kind(k: &str) -> OptionKind {
    match k {
        "allow_always" => OptionKind::AllowAlways,
        "reject_once" => OptionKind::RejectOnce,
        "reject_always" => OptionKind::RejectAlways,
        _ => OptionKind::AllowOnce,
    }
}

/// Reads or writes a file for the agent.
fn fs_call(method: &str, params: &str) -> Result<Option<Value>, RpcError> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct P {
        path: String,
        line: i64,
        limit: i64,
        content: String,
    }
    let p = match serde_json::from_str::<P>(params) {
        Ok(p) if Path::new(&p.path).is_absolute() => p,
        _ => {
            return Err(RpcError::new(
                CODE_INVALID_PARAMS,
                "an absolute path is needed",
            ));
        }
    };
    let internal = |e: std::io::Error| RpcError::new(CODE_INTERNAL, e.to_string());
    if method == "fs/write_text_file" {
        if let Some(dir) = Path::new(&p.path).parent() {
            std::fs::create_dir_all(dir).map_err(internal)?;
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&p.path)
            .map_err(internal)?;
        f.write_all(p.content.as_bytes()).map_err(internal)?;
        return Ok(None);
    }
    let b = match std::fs::read(&p.path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(RpcError::new(
                CODE_NOT_FOUND,
                format!("open {}: no such file or directory", p.path),
            ));
        }
        Err(e) => return Err(internal(e)),
    };
    let mut text = String::from_utf8_lossy(&b).into_owned();
    if p.line > 1 || p.limit > 0 {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let from = ((p.line.max(1) - 1) as usize).min(lines.len());
        let mut to = lines.len();
        if p.limit > 0 {
            to = (from + p.limit as usize).min(to);
        }
        text = lines[from..to].concat();
    }
    Ok(Some(json!({ "content": text })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_go_writes_them() {
        assert_eq!(go_number(3.0).to_string(), "3");
        assert_eq!(go_number(2.5).to_string(), "2.5");
    }

    #[test]
    fn trusted_by_name_or_title() {
        let names = vec!["mcp__rush__spawn_agent".to_string()];
        let call = |name: &str, title: &str| tool::Call {
            name: name.into(),
            title: title.into(),
            ..tool::Call::default()
        };
        assert!(trusted(&names, &call("mcp__rush__spawn_agent", "")));
        assert!(trusted(&names, &call("x", "mcp__rush__spawn_agent: go")));
        assert!(!trusted(&names, &call("x", "mcp__rush__spawn_agentx")));
    }
}
