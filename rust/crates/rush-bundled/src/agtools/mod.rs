//! rush's own MCP server, run inside a rush session's host rather than as
//! a process: Claude Code names it in initialize and sends every MCP
//! message for it over the session's control channel. Its tools are
//! things only rush can do: draw a drawing in its own frame, keep a view,
//! and hand work to another agent (agents.rs). An agent that only runs MCP
//! servers as processes runs `rush mcp-tools`: serve.
//! Ported from internal/agtools/agtools.go:1-314.

mod agents;

pub use agents::*;

use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

/// The MCP server's name; Claude Code calls its tools mcp__rush__*.
pub const SERVER: &str = "rush";

/// Starts the name Claude Code gives each of these tools.
pub const PREFIX: &str = "mcp__rush__";

/// The show tool as Claude Code names it.
pub const SHOW: &str = "mcp__rush__show";

/// What Claude sends to show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowInput {
    pub title: String,
    pub drawing: String,
}

/// What the agent sends to view.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ViewInput {
    pub name: String,
    #[serde(deserialize_with = "present")]
    pub view: Option<Box<RawValue>>,
    pub clear: bool,
}

/// One tool as tools/list says it.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

fn show_tool() -> Tool {
    Tool {
        name: "show".into(),
        description: "Show the user a drawing: an ASCII or box-drawing diagram, a flow or architecture sketch, a layout mockup, \
a tree, or any text whose exact layout matters. rush draws it in a frame of its own, full width, never wrapped \
or folded away, and the user can copy it whole. Use it instead of a code block whenever the picture is the point. \
The drawing is shown as-is: plain text, no markdown, no fences."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "A few words naming what the drawing shows." },
                "drawing": { "type": "string", "description": "The drawing, exactly as it should appear, lines separated by \\n." },
            },
            "required": ["drawing"],
            "additionalProperties": false,
        }),
        // Loaded with the built-in tools, not deferred behind tool search,
        // so Claude reaches for it without first looking it up.
        meta: Some(serde_json::json!({ "anthropic/alwaysLoad": true })),
    }
}

fn view_tool() -> Tool {
    Tool {
        name: "view".into(),
        description: "Keep a live view of your work in rush's panel beside the conversation. Use it on your own, without being \
asked, whenever the user would otherwise follow something through your messages: a list you were handed to work \
through, a long search across the code, several pull requests, a test run or build, a migration across modules, \
services you started, numbers to compare, an incident, a review's findings, research options. Make it as soon as \
that work starts, set it again (same name) as things change, and clear it once it has stopped being useful: it \
goes to the user's past views, where they can bring it back. rush also puts away a view you haven't touched in \
3 turns. Not for a single answer or a short task that ends this turn. Pick the styles that fit and mix them as \
blocks: table, kv, checklist, log, list, bars, chart (columns or line), heatmap, compare, steps, timeline, board, \
tree, hero, callout, code, text, stats, progress. Short words: the panel is about 50 columns. Colour means \
something: tone good, bad, warn, busy or quiet. The user works it too: a click opens a row to its detail and \
buttons you put on it (actions), and ticks a checklist; you hear what they changed. Run `rush view guide` for the style guide with every field, \
and `rush view example <name>` for ready ones."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The view's name, lowercase and dashes: prs, tests, search. The same name again replaces it." },
                "view": { "type": "object", "description": concat!(
                    r#"The view: {"title", "summary" (your line in rush's list, 3-6 words), "#,
                    r#""accent" (orange, blue, green, yellow, purple, red), "tone", "progress": {done, total, label}, "#,
                    r#""stats": [{value, label, tone, trend}], "blocks": [{"kind", "title", "tone", "folded", ...}], "note"}. "#,
                    r#"Blocks by kind: table {columns, items:[{cells, tone, link}]}; kv {items:[{text: key, meta: value, tone}]}; "#,
                    r#"checklist {items:[{text, done, meta}]}; log {items:[{time, text, tone}]}; list {items:[{text, meta, tone, link, progress}]}; "#,
                    r#"bars {items:[{text, value, meta}]}; chart {values, labels, style: "line" or columns}; heatmap {columns, items:[{text, values}]}; "#,
                    r#"compare {items:[{text, from, to, tone}]}; steps {items:[{text, meta, tone}]}; timeline {items:[{time, text, meta, tone}]}; "#,
                    r#"board {items:[{text: column, tone, items:[cards {text, meta, tone}]}]}; tree {items:[{text, meta, tone, items}]}; "#,
                    r#"hero {value, text, tone, trend}; callout {text, tone}; code {text}; text {text}; stats {stats}; progress {progress}. "#,
                    r#"Any item may also have "detail" (shown when a click opens it), "id", and "actions": buttons [{label, tone, "#,
                    r#"send: a message to you as the user's, open: a web link, copy: words, set: {done, tone, meta} changing that row at once}]; "#,
                    r#"the view's own "actions" sit at its foot."#,
                ) },
                "clear": { "type": "boolean", "description": "Put the view of this name away instead: it's no longer useful." },
            },
            "required": ["name"],
            "additionalProperties": false,
        }),
        meta: Some(serde_json::json!({ "anthropic/alwaysLoad": true })),
    }
}

