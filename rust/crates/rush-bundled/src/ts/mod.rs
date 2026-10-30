//! The bundled plugin that gives every agent TypeScript code intelligence
//! from one language server per checkout: tsrs (the Rust port of tsgo)
//! where it's installed, else tsgo. Agents share it, the gate admits it,
//! and it's stopped past its memory cap or once idle. docs/typescript.md
//! has why.
//! Ported from internal/bundled/ts/plugin.go:1-241.

mod edits;
mod engine;
mod pool;
mod server;
mod tools;

pub use engine::{Engine, platform, resolve, workspace_root};
pub use pool::{Log, Options, Pool};
pub use server::{Server, file_uri, path_of};
pub use tools::{Ask, OPS, Op, Position, op_named, run_op};

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use rush_plugin::rpc::{Error, from_raw, no_method};
use rush_plugin::{Bundle, CliResult, CliRun, CliSpec, Ctx, Manifest, Requires, SettingSpec};
use serde::Deserialize;

/// The plugin's.
pub const NAME: &str = "ts";

/// Added to every rush-mode session's system prompt.
pub const PROMPT: &str = "For TypeScript and JavaScript symbols, use rush's ts tools (definition, references, hover, \
implementation, callers, callees, outline, diagnostics, find_symbol, rename, rename_file; or `rush ts <tool> --file F --line N \
--symbol S` from a shell) rather than grepping: they're type-aware and share one language server per \
checkout. Give file and line: find_symbol searches every project, which in a monorepo is slow and heavy. \
Rename and move files with rename and rename_file: they rewrite every use and import.";

/// The plugin's.
pub fn manifest() -> Manifest {
    let choice =
        |key: &str, title: &str, choices: &[&str], default: &str, description: &str| SettingSpec {
            key: key.into(),
            title: title.into(),
            kind: "choice".into(),
            choices: choices.iter().map(|c| c.to_string()).collect(),
            default: default.into(),
            description: description.into(),
        };
    Manifest {
        tools: true,
        prompt: PROMPT.into(),
        settings: vec![
            choice(
                "engine",
                "Engine",
                &["auto", "tsrs", "tsgo"],
                "auto",
                "auto runs tsrs where it's installed, else tsgo",
            ),
            choice(
                "cap",
                "Memory cap",
                &["2G", "3G", "4G", "6G", "8G", "off"],
                "4G",
                "A server holding more is stopped, and the call told to narrow its query",
            ),
            choice(
                "idle",
                "Idle",
                &["5m", "10m", "30m", "1h"],
                "10m",
                "A server asked nothing this long is closed",
            ),
        ],
        cli: cli_specs(),
        requires: Requires {
            os: vec!["darwin".into(), "linux".into()],
            ..Default::default()
        },
        ..crate::manifest(
            NAME,
            "code",
            "TypeScript code intelligence for every agent: definitions, references, hover, callers, outlines and \
             type errors from one language server per checkout, shared by all agents in it. tsrs where it's installed \
             (npm i -g @maschwenk/tsrs), else the project's tsgo. Started through the gate, stopped past its memory cap \
             or after it's idle.",
            128,
        )
    }
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), true, run)
}

fn cli_specs() -> Vec<CliSpec> {
    let mut out: Vec<CliSpec> = OPS
        .iter()
        .map(|op| {
            let mut usage = match op.needs {
                "file" => "--file F".to_string(),
                "query" => "--query Q".to_string(),
                _ => "--file F --line N (--symbol S | --character C)".to_string(),
            };
            match op.name {
                "rename" => usage.push_str(" --new-name N"),
                "rename_file" => usage.push_str(" --to PATH"),
                _ => {}
            }
            let short = op.description.split(". ").next().unwrap_or("");
            CliSpec {
                name: op.name.replace('_', "-"),
                usage,
                description: short.trim_end_matches('.').to_string(),
            }
        })
        .collect();
    out.push(CliSpec {
        name: "status".into(),
        description: "the language servers running, by checkout, and what each holds".into(),
        ..Default::default()
    });
    out
}

