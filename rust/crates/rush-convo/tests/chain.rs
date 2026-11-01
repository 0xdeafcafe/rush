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
