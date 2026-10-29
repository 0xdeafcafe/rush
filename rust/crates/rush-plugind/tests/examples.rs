//! The broker running the example plugins in the repository, built from
//! their Go source (skipped without a Go toolchain) and sandboxed, talked
//! to as a session host would: Claude Code's MCP messages in, the
//! plugin's replies out.
//! Ported from internal/plugind/plugind_test.go and kanban_test.go.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rush_plugin::Manifest;
use rush_plugind::{Broker, FileHosts, Info, Log, Scope};
use rush_sys::fswait::{File, Stop, grown};
use serde_json::value::RawValue;
use serde_json::{Value, json};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

/// A rush folder for this test's thread: short, under /tmp, as a unix
/// socket's path must be.
struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Home {
        let d = tempfile::Builder::new()
            .prefix("rush-")
            .tempdir_in("/tmp")
            .unwrap();
        rush_state::set_thread_dir(Some(d.path().canonicalize().unwrap()));
        Home(d)
    }
    fn path(&self) -> PathBuf {
        self.0.path().canonicalize().unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
    }
}

/// Builds a Go program; false (and says why) without Go to build it with.
fn go_build(pkg: &Path, out: &Path) -> bool {
    match Command::new("go")
        .arg("build")
        .arg("-o")
        .arg(out)
        .arg(".")
        .current_dir(pkg)
        .output()
    {
        Ok(o) if o.status.success() => true,
        Ok(o) => panic!("go build: {}", String::from_utf8_lossy(&o.stderr)),
        Err(e) => {
            eprintln!("skipped: no Go toolchain ({e})");
            false
        }
    }
}

/// Runs a broker in home until nothing is approved; the broker, once it
/// listens, and what Run returned, when it does.
fn run_broker(home: &Home) -> (Broker, mpsc::Receiver<Result<(), String>>) {
    let logger = Arc::new(rush_events::Logger::new(Some(
        home.path().join("events.jsonl"),
    )));
    let scope = Scope::with_events(logger.clone());
    let (ready_tx, ready) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    std::thread::spawn(move || {
        let res = rush_plugind::run_with(
            Arc::new(FileHosts::logging_to(logger)),
            Log::discard(),
            scope,
            false,
            move |b| {
                let _ = ready_tx.send(b.clone());
            },
        );
        let _ = done_tx.send(res);
    });
    match ready.recv_timeout(Duration::from_secs(10)) {
        Ok(b) => (b, done),
        Err(_) => panic!("broker never listened: {:?}", done.try_recv()),
    }
}

/// Waits for the file at path to hold want, as it's written.
fn wait_file(path: &Path, want: &str) -> String {
    let stop = Arc::new(Stop::new().unwrap());
    let s = stop.clone();
    // A timer, not a look: it only ever ends the wait.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        s.close();
    });
    loop {
        let b = std::fs::read_to_string(path).unwrap_or_default();
        if b.contains(want) {
            stop.close();
            return b;
        }
        let f = File {
            path: path.to_path_buf(),
            size: b.len() as u64,
        };
        if !grown(&stop, &[f]) {
            panic!("{} never held {want:?}; it has {b:?}", path.display());
        }
    }
}

fn mcp(b: &rush_plugin::Broker, plugin: &str, session: &str, msg: &str) -> Value {
    let m = RawValue::from_string(msg.to_string()).unwrap();
    serde_json::from_str(b.mcp(plugin, session, &m).get()).unwrap()
}

