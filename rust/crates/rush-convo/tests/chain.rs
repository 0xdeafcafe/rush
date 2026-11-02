//! A running chain in the conversation: which of its commands runs now
//! (n/m on the step's row), how long each ran or runs (beside each
//! command), and the keys that skip the one running now or stop the whole
//! chain, in every view that shows the step. After Go's
//! internal/convo/chainrun_test.go (TestWatchShellsTimesEachPart).
//! UPDATE_SNAPSHOTS=1 writes the snapshot again.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::path::Path;

use rush_convo::render::plain;
use rush_convo::{Options, Preset, Renderer, Session, Shell, ShellProc};
use serde_json::json;

mod common;

use common::*;

const CMD: &str = "cargo build && cargo test --workspace; cargo clippy";

/// A turn whose Bash call runs CMD: the build seen from 1s, the tests
/// from 5s, so the build took 4s.
fn running() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("check it all"), at(0));
    s.feed(tool_use("b1", "Bash", json!({"command": CMD})), at(1));
    let look = |s: &mut Session, args: &[&str], from: i64, now: i64| {
        let sh = Shell {
            command: CMD.into(),
            start: at(1),
            kids: vec![ShellProc {
                pid: 4242,
                args: args.iter().map(|a| a.to_string()).collect(),
                start: from,
                kids: Vec::new(),
            }],
            ..Shell::default()
        };
        s.watch_shells(&[sh], now);
    };
    look(&mut s, &["cargo", "build"], at(1), at(2));
    look(&mut s, &["cargo", "test", "--workspace"], at(5), at(6));
    s
}

fn draw(s: &Session, p: Preset, selected: &str) -> String {
    let o = Options {
        width: 100,
        now: at(20),
        preset: p,
        selected: selected.into(),
        ..Options::default()
    };
    plain(&Renderer::new().render(s, &o))
}

fn snapshot(name: &str, got: &str) {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !p.exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&p).unwrap();
    assert_eq!(got, want, "{name} differs; UPDATE_SNAPSHOTS=1 to accept");
}

/// Normal: the step's row says 2/3 cargo test, each command its time
/// (the build's 4s, the tests' spinner and 15s, clippy nothing yet), and
/// under them the keys; picked, the keys alone.
#[test]
fn normal_shows_the_place_the_times_and_the_keys() {
    let s = running();
    let got = draw(&s, Preset::Normal, "");
    snapshot("chain_running_normal", &got);
    let row = got
        .lines()
        .find(|l| l.starts_with("t1:s:b1") && l.contains("2/3 cargo test"))
        .unwrap_or_else(|| panic!("the step's row names the command running now:\n{got}"));
    assert!(row.contains("19s"), "how long the call has run: {row}");
    let build = got
        .lines()
        .find(|l| l.contains("$ cargo build"))
        .expect(&got);
    assert!(build.contains("✓ 4"), "the build took 4s: {build}");
    let test = got
        .lines()
        .find(|l| l.contains("▸ cargo test"))
        .expect(&got);
    assert!(test.contains("15"), "the tests run 15s now: {test}");
    let clippy = got
        .lines()
        .find(|l| l.contains("$ cargo clippy"))
        .expect(&got);
    assert!(!clippy.contains('✓'), "clippy hasn't run: {clippy}");
    assert!(
        got.contains(
            "↑ pick this step, then  k skip cargo test · x stop the whole chain · p pause it"
        ),
        "{got}"
    );
    let picked = draw(&s, Preset::Normal, "t1:s:b1");
    assert!(
        picked.contains("k skip cargo test · x stop the whole chain · p pause it")
            && !picked.contains("pick this step"),
        "{picked}"
    );
}

/// Code's check line, Trail's step line and Chat's bubble foot say
/// which command runs; Code shows the keys, Trail once it's picked.
#[test]
fn other_views_say_it_too() {
    let s = running();
    let code = draw(&s, Preset::Code, "");
    let row = code.lines().find(|l| l.contains("running")).expect(&code);
    assert!(row.contains("2/3 cargo test"), "{code}");
    assert!(code.contains("k skip cargo test"), "{code}");
    let trail = draw(&s, Preset::Trail, "");
    assert!(trail.contains("2/3 cargo test"), "{trail}");
    assert!(!trail.contains("k skip"), "{trail}");
    let trail = draw(&s, Preset::Trail, "t1:s:b1");
    assert!(trail.contains("k skip cargo test"), "{trail}");
    let chat = draw(&s, Preset::Chat, "");
    assert!(chat.contains("✱ running 2/3 cargo test"), "{chat}");
}

