//! The bundled plugins as rush runs them: the Rust broker starts each as
//! `<exe> plugin run <name>`, this test's own binary standing in for rush,
//! and the ts plugin's pool drives a language server, this binary again,
//! as `<exe> fake-lsp --lsp --stdio`. After internal/plugind/bundled_test.go
//! and internal/bundled/ts/ts_test.go.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use rush_bundled::ts::{Ask, Options, Pool, op_named, platform, run_op};
use rush_plugin::rpc::from_raw;
use rush_plugin::{CleanupReport, CliRun, Ctx, SERVICE_CLEANUP_REPORT};
use rush_plugind::{Broker, FileHosts, Log, Scope};
use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "plugin" && args[2] == "run" {
        rush_bundled::register();
        if let Err(e) = rush_plugin::run_bundled(&args[3]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    if args.get(1).is_some_and(|a| a == "fake-lsp") {
        fake_lsp();
        std::process::exit(0);
    }
    if args.iter().skip(1).any(|a| a == "--list") {
        println!("broker_runs_the_bundled_plugins: test");
        println!("ts_over_a_language_server: test");
        return;
    }
    broker_runs_the_bundled_plugins();
    println!("test broker_runs_the_bundled_plugins ... ok");
    ts_over_a_language_server();
    println!("test ts_over_a_language_server ... ok");
}

fn broker_runs_the_bundled_plugins() {
    let home = tempfile::Builder::new()
        .prefix("rbd-")
        .tempdir_in("/tmp")
        .unwrap();
    rush_state::set_thread_dir(Some(home.path().canonicalize().unwrap()));
    rush_bundled::register();
    // On until turned off: drafts, queue and yap; the rest wait to be
    // turned on.
    let on = rush_plugin::enabled();
    for n in ["drafts", "queue", "yap"] {
        assert!(on.contains_key(n), "{n} isn't on: {:?}", on.keys());
    }
    for n in [
        "gate",
        "ts",
        "git-clean",
        "hex",
        "watch",
        "drive",
        "search-guard",
        "cleanup",
    ] {
        assert!(!on.contains_key(n), "{n} is on before it's turned on");
    }
    rush_plugin::set_bundled("git-clean", true).unwrap();
    let logger = Arc::new(rush_events::Logger::new(Some(
        home.path().join("events.jsonl"),
    )));
    let b = Broker::new(
        Arc::new(FileHosts::logging_to(logger.clone())),
        Log::discard(),
        Scope::with_events(logger),
    );
    b.reload();
    for n in ["drafts", "queue", "yap", "git-clean"] {
        b.wait_status(n, Duration::from_secs(20), |s| s.state == "running")
            .unwrap_or_else(|| panic!("{n} not running: {:?}", b.status()));
    }
    let ctx = Ctx::background().with_timeout(Duration::from_secs(20));
    // queue's CLI runs in it: a bad call says how to make a good one, and
    // a good one reaches the broker, which knows no such session.
    let run = |command: &str, args: &[&str]| CliRun {
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let res = b.cli(&ctx, "queue", run("send", &["a1"])).unwrap();
    assert!(
        res.exit == 2
            && res
                .stderr
                .contains("usage: rush queue send <session> <n> [--was TEXT]"),
        "{res:?}"
    );
    let res = b
        .cli(&ctx, "queue", run("send", &["nobody-here", "0"]))
        .unwrap();
    assert_eq!(res.exit, 1, "{res:?}");
    // git-clean answers the cleanup report.
    let answers = b.ask(&ctx, SERVICE_CLEANUP_REPORT, "");
    let git = answers
        .iter()
        .find(|a| a.plugin == "git-clean")
        .unwrap_or_else(|| panic!("no git-clean answer: {answers:?}"));
    let rep: CleanupReport = from_raw(git.result.as_deref()).unwrap();
    assert!(
        rep.title == "git-clean" && !rep.report.is_empty() && rep.rules.contains("git-clean"),
        "{rep:?}"
    );
    // Turned off, it stops.
    rush_plugin::set_bundled("git-clean", false).unwrap();
    b.reload();
    assert!(
        b.status().iter().all(|s| s.name != "git-clean"),
        "turned off, it should stop"
    );
    b.stop();
    rush_state::set_thread_dir(None);
}

fn ts_over_a_language_server() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    // The project's own tsrs, as npm hoists it: a script that runs this
    // binary as the server.
    let bin = root
        .join("node_modules")
        .join("@maschwenk")
        .join(format!("tsrs-{}", platform()))
        .join("tsrs");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    let exe = std::env::current_exe().unwrap();
    std::fs::write(
        &bin,
        format!("#!/bin/sh\nexec '{}' fake-lsp \"$@\"\n", exe.display()),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(root.join("tsconfig.json"), "{}").unwrap();
    let a = root.join("a.ts");
    std::fs::write(&a, "export const thing = 1;\nconst s = \"😀\"; thing;\n").unwrap();
    let (a, r) = (
        a.to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
    );

    let ctx = Ctx::background();
    let logged = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let lg = logged.clone();
    let p = Pool::new(
        &ctx,
        Options {
            engine: "auto".into(),
            log: Some(Arc::new(move |kind, _| {
                lg.lock().unwrap().push(kind.into())
            })),
            ..Default::default()
        },
    );
    let ask = |line, symbol: &str| Ask {
        file: "a.ts".into(),
        line,
        symbol: symbol.into(),
        ..Default::default()
    };
    // The server asked for its settings before it answered initialize,
    // and got a null for each.
    let out = run_op(&ctx, &p, op_named("hover").unwrap(), &ask(2, "thing"), &r).unwrap();
    assert_eq!(out, "const thing: 1 (settings: [null,null]) at 2:16");
    let out = run_op(
        &ctx,
        &p,
        op_named("definition").unwrap(),
        &ask(2, "thing"),
        &r,
    )
    .unwrap();
    assert_eq!(out, "a.ts:1:14: export const thing = 1;");
    let out = run_op(&ctx, &p, op_named("outline").unwrap(), &ask(0, ""), &r).unwrap();
    assert_eq!(out, "variable thing  1-1\n  constant inner  1-1");
    let out = run_op(&ctx, &p, op_named("diagnostics").unwrap(), &ask(0, ""), &r).unwrap();
    assert_eq!(out, "a.ts:2:1: error TS2322: bad thing");
    let out = run_op(
        &ctx,
        &p,
        op_named("rename").unwrap(),
        &Ask {
            new_name: "item".into(),
            ..ask(1, "thing")
        },
        &r,
    )
    .unwrap();
    assert_eq!(out, "renamed to item in 1 files:\na.ts");
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "export const item = 1;\nconst s = \"😀\"; item;\n"
    );
    // Moving a file rewrites what imports it, then moves it.
    let b = root.join("b.ts");
    std::fs::write(&b, "import { item } from \"./a.js\";\n").unwrap();
    let out = run_op(
        &ctx,
        &p,
        op_named("rename_file").unwrap(),
        &Ask {
            file: "a.ts".into(),
            to: "lib/c.ts".into(),
            ..Default::default()
        },
        &r,
    )
    .unwrap();
    assert_eq!(
        out,
        "moved a.ts → lib/c.ts; imports rewritten in 1 files:\nb.ts"
    );
    assert!(root.join("lib").join("c.ts").exists() && !root.join("a.ts").exists());
    assert_eq!(
        std::fs::read_to_string(&b).unwrap(),
        "import { item } from \"./lib/c.js\";\n"
    );
    let a = root.join("lib").join("c.ts").to_string_lossy().into_owned();
    // A call it can't answer says why, by the server's name.
    let err = run_op(
        &ctx,
        &p,
        op_named("callers").unwrap(),
        &Ask {
            file: a.clone(),
            line: 1,
            symbol: "item".into(),
            ..Default::default()
        },
        &r,
    )
    .unwrap_err();
    assert_eq!(err, "tsrs: no call hierarchy here");
    let servers = p.servers();
    let s = servers
        .get(&r)
        .expect("one server for the checkout")
        .clone();
    assert!(s.footprint() > 0);
    // Past its cap it's stopped, and a call in flight is told why; the
    // next call gets a fresh server.
    p.set_options(Options {
        engine: "auto".into(),
        cap: 1,
        ..Default::default()
    });
    p.look();
    assert!(
        s.wait_done(Duration::from_secs(10)),
        "not stopped past its cap"
    );
    let why = s.why().unwrap();
    assert!(
        why.contains("language server for") && why.contains("past its"),
        "{why}"
    );
    p.set_options(Options {
        engine: "auto".into(),
        ..Default::default()
    });
    let again = p.server_for(&ctx, &a).unwrap();
    assert!(!Arc::ptr_eq(&again, &s), "no fresh server");
    let _ = a.as_str();
    assert!(logged.lock().unwrap().iter().any(|k| k == "ts-started"));
    p.close_all();
    assert!(
        again.wait_done(Duration::from_secs(10)),
        "closed, it should exit"
    );
    assert!(p.servers().is_empty());
}

/// A language server that knows one file: enough LSP for the tools.
fn fake_lsp() {
    let mut r = BufReader::new(std::io::stdin().lock());
    let mut w = std::io::stdout().lock();
    let send = |w: &mut dyn Write, v: Value| {
        let b = serde_json::to_vec(&v).unwrap();
        write!(w, "Content-Length: {}\r\n\r\n", b.len()).unwrap();
        w.write_all(&b).unwrap();
        w.flush().unwrap();
    };
    let mut texts: HashMap<String, String> = HashMap::new();
    let mut settings = String::from("none");
    let mut init_id = None;
    loop {
        let mut n = 0usize;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let l = line.trim();
            if l.is_empty() {
                break;
            }
            if let Some(v) = l.strip_prefix("Content-Length:") {
                n = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; n];
        r.read_exact(&mut body).unwrap();
        let m: Value = serde_json::from_slice(&body).unwrap();
        let method = m["method"].as_str().unwrap_or("");
        let id = m.get("id").cloned();
        let p = &m["params"];
        let reply = |w: &mut dyn Write, result: Value| {
            send(w, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        };
        match method {
            "initialize" => {
                // Ask for settings first, as tsgo does, and answer once
                // they come.
                init_id = id.clone();
                send(
                    &mut w,
                    json!({ "jsonrpc": "2.0", "id": 900, "method": "workspace/configuration",
                    "params": { "items": [{}, {}] } }),
                );
            }
            "" if m["id"] == 900 => {
                settings = m["result"].to_string();
                send(
                    &mut w,
                    json!({ "jsonrpc": "2.0", "id": init_id, "result": { "capabilities": { "hoverProvider": true } } }),
                );
            }
            "textDocument/didOpen" | "textDocument/didChange" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap().to_string();
                let text = p["textDocument"]["text"]
                    .as_str()
                    .or(p["contentChanges"][0]["text"].as_str())
                    .unwrap()
                    .to_string();
                texts.insert(uri.clone(), text);
                send(
                    &mut w,
                    json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
                    "uri": uri, "diagnostics": [{ "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 5 } },
                        "severity": 1, "message": "bad\nthing", "code": 2322 }] } }),
                );
            }
            "textDocument/hover" => {
                let pos = &p["position"];
                reply(
                    &mut w,
                    json!({ "contents": { "kind": "markdown",
                    "value": format!(" const thing: 1 (settings: {settings}) at {}:{} ", pos["line"].as_i64().unwrap() + 1, pos["character"]) } }),
                );
            }
            "textDocument/definition" => {
                let uri = p["textDocument"]["uri"].clone();
                reply(
                    &mut w,
                    json!([{ "uri": uri, "range": { "start": { "line": 0, "character": 13 }, "end": { "line": 0, "character": 18 } } }]),
                );
            }
            "textDocument/documentSymbol" => {
                let r1 = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 22 } });
                reply(
                    &mut w,
                    json!([{ "name": "thing", "kind": 13, "range": r1, "selectionRange": r1,
                    "children": [{ "name": "inner", "kind": 14, "range": r1, "selectionRange": r1 }] }]),
                );
            }
            "textDocument/rename" => {
                // Every whole "thing" in the file, by UTF-16 column.
                let uri = p["textDocument"]["uri"].as_str().unwrap().to_string();
                let new = p["newName"].as_str().unwrap();
                let mut edits = Vec::new();
                for (i, line) in texts[&uri].split('\n').enumerate() {
                    for (at, _) in line.match_indices("thing") {
                        let col = line[..at].encode_utf16().count();
                        edits.push(json!({ "range": { "start": { "line": i, "character": col },
                            "end": { "line": i, "character": col + 5 } }, "newText": new }));
                    }
                }
                reply(&mut w, json!({ "changes": { uri: edits } }));
            }
            "workspace/willRenameFiles" => {
                // b.ts imports a.ts: point it at where it's going.
                let to = p["files"][0]["newUri"].as_str().unwrap();
                let b = to.rsplit_once("/lib/").unwrap().0.to_string() + "/b.ts";
                let line = "import { item } from \"./a.js\";";
                let at = line.find("./a.js").unwrap();
                reply(
                    &mut w,
                    json!({ "documentChanges": [{ "textDocument": { "uri": b, "version": null },
                    "edits": [{ "range": { "start": { "line": 0, "character": at }, "end": { "line": 0, "character": at + 6 } },
                        "newText": "./lib/c.js" }] }] }),
                );
            }
            "shutdown" => reply(&mut w, Value::Null),
            "exit" => return,
            _ if id.is_some() => {
                let msg = if method == "textDocument/prepareCallHierarchy" {
                    "no call hierarchy here".to_string()
                } else {
                    format!("no {method}")
                };
                send(
                    &mut w,
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": msg } }),
                );
            }
            _ => {}
        }
    }
}