fn text(r: &Value) -> String {
    r["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Builds plugins/examples/delegate, approves it, and talks to it as a
/// session host would.
#[test]
#[cfg(target_os = "macos")]
fn broker_runs_the_example_plugin() {
    if rush_plugin::supported().is_err() {
        return;
    }
    let home = Home::new();
    let dir = rush_plugin::root().join("delegate");
    std::fs::create_dir_all(&dir).unwrap();
    let src = repo().join("plugins/examples/delegate");
    if !go_build(&src, &dir.join("delegate")) {
        return;
    }
    std::fs::copy(src.join("plugin.json"), dir.join("plugin.json")).unwrap();
    let p = rush_plugin::load(&dir).unwrap();
    rush_plugin::approve(&p).unwrap();
    let (broker, done) = run_broker(&home);

    let b = rush_plugin::Broker::new();
    let init = mcp(
        &b,
        "delegate",
        "sess1",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
    );
    assert_eq!(
        init["result"]["serverInfo"]["name"], "rush-delegate",
        "{init}"
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let n = mcp(
        &b,
        "delegate",
        "sess1",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert!(!n["result"].is_null(), "notification reply = {n}");
    let list = mcp(
        &b,
        "delegate",
        "sess1",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    assert_eq!(
        list["result"]["tools"].as_array().map(Vec::len),
        Some(4),
        "{list}"
    );
    let call = |id: u32, tool: &str, args: Value| {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool, "arguments": args}});
        text(&mcp(&b, "delegate", "sess1", &msg.to_string()))
    };
    assert_eq!(call(3, "list_agents", json!({})), "No agents.");
    // Sessions it didn't start are out of its reach, and ids can't be paths.
    for id in ["someone-else", "../../plugin-data/delegate"] {
        let got = call(4, "message_agent", json!({"id": id, "text": "rm -rf ~"}));
        assert!(
            got.contains("no session") || got.contains("bad session id"),
            "message_agent({id}) = {got:?}"
        );
    }
    // Outside its workspaces, nothing starts.
    let got = call(5, "start_agent", json!({"cwd": "/tmp", "prompt": "hi"}));
    assert!(
        got.contains("outside the plugin's workspaces"),
        "start_agent in /tmp = {got:?}"
    );

    // It is started again when it dies.
    let st = rush_plugind::ask().unwrap();
    assert!(st.len() == 1 && st[0].state == "running", "status = {st:?}");
    let pid = st[0].pid;
    // SAFETY: kills the plugin the broker started for this test.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    let st = broker.wait_status("delegate", Duration::from_secs(10), |s| {
        s.state == "running" && s.pid != 0 && s.pid != pid && s.restarts == 1
    });
    assert!(st.is_some(), "not restarted: {:?}", broker.status());
    assert_eq!(
        call(6, "list_agents", json!({})),
        "No agents.",
        "after restart"
    );

    // Revoking the last plugin stops it, and the broker with it.
    rush_plugin::revoke("delegate").unwrap();
    rush_plugind::reload().unwrap();
    let res = done
        .recv_timeout(Duration::from_secs(10))
        .expect("broker still running with nothing approved");
    res.unwrap();
    assert!(
        !rush_plugin::broker_sock().exists(),
        "broker left its socket behind"
    );
}