/// Once the call ends its row says no command, and no keys show.
#[test]
fn a_finished_chain_has_no_keys() {
    let mut s = running();
    s.feed(
        tool_result("b1", "", false, Some(json!({"stdout": "", "stderr": ""}))),
        at(30),
    );
    let got = draw(&s, Preset::Normal, "");
    assert!(!got.contains("2/3"), "{got}");
    assert!(!got.contains("k skip"), "{got}");
}

/// The shell Claude Code runs a Bash call in, as rush reads it off the
/// process: its command eval'd inside a wrapper (a quote written
/// '"'"'), so its own -c command isn't the call's and is left empty.
fn claude_shell(cmd: &str, start: i64, kids: Vec<ShellProc>) -> Shell {
    Shell {
        cmd: format!(
            "/bin/zsh -c source /Users/me/.claude/shell-snapshots/snapshot-zsh-1.sh 2>/dev/null || true && eval '{}' < /dev/null && pwd -P >| /tmp/claude-1a2b-cwd",
            cmd.replace('\'', r#"'"'"'"#)
        ),
        command: String::new(),
        start,
        kids,
    }
}

fn proc_(pid: i32, args: &str, start: i64, kids: Vec<ShellProc>) -> ShellProc {
    ShellProc {
        pid,
        args: args.split_whitespace().map(str::to_string).collect(),
        start,
        kids,
    }
}

/// A running chain that isn't the turn's newest step (the agent ran a
/// Read beside it) still shows its commands, each one's time and the
/// keys, without being opened by hand.
#[test]
fn a_chain_beside_another_call_stays_open() {
    let mut s = running();
    s.feed(
        tool_use("r1", "Read", json!({"file_path": "/work/app/src/main.rs"})),
        at(7),
    );
    let got = draw(&s, Preset::Normal, "");
    let build = got
        .lines()
        .find(|l| l.contains("$ cargo build"))
        .unwrap_or_else(|| panic!("the chain's commands are shown:\n{got}"));
    assert!(build.contains("✓ 4"), "{build}");
    assert!(got.contains("k skip cargo test"), "{got}");
}

/// A Bash call with a quote in it, in Claude Code's real wrapper, with a
/// shell of a background task started just before: its own shell is
/// found, so its row says where it is.
#[test]
fn a_quoted_chain_in_claude_codes_wrapper() {
    let cmd = "rg -n 'TODO' src; cargo test --workspace";
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("check it"), at(0));
    s.feed(tool_use("b1", "Bash", json!({"command": cmd})), at(1));
    let dev = claude_shell(
        "npm run dev",
        at(1) + 100,
        vec![proc_(70, "node vite", at(1) + 100, Vec::new())],
    );
    let mine = claude_shell(
        cmd,
        at(1) + 300,
        vec![proc_(71, "cargo test --workspace", at(2), Vec::new())],
    );
    s.watch_shells(&[dev, mine], at(3));
    let got = draw(&s, Preset::Normal, "");
    assert!(
        got.lines()
            .any(|l| l.starts_with("t1:s:b1") && l.contains("2/2 cargo test")),
        "{got}"
    );
    assert!(got.contains("k skip cargo test"), "{got}");
}

/// A loop: its command running now is marked, with how long it has run
/// and the loop's round, on the line it's written on.
#[test]
fn a_loops_command_is_marked() {
    let cmd = "for p in rush-core rush-convo; do\n  cargo test -p $p\ndone";
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("test them"), at(0));
    s.feed(tool_use("b1", "Bash", json!({"command": cmd})), at(1));
    let look = |s: &mut Session, krate: &str, from: i64, now: i64| {
        let p = proc_(80, &format!("cargo test -p {krate}"), from, Vec::new());
        s.watch_shells(&[claude_shell(cmd, at(1), vec![p])], now);
    };
    look(&mut s, "rush-core", at(1), at(2));
    look(&mut s, "rush-convo", at(8), at(9));
    let got = draw(&s, Preset::Normal, "");
    let line = got
        .lines()
        .find(|l| l.contains("cargo test -p $p"))
        .unwrap_or_else(|| panic!("{got}"));
    assert!(line.contains('▸') && line.contains("run 2"), "{line}");
    assert!(
        got.lines()
            .any(|l| l.starts_with("t1:s:b1") && l.contains("cargo test · run 2")),
        "{got}"
    );
}