/// The pool's options from the settings.
fn options() -> Options {
    let v = rush_plugin::setting_values_of(&manifest());
    let get = |k: &str| v.get(k).cloned().unwrap_or_default();
    let mut o = Options {
        engine: get("engine"),
        log: Some(Arc::new(rush_events::log)),
        ..Default::default()
    };
    if o.engine.is_empty() {
        o.engine = "auto".into();
    }
    let cap = get("cap");
    if cap != "off" {
        let c = if cap.is_empty() {
            "4G".to_string()
        } else {
            cap
        };
        o.cap = crate::gatecore::parse_size(&c).unwrap_or(0);
    }
    o.idle = Duration::from_secs(10 * 60);
    if let Ok(d) = rush_events::parse_duration(&get("idle"))
        && d > 0
    {
        o.idle = Duration::from_nanos(d as u64);
    }
    o
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ToolCall {
    name: String,
    arguments: Ask,
    cwd: String,
}

/// The plugin, on its connection to the broker.
fn run(s: UnixStream) -> Result<(), String> {
    let (ctx, cancel) = Ctx::background().with_cancel();
    let pool = Pool::new(&ctx, options());
    let p = pool.clone();
    let res = crate::serve(
        s,
        Arc::new(move |_, cctx, method, params| match method {
            "initialize" => crate::empty(),
            "ui.settings" => {
                p.set_options(options());
                crate::empty()
            }
            "tools.list" => crate::answer(&serde_json::json!({ "tools": tool_list() })),
            "tools.call" => {
                let input: ToolCall = from_raw(params).map_err(Error::server)?;
                let Some(op) = op_named(&input.name) else {
                    return crate::answer(&crate::tool_result(
                        &format!("no tool {}", input.name),
                        true,
                    ));
                };
                match run_op(cctx, &p, op, &input.arguments, &input.cwd) {
                    Ok(text) => crate::answer(&crate::tool_result(&text, false)),
                    Err(e) => crate::answer(&crate::tool_result(&e, true)),
                }
            }
            "cli.run" => {
                let r: CliRun = from_raw(params).map_err(Error::server)?;
                crate::answer(&run_cli(cctx, &p, &r))
            }
            _ => Err(no_method(method)),
        }),
    );
    cancel.cancel();
    pool.close_all();
    res
}

fn tool_list() -> Vec<serde_json::Value> {
    let s = |d: &str| serde_json::json!({ "type": "string", "description": d });
    let n = |d: &str| serde_json::json!({ "type": "integer", "minimum": 1, "description": d });
    OPS.iter()
        .map(|op| {
            let mut props = serde_json::Map::new();
            let mut req: Vec<&str> = Vec::new();
            match op.needs {
                "position" => {
                    props.insert("file".into(), s("The file, absolute or from your folder"));
                    props.insert("line".into(), n("The line, from 1"));
                    props.insert("symbol".into(), s("The name on that line to ask about (or give character)"));
                    props.insert("character".into(), n("The column, from 1 (or give symbol)"));
                    req = vec!["file", "line"];
                }
                "file" => {
                    props.insert("file".into(), s("The file, absolute or from your folder"));
                    req = vec!["file"];
                }
                "query" => {
                    props.insert("query".into(), s("The name, or part of it"));
                    props.insert(
                        "file".into(),
                        s("Any file in the checkout to search, when it isn't your folder's"),
                    );
                    req = vec!["query"];
                }
                _ => {}
            }
            match op.name {
                "rename" => {
                    props.insert("new_name".into(), s("The new name"));
                    req.push("new_name");
                }
                "rename_file" => {
                    props.insert("to".into(), s("The new path, absolute or from your folder"));
                    req.push("to");
                }
                _ => {}
            }
            serde_json::json!({
                "name": op.name,
                "description": op.description,
                "inputSchema": {
                    "type": "object", "properties": props, "required": req, "additionalProperties": false,
                },
            })
        })
        .collect()
}

/// `rush ts <tool> …`, for agents with only a shell.
fn run_cli(ctx: &Ctx, pool: &Pool, r: &CliRun) -> CliResult {
    if r.command == "status" {
        return CliResult {
            stdout: status(pool),
            ..Default::default()
        };
    }
    let Some(op) = op_named(&r.command.replace('-', "_")) else {
        return CliResult {
            stderr: format!("rush ts: no command {}\n", r.command),
            exit: 2,
            ..Default::default()
        };
    };
    let mut a = Ask::default();
    let mut i = 0;
    while i < r.args.len() {
        let (k, v) = match r.args[i].split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None if i + 1 < r.args.len() => {
                i += 1;
                (r.args[i - 1].clone(), r.args[i].clone())
            }
            None => (r.args[i].clone(), String::new()),
        };
        let num = |v: &str| v.parse::<i64>().unwrap_or(0);
        match k.as_str() {
            "--file" | "-f" => a.file = v,
            "--line" | "-l" => a.line = num(&v),
            "--character" | "-c" => a.character = num(&v),
            "--symbol" | "-s" => a.symbol = v,
            "--query" | "-q" => a.query = v,
            "--new-name" => a.new_name = v,
            "--to" => a.to = v,
            _ => {
                return CliResult {
                    stderr: format!("rush ts: unknown {k}\n"),
                    exit: 2,
                    ..Default::default()
                };
            }
        }
        i += 1;
    }
    match run_op(ctx, pool, op, &a, &r.cwd) {
        Ok(text) => CliResult {
            stdout: format!("{text}\n"),
            ..Default::default()
        },
        Err(e) => CliResult {
            stderr: format!("rush ts: {e}\n"),
            exit: 1,
            ..Default::default()
        },
    }
}

fn status(pool: &Pool) -> String {
    let ss = pool.servers();
    if ss.is_empty() {
        return "no language servers running\n".into();
    }
    let mut b = String::new();
    for (root, s) in ss {
        let idle = s.idle();
        let secs = (idle.as_millis() + 500) / 1000; // Round(time.Second)
        b.push_str(&format!(
            "{root}  {} (pid {})  {}  idle {}\n",
            s.engine.name,
            s.pid(),
            crate::gatecore::size(s.footprint()),
            crate::go_duration(secs as i64 * 1_000_000_000)
        ));
    }
    b
}

#[cfg(test)]
mod tests;