/// Runs plugins/examples/kanban, sandboxed, against a kanban-code board
/// and CLI of its own: Claude reads the board and its card, the plugin has
/// kanban-code link the card to the agent it started for it, and a message
/// for the card reaches that agent.
#[test]
#[cfg(target_os = "macos")]
fn broker_runs_the_kanban_example() {
    if rush_plugin::supported().is_err() {
        return;
    }
    let home = Home::new();
    let hp = home.path().to_string_lossy().into_owned();
    // kanban-code: a board with one card, and a CLI that notes what it's
    // asked.
    let kb = home.path().join("kb");
    std::fs::create_dir_all(&kb).unwrap();
    std::fs::write(
        kb.join("links.json"),
        format!(
            r#"{{"links": [{{"id": "card_bbb222", "column": "backlog",
            "projectPath": "{hp}", "sessionLink": {{"sessionId": "s-9"}}, "issueLink": {{"number": 12, "title": "Dark mode", "body": "Please."}},
            "manualOverrides": {{}}, "manuallyArchived": false, "source": "githubIssue", "isRemote": false,
            "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-01T00:00:00Z"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(kb.join("calls"), "").unwrap();
    let cli = home.path().join("bin/kanban");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}/calls\necho '{{}}'\n",
                kb.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    // An agent the plugin started for the card, as its host publishes it.
    let info = Info {
        id: "k1".into(),
        session_id: "s-9".into(),
        cwd: hp.clone(),
        state: "idle".into(),
        host_pid: std::process::id() as i64,
        started_by: "kanban".into(),
        meta: [("card".to_string(), "card_bbb222".to_string())].into(),
        ..Default::default()
    };
    let sd = rush_plugind::host::root().join("k1");
    std::fs::create_dir_all(&sd).unwrap();
    std::fs::write(sd.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();

    let dir = rush_plugin::root().join("kanban");
    std::fs::create_dir_all(&dir).unwrap();
    if !go_build(&repo().join("plugins/examples/kanban"), &dir.join("kanban")) {
        return;
    }
    let m = Manifest {
        name: "kanban".into(),
        command: vec!["kanban".into()],
        tools: true,
        sidebar: true,
        sessions: vec!["list".into(), "start".into(), "queue".into()],
        workspaces: vec![hp.clone()],
        read: vec![kb.to_string_lossy().into_owned()],
        env: [(
            "KANBAN_CODE_HOME".to_string(),
            kb.to_string_lossy().into_owned(),
        )]
        .into(),
        exec: [(
            "kanban".to_string(),
            vec![cli.to_string_lossy().into_owned()],
        )]
        .into(),
        ..Default::default()
    };
    std::fs::write(
        dir.join("plugin.json"),
        rush_state::json::to_vec(&m).unwrap(),
    )
    .unwrap();
    let p = rush_plugin::load(&dir).unwrap();
    rush_plugin::approve(&p).unwrap();
    let (_broker, done) = run_broker(&home);

    let b = rush_plugin::Broker::new();
    let call = |tool: &str, args: &str| {
        let msg = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{args}}}}}"#
        );
        let r = mcp(&b, "kanban", "k1", &msg);
        let t = text(&r);
        assert!(!t.is_empty(), "{tool}: {r}");
        t
    };
    let got = call("board", "{}");
    assert!(
        got.contains("## Backlog\n- card_bbb222  Dark mode"),
        "board = {got:?}"
    );
    // The card comes from the meta rush hands along with the call.
    let got = call("my_card", "{}");
    assert!(got.contains("Issue #12: Dark mode"), "my_card = {got:?}");
    let got = call("start_card", r#"{"card": "card_bbb222"}"#);
    assert!(
        got.contains("already being worked on"),
        "start_card on a card with an agent = {got:?}"
    );
    // Reaches the agent: it isn't really running, so rush says so.
    let got = call("card_message", r#"{"card": "dark mode", "text": "hi"}"#);
    assert!(got.contains("k1 is not running"), "card_message = {got:?}");

    // Watching, it saw the agent's conversation, and linked the card to it.
    wait_file(&kb.join("calls"), "relink card_bbb222 s-9 --json");
    // It arranged rush's list as the board.
    wait_file(
        &rush_plugin::sidebar::sidebar_path("kanban"),
        r#""s-9":{"name":"Dark mode","section":"Backlog""#,
    );

    rush_plugin::revoke("kanban").unwrap();
    let _ = rush_plugind::reload();
    done.recv_timeout(Duration::from_secs(10))
        .expect("broker still running with nothing approved")
        .unwrap();
    assert!(
        !rush_plugin::sidebar::sidebar_path("kanban").exists(),
        "revoked, its sidebar stayed"
    );
}

/// An MCP server runs as a plugin as it is: initialized as any MCP client
/// would, its own guidance passed on, its tools called through.
#[test]
#[cfg(target_os = "macos")]
fn broker_runs_an_mcp_plugin() {
    if rush_plugin::supported().is_err() {
        return;
    }
    let home = Home::new();
    let dir = rush_plugin::root().join("echo");
    std::fs::create_dir_all(&dir).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/mcpecho");
    if !go_build(&src, &dir.join("echo")) {
        return;
    }
    let m = Manifest {
        name: "echo".into(),
        protocol: rush_plugin::PROTO_MCP.into(),
        command: vec!["echo".into()],
        ..Default::default()
    };
    std::fs::write(
        dir.join("plugin.json"),
        rush_state::json::to_vec(&m).unwrap(),
    )
    .unwrap();
    rush_plugin::approve(&rush_plugin::load(&dir).unwrap()).unwrap();
    let (broker, done) = run_broker(&home);
    broker
        .wait_status("echo", Duration::from_secs(10), |s| s.state == "running")
        .unwrap_or_else(|| panic!("not running: {:?}", broker.status()));

    let b = rush_plugin::Broker::new();
    let init = mcp(
        &b,
        "echo",
        "s1",
        r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
    );
    assert_eq!(
        init,
        json!({"id": "a", "jsonrpc": "2.0", "result": {"capabilities": {"tools": {}},
            "instructions": "Echo things.", "protocolVersion": "2025-03-26",
            "serverInfo": {"name": "rush-echo", "version": ""}}})
    );
    let list = mcp(
        &b,
        "echo",
        "s1",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    assert_eq!(list["result"]["tools"][0]["name"], "echo", "{list}");
    let got = mcp(
        &b,
        "echo",
        "s1",
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}"#,
    );
    assert_eq!(text(&got), "echo: hi", "{got}");
    let got = mcp(
        &b,
        "echo",
        "s1",
        r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#,
    );
    assert_eq!(got["error"]["code"], -32601, "{got}");

    rush_plugin::revoke("echo").unwrap();
    rush_plugind::reload().unwrap();
    done.recv_timeout(Duration::from_secs(10))
        .expect("broker still running with nothing approved")
        .unwrap();
}