/// A script file: the commands seen run under it, each with its time,
/// the one running now marked, and the keys for it.
#[test]
fn a_script_files_commands_are_shown() {
    let cmd = "bash scripts/check.sh";
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("check"), at(0));
    s.feed(tool_use("b1", "Bash", json!({"command": cmd})), at(1));
    let look = |s: &mut Session, kid: ShellProc, now: i64| {
        let script = proc_(90, "/bin/bash scripts/check.sh", at(1), vec![kid]);
        s.watch_shells(&[claude_shell(cmd, at(1), vec![script])], now);
    };
    look(&mut s, proc_(91, "cargo build", at(1), Vec::new()), at(2));
    look(
        &mut s,
        proc_(92, "cargo test --workspace", at(5), Vec::new()),
        at(6),
    );
    let got = draw(&s, Preset::Normal, "");
    assert!(
        got.lines()
            .any(|l| l.starts_with("t1:s:b1") && l.contains("now cargo test")),
        "{got}"
    );
    let build = got
        .lines()
        .find(|l| l.contains("cargo build"))
        .unwrap_or_else(|| panic!("{got}"));
    assert!(build.contains("✓ 4"), "{build}");
    let test = got
        .lines()
        .find(|l| l.contains("▸ cargo test"))
        .unwrap_or_else(|| panic!("{got}"));
    assert!(test.contains("15"), "the tests run 15s now: {test}");
    assert!(got.contains("k skip cargo test"), "{got}");
}

const PIPE: &str =
    "cargo build && cargo test --workspace 2>&1 | grep -E \"result|FAILED\" | head; cargo clippy";

/// A chain whose second command is a pipeline: its stages run at once.
fn piping() -> Session {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("check it all"), at(0));
    s.feed(tool_use("b1", "Bash", json!({"command": PIPE})), at(1));
    let sh = |kids| claude_shell(PIPE, at(1), kids);
    s.watch_shells(
        &[sh(vec![proc_(10, "cargo build", at(1), Vec::new())])],
        at(2),
    );
    s.watch_shells(
        &[sh(vec![
            proc_(11, "cargo test --workspace", at(5), Vec::new()),
            proc_(12, "grep -E result|FAILED", at(5), Vec::new()),
            proc_(13, "head", at(5), Vec::new()),
        ])],
        at(6),
    );
    s
}

/// A pipeline is one command of the chain (one place, one time) drawn a
/// stage a line: the first behind the marker and time, the filters under
/// it behind a faint bar.
#[test]
fn a_pipeline_is_drawn_a_stage_a_line() {
    let s = piping();
    let got = draw(&s, Preset::Normal, "");
    let rows: Vec<&str> = got.lines().collect();
    let at_test = rows
        .iter()
        .position(|l| l.contains("▸ cargo test --workspace 2>&1"))
        .unwrap_or_else(|| panic!("{got}"));
    assert!(rows[at_test].contains("15"), "the time: {}", rows[at_test]);
    assert!(
        rows[at_test + 1].contains("│ grep -E \"result|FAILED\""),
        "{got}"
    );
    assert!(rows[at_test + 2].contains("│ head"), "{got}");
    assert!(!rows[at_test + 1].contains('▸'), "{got}");
    assert!(
        rows.iter().any(|l| l.contains("$ cargo clippy")),
        "the next command follows the stages: {got}"
    );
    let row = rows
        .iter()
        .find(|l| l.starts_with("t1:s:b1") && l.contains("2/3"))
        .unwrap_or_else(|| panic!("{got}"));
    assert!(row.contains("2/3 cargo test"), "{row}");
    assert!(!row.contains("grep"), "only the first stage: {row}");
}

/// k on a pipeline ends all its stages together; they're one command.
#[test]
fn skipping_a_pipeline_ends_every_stage() {
    let s = piping();
    let rp = s.running_part("b1").expect("running");
    assert_eq!((rp.at, rp.of), (2, 3));
    let pids: Vec<i32> = rp.procs.iter().map(|p| p.pid).collect();
    assert_eq!(pids, [11, 12, 13]);
}
