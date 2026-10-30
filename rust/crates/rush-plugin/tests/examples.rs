//! rush-plugin against the plugins in the repository: their manifests
//! load as Go rush loads them, and the Go programs (built here with the Go
//! toolchain, skipped without one) talk to Rust rush over the wire
//! protocol, from inside the sandbox Rust rush puts them in.
//! Ported from internal/plugin/category_test.go:24-37 (TestRepoPluginsLoad)
//! and sandbox_darwin_test.go:14-110 (TestSandboxHolds).

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use rush_plugin::rpc::{Handler, no_method, to_raw};
use rush_plugin::sandbox::with_ipc_fd;
use rush_plugin::{Conn, Ctx, Launch, Proxy, load};
use serde_json::{Value, json};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

/// A rush folder of its own for this test's thread, resolved, as the
/// sandbox is keyed on resolved paths.
struct Home(tempfile::TempDir);

impl Home {
    fn new() -> Home {
        let d = tempfile::tempdir().unwrap();
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

/// Builds a Go program: go build -o out pkg, in dir. False (and says why)
/// when there's no Go to build it with.
fn go_build(dir: &Path, pkg: &str, out: &Path) -> bool {
    match Command::new("go")
        .arg("build")
        .arg("-o")
        .arg(out)
        .arg(pkg)
        .current_dir(dir)
        .output()
    {
        Ok(o) if o.status.success() => true,
        Ok(o) => panic!("go build {pkg}: {}", String::from_utf8_lossy(&o.stderr)),
        Err(e) => {
            eprintln!("skipped: no Go toolchain to build {pkg} ({e})");
            false
        }
    }
}

/// Installs the example plugin name into the test's rush folder: its
/// manifest, and its program built from source.
fn install_example(name: &str, build_in: &Path, pkg: &str) -> Option<PathBuf> {
    let dir = rush_plugin::root().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        repo()
            .join("plugins/examples")
            .join(name)
            .join("plugin.json"),
        dir.join("plugin.json"),
    )
    .unwrap();
    go_build(build_in, pkg, &dir.join(name)).then_some(dir)
}

/// Starts the plugin in dir as the broker does, sandboxed, with conn on
/// the other end of its fd 3.
fn start(dir: &Path, proxy_port: u16, h: Handler) -> (std::process::Child, Conn) {
    let p = load(dir).unwrap();
    let (mine, theirs) = UnixStream::pair().unwrap();
    let mut cmd = Launch {
        plugin: p,
        proxy_port,
    }
    .command()
    .unwrap();
    with_ipc_fd(&mut cmd, theirs.as_raw_fd());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let child = cmd.spawn().unwrap();
    drop(theirs);
    (child, Conn::new(mine, Some(h)).unwrap())
}

fn stop(mut child: std::process::Child, conn: Conn) {
    conn.close();
    let _ = child.kill();
    let _ = child.wait();
}

/// The plugins in the repository load, each under a category.
#[test]
fn repo_plugins_load() {
    let mut n = 0;
    for e in std::fs::read_dir(repo().join("plugins/examples"))
        .unwrap()
        .flatten()
    {
        let dir = e.path();
        if !dir.join("plugin.json").exists() {
            continue;
        }
        let p = load(&dir).unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
        assert!(!p.category.is_empty(), "{} names no category", p.name);
        n += 1;
    }
    assert!(n >= 6, "found {n} example plugins");
}

/// neighbours, built from the Go source with Go rush's own plugin package,
/// answers rush's calls and calls rush back, sandboxed.
#[test]
#[cfg(target_os = "macos")]
fn neighbours_speaks_the_protocol() {
    let _h = Home::new();
    let Some(dir) = install_example(
        "neighbours",
        &repo().join("plugins/examples"),
        "./neighbours",
    ) else {
        return;
    };
    let h: Handler = Arc::new(|_, _, method, _| match method {
        "sessions.list" => to_raw(&json!([
            {"id": "me", "name": "Me", "repo": "/r", "state": "running"},
            {"id": "x", "name": "Other", "repo": "/r/.claude/worktrees/w", "branch": "feat",
             "state": "running", "worktree": true, "detail": "testing"},
            {"id": "y", "name": "Gone", "repo": "/r", "state": "stopped"},
        ]))
        .map(Some),
        m => Err(no_method(m)),
    });
    let (child, conn) = start(&dir, 0, h);
    let ctx = Ctx::background().with_timeout(Duration::from_secs(20));
    let init: Value = conn
        .call(&ctx, "initialize", &json!({"settings": {}}))
        .unwrap();
    assert_eq!(init, json!({}));
    let tools: Value = conn.call(&ctx, "tools.list", &()).unwrap();
    assert_eq!(tools["tools"][0]["name"], "neighbours");
    let res: Value = conn
        .call(
            &ctx,
            "tools.call",
            &json!({"session": "me", "cwd": "/r", "name": "neighbours", "arguments": {}}),
        )
        .unwrap();
    assert_eq!(res["isError"], false);
    assert_eq!(
        res["content"][0]["text"],
        "- Other (x) on feat, running in its own worktree: testing\n"
    );
    // A method it doesn't know is Go's "method not found".
    let e = conn.call::<_, Value>(&ctx, "frob", &()).unwrap_err();
    assert_eq!(
        e.rpc().map(|e| e.code),
        Some(rush_plugin::rpc::CODE_NO_METHOD)
    );
    stop(child, conn);
}

/// reconnect, built from its own module with an RPC of its own written to
/// the protocol's description, speaks it with Rust rush too.
#[test]
#[cfg(target_os = "macos")]
fn reconnect_speaks_the_protocol() {
    let _h = Home::new();
    let src = repo().join("plugins/examples/reconnect");
    let Some(dir) = install_example("reconnect", &src, ".") else {
        return;
    };
    let h: Handler = Arc::new(|_, _, m, _| Err(no_method(m)));
    let (child, conn) = start(&dir, 0, h);
    let ctx = Ctx::background().with_timeout(Duration::from_secs(20));
    let init: Value = conn
        .call(&ctx, "initialize", &json!({"settings": {"wait": "5s"}}))
        .unwrap();
    assert_eq!(init, json!({}));
    let set: Value = conn
        .call(&ctx, "ui.settings", &json!({"values": {"tries": "3"}}))
        .unwrap();
    assert_eq!(set, json!({}));
    conn.notify(
        "ui.event",
        &json!({"kind": "network.down", "ui": "w1", "at": "2026-10-29T10:00:00Z"}),
    )
    .unwrap();
    let tools: Value = conn.call(&ctx, "tools.list", &()).unwrap();
    assert_eq!(tools, json!({"tools": []}));
    let e = conn
        .call::<_, Value>(&ctx, "ui.command", &json!({"command": "x"}))
        .unwrap_err();
    assert_eq!(e.to_string(), "reconnect has no commands");
    stop(child, conn);
}

/// Runs a real plugin in the real sandbox and has it try to get out: read
/// your files, write outside its data folder, start a program, reach the
/// internet, localhost or rush's own sockets.
#[test]
#[cfg(target_os = "macos")]
fn sandbox_holds() {
    let home = Home::new();
    let dir = rush_plugin::root().join("probe");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        r#"{"name":"probe","command":["probe"],"network":["example.com:443"]}"#,
    )
    .unwrap();
    if !go_build(
        &repo(),
        "./internal/plugin/testdata/probe",
        &dir.join("probe"),
    ) {
        return;
    }
    let p = load(&dir).unwrap();
    let secret_dir = home.path().join("secrets");
    std::fs::create_dir_all(&secret_dir).unwrap();
    let secret = secret_dir.join("token");
    std::fs::write(&secret, "sk-secret").unwrap();
    let sock = home.path().join("rush.sock");
    let _ln = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let proxy = Proxy::listen(&p.network, None).unwrap();

    let h: Handler = Arc::new(|_, _, m, _| Err(no_method(m)));
    let (child, conn) = start(&dir, proxy.port(), h);
    let ctx = Ctx::background().with_timeout(Duration::from_secs(20));
    let data = rush_plugin::data_dir("probe");
    let got: std::collections::BTreeMap<String, String> = conn
        .call(
            &ctx,
            "probe",
            &json!({
                "secret": secret, "outside": dir.join("planted"), "sock": sock,
                "data": data, "proxy": proxy.port(),
            }),
        )
        .unwrap();
    for (k, want) in [
        ("read secret", "denied"),
        ("write outside", "denied"),
        ("list secret dir", "denied"),
        ("exec", "denied"),
        ("internet", "denied"),
        ("localhost", "denied"),
        ("rush socket", "denied"),
        ("write data", "allowed"),
        ("proxy", "allowed"),
    ] {
        let v = got.get(k).cloned().unwrap_or_default();
        assert!(v.starts_with(want), "{k}: {v}, want {want}");
    }
    assert_eq!(
        got.get("token").map(String::as_str).unwrap_or(""),
        "",
        "the plugin saw rush's API key"
    );
    assert_eq!(got["home"], data.display().to_string());
    stop(child, conn);
    proxy.close();
}
