//! A shell block in a reply drawn as a terminal box: before it's run
//! (the folder, ▶ run and ⧉ copy), picked (its keys on the foot), run
//! (its output under a rule, how it ended), a chain running (each
//! command's time, the one running now, k and p), a long output capped
//! to its newest rows, and a `!` run drawn after the items its turn had.
//! Rust-only. UPDATE_SNAPSHOTS=1 writes the snapshot again.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rush_convo::render::plain;
use rush_convo::shellbox::{BoxRef, ShellRun, bang_ref, block_ref, shell_blocks};
use rush_convo::{Options, Renderer, Session, Shell, ShellProc};

mod common;

use common::*;

const REPLY: &str = "Remove the key:\n\n```sh\n$ rm ~/Desktop/DeveloperIDApplication.p12\n```";
const CHAIN: &str = "Then check:\n\n```bash\ncargo build\ncargo test\n```";

fn session(reply: &str) -> Session {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    s.feed(sent("how do I clean up?"), at(0));
    s.feed(say(reply), at(1));
    s.feed(done(0.0), at(2));
    s
}

fn opts(selected: &str, runs: &[(&str, ShellRun)], now: i64) -> Options {
    let shells: HashMap<String, Arc<ShellRun>> = runs
        .iter()
        .map(|(k, r)| (k.to_string(), Arc::new(r.clone())))
        .collect();
    Options {
        width: 64,
        now,
        selected: selected.into(),
        shells,
        ..Options::default()
    }
}

fn draw(s: &Session, o: &Options) -> String {
    plain(&Renderer::new().render(s, o))
}