/// rush's tools by their own names, which all never ask: show only draws,
/// view only fills rush's panel, and the agent tools start what the Agent
/// tool would.
pub fn names() -> Vec<String> {
    let mut out = vec![show_tool().name, view_tool().name];
    out.extend(agent_tools().into_iter().map(|t| t.name));
    out
}

/// What to pass --allowedTools so the tools run without asking.
pub fn allowed() -> Vec<String> {
    names()
        .into_iter()
        .map(|n| format!("{PREFIX}{n}"))
        .collect()
}

/// Answers one JSON-RPC message from an agent's MCP client.
pub type Handle = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

/// Answers one JSON-RPC message with show alone: handler(None).
pub fn handle(msg: &[u8]) -> Vec<u8> {
    answer(None, msg)
}

/// Answers one JSON-RPC message from an agent's MCP client, with the agent
/// tools too when ag runs them. A notification gets an empty result,
/// which is what Claude Code expects back.
pub fn handler(ag: Option<Arc<dyn Agents>>) -> Handle {
    Arc::new(move |msg| answer(ag.as_deref(), msg))
}

/// A member that's there, even as null, read as written; None only when
/// it's missing.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Message {
    #[serde(deserialize_with = "present")]
    id: Option<Box<RawValue>>,
    method: String,
    params: Option<Box<RawValue>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Init {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CallMeta {
    #[serde(rename = "claudecode/toolUseId")]
    tool_use_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Call {
    name: String,
    arguments: Option<Box<RawValue>>,
    #[serde(rename = "_meta")]
    meta: CallMeta,
}

fn read<T: for<'de> Deserialize<'de> + Default>(p: Option<&RawValue>) -> T {
    p.and_then(|p| rush_state::json::from_slice(p.get().as_bytes()).ok())
        .unwrap_or_default()
}

fn answer(ag: Option<&dyn Agents>, msg: &[u8]) -> Vec<u8> {
    let Ok(m) = rush_state::json::from_slice::<Message>(msg) else {
        return reply(None, None, Some((-32700, "parse error".into())));
    };
    let Some(id) = m.id.as_deref() else {
        return reply(None, Some(serde_json::json!({})), None);
    };
    let headless = ag.is_some_and(|a| a.headless());
    let ok = |v: Value| reply(Some(id), Some(v), None);
    match m.method.as_str() {
        "initialize" => {
            let p: Init = read(m.params.as_deref());
            ok(serde_json::json!({
                "protocolVersion": p.protocol_version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER, "version": "1" },
            }))
        }
        "tools/list" => {
            let mut list = if headless {
                Vec::new()
            } else {
                vec![show_tool()]
            };
            if let Some(ag) = ag {
                list.extend(agent_list(ag));
                if ag.viewer().is_some() && !headless {
                    list.push(view_tool());
                }
            }
            ok(serde_json::json!({ "tools": list }))
        }
        "tools/call" => {
            let p: Call = read(m.params.as_deref());
            if let Some(res) = call_agent(ag, &p.name, p.arguments.as_deref(), &p.meta.tool_use_id)
            {
                return ok(res);
            }
            if headless {
                return ok(result(&format!("rush has no tool named {}.", p.name), true));
            }
            if let Some(v) = ag.and_then(|a| a.viewer())
                && p.name == "view"
            {
                return ok(call_view(v, p.arguments.as_deref()));
            }
            ok(call(&p.name, p.arguments.as_deref()))
        }
        "ping" => ok(serde_json::json!({})),
        _ => reply(
            Some(id),
            None,
            Some((-32601, format!("method not found: {}", m.method))),
        ),
    }
}

/// Runs a tool. The drawing itself is the call's input, which the
/// transcript keeps, so showing it needs nothing more than a yes.
fn call(name: &str, args: Option<&RawValue>) -> Value {
    if name == "show" {
        let input: ShowInput = read(args);
        if input.drawing.trim().is_empty() {
            return result("Nothing to show: the drawing is empty.", true);
        }
        return result("Shown to the user.", false);
    }
    result(&format!("rush has no tool named {name}."), true)
}

/// Sets or clears a view.
fn call_view(v: &dyn Viewer, args: Option<&RawValue>) -> Value {
    let input: ViewInput =
        match args.map(|a| rush_state::json::from_slice::<ViewInput>(a.get().as_bytes())) {
            Some(Ok(i)) => i,
            Some(Err(e)) => return result(&format!("Its input: {e}"), true),
            None => ViewInput::default(),
        };
    if input.clear {
        if let Err(e) = v.clear_view(&input.name) {
            return result(&e, true);
        }
        return result(
            "Put away: the user can bring it back from their past views.",
            false,
        );
    }
    let Some(view) = input.view else {
        return result(
            r#"No view: {"name": "prs", "view": {"title": "Pull requests", "blocks": [...]}}, or "clear": true."#,
            true,
        );
    };
    match v.set_view(&input.name, &view) {
        Ok(said) => result(&said, false),
        Err(e) => result(&format!("{e} (rush view guide has every field)"), true),
    }
}

pub(crate) fn result(text: &str, is_err: bool) -> Value {
    crate::tool_result(text, is_err)
}

#[derive(Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// A reply as Go writes its map: keys in order, error, id, jsonrpc,
/// result.
#[derive(Serialize)]
struct Reply<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a RawValue>,
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
}