/// The box's rows alone: from its top edge to its foot.
fn the_box(all: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for l in all.lines() {
        if l.contains("╭─") {
            inside = true;
        }
        if inside {
            out.push_str(l);
            out.push('\n');
        }
        if inside && l.contains('╰') {
            break;
        }
    }
    out
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

fn finished(cmd: &str, out: &str, exit: i32) -> ShellRun {
    let mut r = ShellRun::new(cmd, "/work/app", at(10));
    r.push(out);
    r.finish(Some(exit), at(10) + 100);
    r
}

/// Before it's run, picked, and run: the three as one snapshot.
#[test]
fn a_block_before_and_after_its_run() {
    let s = session(REPLY);
    let r = block_ref(1, 0, 0);
    assert_eq!(r, "t1:sh:0:0");
    let mut got = String::from("before\n");
    got.push_str(&the_box(&draw(&s, &opts("", &[], at(5)))));
    got.push_str("picked\n");
    got.push_str(&the_box(&draw(&s, &opts(&r, &[], at(5)))));
    got.push_str("run\n");
    let run = finished("rm ~/Desktop/DeveloperIDApplication.p12", "removed\n", 0);
    got.push_str(&the_box(&draw(&s, &opts("", &[(&r, run)], at(20)))));
    snapshot("shellbox_block", &got);
    assert!(got.contains("▶ run  ⧉ copy"), "{got}");
    assert!(got.contains("enter runs it · c copies it"), "{got}");
    assert!(got.contains("✓ exit 0 · 0.1s"), "{got}");
    assert!(got.contains("→ send to agent  ↻ again  ✕"), "{got}");
}

/// The buttons are links a click follows.
#[test]
fn the_buttons_link_to_what_they_do() {
    let s = session(REPLY);
    let ls = Renderer::new().render(&s, &opts("", &[], at(5)));
    let top = ls
        .iter()
        .find(|l| l.r#ref == "t1:sh:0:0")
        .expect("the box's top edge is its row");
    let col = top
        .line
        .plain()
        .find("▶")
        .map(|b| top.line.plain()[..b].chars().count());
    let url = rush_core::text::link_at(&top.line, col.unwrap()).expect("▶ run is a link");
    assert_eq!(&*url, "rush:shell/run/t1:sh:0:0");
    // The box's other rows are its own, for a click on them.
    let n = ls.iter().filter(|l| l.owner == "t1:sh:0:0").count();
    assert!(n >= 3, "{n}");
}

/// A failed run says its code, a stopped one that it was stopped, and
/// output past OUT_SHOWN rows shows its newest and how many more.
#[test]
fn how_it_ended_and_its_newest_rows() {
    let s = session(REPLY);
    let r = block_ref(1, 0, 0);
    let mut out = String::new();
    for i in 1..=30 {
        out.push_str(&format!("line {i}\n"));
    }
    let run = finished("rm ~/Desktop/DeveloperIDApplication.p12", &out, 1);
    let got = draw(&s, &opts(&r, &[(&r, run.clone())], at(20)));
    assert!(got.contains("✗ exit 1"), "{got}");
    assert!(got.contains("… 18 more lines · enter opens all"), "{got}");
    assert!(got.contains("line 30") && got.contains("line 19"), "{got}");
    assert!(!got.contains("line 18 "), "{got}");
    // Opened, all of it.
    let mut o = opts(&r, &[(&r, run)], at(20));
    o.open.insert(r.clone(), true);
    let got = draw(&s, &o);
    assert!(got.contains("line 1 ") && got.contains("line 30"), "{got}");
    // Stopped.
    let mut run = finished("rm ~/Desktop/DeveloperIDApplication.p12", "", 0);
    run.stopped = true;
    run.exit = None;
    let got = draw(&s, &opts("", &[(&r, run)], at(20)));
    assert!(got.contains("✕ stopped"), "{got}");
    assert!(got.contains("(no output)"), "{got}");
    // A run of what the block no longer says isn't its.
    let run = finished("something else", "x\n", 0);
    let got = draw(&s, &opts("", &[(&r, run)], at(20)));
    assert!(got.contains("▶ run"), "{got}");
}

/// A block of two commands running: the first's time, the spinner on the
/// one running now, where the chain is on the foot, and k and p.
#[test]
fn a_chain_running_in_the_box() {
    let s = session(CHAIN);
    let r = block_ref(1, 0, 0);
    let mut run = ShellRun::new("cargo build\ncargo test", "/work/app", at(10));
    let sh = |args: &[&str], from: i64| Shell {
        command: "cargo build\ncargo test".into(),
        start: at(10),
        kids: vec![ShellProc {
            pid: 4242,
            args: args.iter().map(|a| a.to_string()).collect(),
            start: from,
            kids: Vec::new(),
        }],
        ..Shell::default()
    };
    run.watch(&sh(&["cargo", "build"], at(10)), at(11));
    run.watch(&sh(&["cargo", "test"], at(14)), at(15));
    run.push("   Compiling app\n");
    assert_eq!(run.label(), "2/2 cargo test");
    let rp = run.running_part().expect("cargo test runs now");
    assert_eq!((rp.at, rp.of, rp.procs[0].pid), (2, 2, 4242));
    let got = draw(&s, &opts("", &[(&r, run.clone())], at(20)));
    let build = got
        .lines()
        .find(|l| l.contains("$ cargo build"))
        .expect(&got);
    assert!(build.contains("✓ 4.0s"), "{build}");
    let test = got
        .lines()
        .find(|l| l.contains("▸ cargo test"))
        .expect(&got);
    assert!(test.contains("6s"), "{test}");
    assert!(got.contains("2/2 cargo test"), "{got}");
    assert!(
        got.contains("↑ pick this, then  k skip cargo test · p pause it"),
        "{got}"
    );
    assert!(got.contains("✕ stop"), "{got}");
    assert!(got.contains("Compiling app"), "{got}");
    let picked = draw(&s, &opts(&r, &[(&r, run)], at(20)));
    assert!(
        picked.contains("k skip cargo test · p pause it"),
        "{picked}"
    );
    assert!(!picked.contains("pick this"), "{picked}");
}

/// A `!` run is drawn after the items its turn had when it ran, with !
/// on its top edge.
#[test]
fn a_bang_run_after_its_turns_items() {
    let s = session(REPLY);
    let mut run = finished("git status", "clean\n", 0);
    run.bang = true;
    run.turn = 1;
    run.after = 1;
    let r = bang_ref(1, run.start);
    assert_eq!(
        BoxRef::parse(&r),
        Some(BoxRef::Bang {
            turn: 1,
            start: run.start
        })
    );
    let got = draw(&s, &opts("", &[(&r, run)], at(20)));
    let reply = got.find("Remove the key").expect(&got);
    let bang = got.find("! shell · /work/app").expect(&got);
    assert!(bang > reply, "after the reply:\n{got}");
    assert!(got.contains("$ git status"), "{got}");
    assert!(got.contains("clean"), "{got}");
}

#[test]
fn blocks_are_found_as_drawn() {
    let b = shell_blocks(REPLY);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].script(), "rm ~/Desktop/DeveloperIDApplication.p12");
}