fn reply(id: Option<&RawValue>, res: Option<Value>, e: Option<(i64, String)>) -> Vec<u8> {
    let r = match e {
        Some((code, message)) => Reply {
            error: Some(RpcError { code, message }),
            id,
            jsonrpc: "2.0",
            result: None,
        },
        None => Reply {
            error: None,
            id,
            jsonrpc: "2.0",
            // Go writes a nil result as null.
            result: Some(res.unwrap_or(Value::Null)),
        },
    };
    rush_state::json::to_vec(&r).unwrap_or_default()
}

/// The server over stdio, a message a line, for an agent that runs its
/// MCP servers as processes (Codex, the ACP agents). A notification gets
/// nothing back; each request is answered as it's done, as an agent's
/// call may wait on another agent for a while.
pub fn serve(
    r: impl std::io::Read,
    w: impl Write + Send + 'static,
    handle: Handle,
) -> Result<(), String> {
    const MAX: usize = 16 << 20;
    let mut r = std::io::BufReader::with_capacity(64 << 10, r);
    let w = Arc::new(Mutex::new(w));
    let failed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut answering = Vec::new();
    let mut line = Vec::new();
    let mut scan_err = None;
    loop {
        line.clear();
        match r.by_ref().take(MAX as u64 + 1).read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                scan_err = Some(e.to_string());
                break;
            }
        }
        if line.ends_with(b"\n") {
            line.pop();
        } else if line.len() > MAX {
            scan_err = Some("bufio.Scanner: token too long".into());
            break;
        }
        if line.ends_with(b"\r") {
            line.pop();
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Id {
            #[serde(deserialize_with = "present")]
            id: Option<Box<RawValue>>,
        }
        if rush_state::json::from_slice::<Id>(&line).is_ok_and(|m| m.id.is_none()) {
            continue;
        }
        let msg = line.clone();
        let (w, failed, handle) = (w.clone(), failed.clone(), handle.clone());
        answering.push(std::thread::spawn(move || {
            let mut out = handle(&msg);
            out.push(b'\n');
            let mut w = w.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = w.write_all(&out).and_then(|_| w.flush()) {
                failed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert(e.to_string());
            }
        }));
    }
    // What was asked before the agent hung up is still answered.
    for t in answering {
        let _ = t.join();
    }
    if let Some(e) = failed.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(e);
    }
    scan_err.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(msg: &str) -> Value {
        serde_json::from_slice(&handle(msg.as_bytes())).unwrap()
    }

    // agtools_test.go TestHandshake
    #[test]
    fn handshake() {
        let init = rpc(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        );
        assert!(
            init["result"]["protocolVersion"] == "2025-11-25"
                && !init["result"]["capabilities"]["tools"].is_null(),
            "initialize: {init}"
        );
        let n = rpc(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(
            !n["result"].is_null() && n.get("id").is_none(),
            "a notification gets an empty result: {n}"
        );
        let list = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let show = &list["result"]["tools"][0];
        assert!(
            show["name"] == "show" && show["_meta"]["anthropic/alwaysLoad"] == true,
            "tools/list: {show}"
        );
        let got = allowed();
        assert!(
            got.len() == 5 && got[0] == SHOW && got[1] == format!("{PREFIX}view"),
            "allowed: {got:?}"
        );
        // Written as Go writes it: keys in order.
        assert_eq!(
            String::from_utf8(handle(br#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#)).unwrap(),
            r#"{"id":7,"jsonrpc":"2.0","result":{}}"#
        );
    }

    // agtools_test.go TestShow
    #[test]
    fn show() {
        let ok = rpc(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"show","arguments":{"drawing":"+-+\n|a|\n+-+"}}}"#,
        );
        let r = &ok["result"];
        assert!(
            r["isError"] == false && r["content"][0]["text"].as_str().unwrap().contains("Shown"),
            "show: {ok}"
        );
        let empty = rpc(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"show","arguments":{"drawing":"  "}}}"#,
        );
        assert_eq!(
            empty["result"]["isError"], true,
            "an empty drawing is an error"
        );
        let u = rpc(r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#);
        assert!(!u["error"].is_null(), "unknown method: {u}");
        assert_eq!(
            String::from_utf8(handle(b"not json")).unwrap(),
            r#"{"error":{"code":-32700,"message":"parse error"},"jsonrpc":"2.0"}"#
        );
    }

    struct Out(Arc<Mutex<Vec<u8>>>);

    impl Write for Out {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // agtools_test.go TestServe
    #[test]
    fn serves() {
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        let out = Arc::new(Mutex::new(Vec::new()));
        serve(input.as_bytes(), Out(out.clone()), handler(None)).unwrap();
        let s = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = s.trim().split('\n').collect();
        assert!(
            lines.len() == 1 && lines[0].contains(r#""show""#),
            "one answer, to the request alone: {s:?}"
        );
    }

    #[derive(Default)]
    struct Fake {
        spawned: Mutex<Option<SpawnInput>>,
        set: Mutex<Vec<(String, String)>>,
        clear: Mutex<Vec<String>>,
        views: bool,
        headless: bool,
    }

    impl Viewer for Fake {
        fn set_view(&self, name: &str, v: &RawValue) -> Result<String, String> {
            self.set.lock().unwrap().push((name.into(), v.get().into()));
            Ok("Shown".into())
        }
        fn clear_view(&self, name: &str) -> Result<(), String> {
            self.clear.lock().unwrap().push(name.into());
            Ok(())
        }
    }

    impl Agents for Fake {
        fn catalogue(&self) -> String {
            String::new()
        }
        fn spawn(&self, input: SpawnInput) -> Result<String, String> {
            *self.spawned.lock().unwrap() = Some(input);
            Ok("ok".into())
        }
        fn result(&self, _: ResultInput) -> Result<String, String> {
            Err("no".into())
        }
        fn send(&self, _: SendInput) -> Result<String, String> {
            Err("no".into())
        }
        fn viewer(&self) -> Option<&dyn Viewer> {
            self.views.then_some(self as &dyn Viewer)
        }
        fn headless(&self) -> bool {
            self.headless
        }
    }

    // agtools_test.go TestSpawnKnowsItsCall: the call Claude names in
    // _meta reaches spawn, so its child is drawn there.
    #[test]
    fn spawn_knows_its_call() {
        let f = Arc::new(Fake::default());
        handler(Some(f.clone()))(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"spawn_agent","arguments":{"agent":"a","prompt":"p"},"_meta":{"claudecode/toolUseId":"toolu_9"}}}"#,
        );
        let got = f.spawned.lock().unwrap().clone().unwrap();
        assert!(
            got.step == "toolu_9" && got.prompt == "p",
            "spawned {got:?}"
        );
        assert!(
            is_spawn("mcp__rush__spawn_agent")
                && is_spawn("rush.spawn_agent")
                && !is_spawn("spawn")
        );
        assert_eq!(
            read_spawn(br#"{"agent":"a","prompt":"p","model":"m"}"#)
                .unwrap()
                .model,
            "m"
        );
        assert!(read_spawn(br#"{"prompt":"p"}"#).is_none());
    }

    // agtools_test.go TestView: the view tool is listed only where views
    // are kept, sets one and puts one away.
    #[test]
    fn view() {
        let f = Arc::new(Fake {
            views: true,
            ..Default::default()
        });
        let h = handler(Some(f.clone()));
        let call = |msg: &str| -> Value {
            let v: Value = serde_json::from_slice(&h(msg.as_bytes())).unwrap();
            v["result"].clone()
        };
        let list =
            String::from_utf8(h(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)).unwrap();
        assert!(list.contains(r#""name":"view""#), "view isn't listed");
        let bare = String::from_utf8(handle(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#))
            .unwrap();
        assert!(
            !bare.contains(r#""name":"view""#),
            "view is listed where no views are kept"
        );
        let r = call(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"view","arguments":{"name":"prs","view":{"title":"PRs"}}}}"#,
        );
        assert!(
            r["isError"] != true && f.set.lock().unwrap()[0].1 == r#"{"title":"PRs"}"#,
            "set: {r}"
        );
        let r = call(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"view","arguments":{"name":"prs","clear":true}}}"#,
        );
        assert!(
            r["isError"] != true && f.clear.lock().unwrap().len() == 1,
            "clear: {r}"
        );
        let r = call(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"view","arguments":{"name":"prs"}}}"#,
        );
        assert_eq!(r["isError"], true, "no view wasn't refused: {r}");
    }

    // agtools_test.go TestHeadlessHasNoDrawingTools: a headless run isn't
    // given the tools that draw, and can't call them.
    #[test]
    fn headless_has_no_drawing_tools() {
        let f = Arc::new(Fake {
            views: true,
            headless: true,
            ..Default::default()
        });
        let h = handler(Some(f.clone()));
        let list =
            String::from_utf8(h(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)).unwrap();
        assert!(
            !list.contains(r#""name":"view""#) && !list.contains(r#""name":"show""#),
            "headless lists drawing tools: {list}"
        );
        assert!(
            list.contains(r#""name":"spawn_agent""#),
            "headless lost the agent tools: {list}"
        );
        let out = String::from_utf8(h(
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"view","arguments":{"name":"prs","view":{"title":"PRs"}}}}"#,
        ))
        .unwrap();
        assert!(
            out.contains(r#""isError":true"#) && f.set.lock().unwrap().is_empty(),
            "headless view call: {out}"
        );
    }
}
