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
use rush_convo::{Options, Preset, Renderer, Session, Shell, ShellProc};

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
    snap("shellbox_block", &got);
    assert!(got.contains("▶ run  ✎ edit  ⧉ copy"), "{got}");
    assert!(
        got.contains("enter runs it · e edits it · c copies it"),
        "{got}"
    );
    assert!(got.contains("✓ exit 0 · 0.1s"), "{got}");
    assert!(got.contains("→ send  ✎ draft  ≈ summary  ↻  ✕"), "{got}");
}

/// A finished run's foot as each of its buttons was used: sent to the
/// agent, drafted in your message, summed up (and summing), a summary
/// that failed saying why; and the words it has at each width.
#[test]
fn the_foot_as_its_buttons_are_used() {
    let s = session(REPLY);
    let r = block_ref(1, 0, 0);
    let base = finished("rm ~/Desktop/DeveloperIDApplication.p12", "removed\n", 0);
    let mut got = String::new();
    let mut each = |name: &str, f: &dyn Fn(&mut ShellRun), width: usize| {
        let mut run = base.clone();
        f(&mut run);
        let mut o = opts("", &[(&r, run)], at(20));
        o.width = width;
        got.push_str(name);
        got.push('\n');
        got.push_str(&the_box(&draw(&s, &o)));
    };
    each("wide", &|_| {}, 90);
    each("sent", &|x| x.told = true, 64);
    each("drafted", &|x| x.sent = true, 64);
    each("summing", &|x| x.summing = true, 64);
    each(
        "summary sent",
        &|x| (x.told, x.told_summary) = (true, true),
        64,
    );
    each(
        "summary failed",
        &|x| x.note = "couldn't sum it up: timed out".into(),
        64,
    );
    each("narrow", &|_| {}, 40);
    snap("shellbox_foot", &got);
    assert!(
        got.contains("→ send to agent  ✎ draft  ≈ summary  ↻ again  ✕"),
        "{got}"
    );
    assert!(got.contains("✓ sent"), "{got}");
    assert!(got.contains("✓ in box"), "{got}");
    assert!(got.contains("≈ summing…"), "{got}");
    assert!(got.contains("✓ summary"), "{got}");
    assert!(got.contains("✗ couldn't sum it up: timed out"), "{got}");
}

/// A block's commands being edited: the editor in the box, its cursor
/// and keys; once run, the box runs and shows what was edited, marked.
#[test]
fn a_block_edited_before_its_run() {
    let s = session(REPLY);
    let r = block_ref(1, 0, 0);
    let mut o = opts(&r, &[], at(5));
    o.shell_edit = Some(rush_convo::shellbox::ShellEdit {
        r: r.clone(),
        text: "rm -i ~/Desktop/DeveloperIDApplication.p12\nls ~/Desktop".into(),
        cursor: 5,
    });
    let mut got = String::from("editing\n");
    got.push_str(&the_box(&draw(&s, &o)));
    assert!(got.contains("shell · editing"), "{got}");
    assert!(got.contains("$ rm -i ~/Desktop"), "{got}");
    assert!(got.contains("$ ls ~/Desktop"), "{got}");
    assert!(got.contains("enter runs it"), "{got}");
    assert!(got.contains("esc as it was"), "{got}");
    // Run as edited: the edited commands, marked, not the agent's.
    let mut run = finished(
        "rm -i ~/Desktop/DeveloperIDApplication.p12\nls ~/Desktop",
        "removed\n",
        0,
    );
    run.edited = true;
    got.push_str("edited\n");
    let ran = the_box(&draw(&s, &opts("", &[(&r, run)], at(20))));
    got.push_str(&ran);
    snap("shellbox_edited", &got);
    assert!(ran.contains("shell · edited"), "{ran}");
    assert!(ran.contains("rm -i ~/Desktop"), "{ran}");
    assert!(ran.contains("✓ exit 0"), "{ran}");
    // The agent's words are as they were, above the box.
    let all = draw(&s, &opts("", &[], at(20)));
    assert!(all.contains("$ rm ~/Desktop"), "{all}");
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

// ---- the boxes in the other views ----

const TWO: &str = "Remove the key:\n\n```sh\n$ rm ~/Desktop/DeveloperIDApplication.p12\n```\n\nThen check:\n\n```bash\ncargo test\n```\n\nThen you're done.";

/// A snapshot that must be there: a missing one fails, showing what it
/// would hold, to be checked and written by hand.
fn snap(name: &str, got: &str) {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&p, got).unwrap();
        return;
    }
    let Ok(want) = std::fs::read_to_string(&p) else {
        panic!("no snapshot {name}; it would be:\n{got}<<end>>");
    };
    assert!(
        got == want,
        "{name} differs; UPDATE_SNAPSHOTS=1 to accept. got:\n{got}<<end>>"
    );
}

fn in_view(preset: Preset, mut o: Options) -> Options {
    o.preset = preset;
    o
}

/// The first block run, a `!` run after the reply, the second block not
/// run: what each view is given.
fn ran_some() -> (Session, Vec<(String, ShellRun)>) {
    let s = session(TWO);
    let block = finished("rm ~/Desktop/DeveloperIDApplication.p12", "removed\n", 0);
    let mut bang = finished("git status", "clean\n", 0);
    bang.start = at(12);
    bang.end = at(12) + 300;
    (bang.bang, bang.turn, bang.after) = (true, 1, 1);
    let runs = vec![(block_ref(1, 0, 0), block), (bang_ref(1, bang.start), bang)];
    (s, runs)
}

fn opts_of(runs: &[(String, ShellRun)], selected: &str, now: i64) -> Options {
    let rs: Vec<(&str, ShellRun)> = runs.iter().map(|(k, r)| (k.as_str(), r.clone())).collect();
    opts(selected, &rs, now)
}

/// Chat: a reply's shell block is a box in its place between its
/// bubbles; a `!` run a box on your side, after the reply it followed.
#[test]
fn chat_has_the_boxes_between_the_bubbles() {
    let (s, runs) = ran_some();
    let o = in_view(Preset::Chat, opts_of(&runs, "", at(20)));
    let got = draw(&s, &o);
    snap("shellbox_chat", &got);
    let first = got.find("Remove the key").expect(&got);
    let block = got.find("╭─ shell · /work").expect(&got);
    let then = got.find("Then check").expect(&got);
    let unrun = got.find("$ cargo test").expect(&got);
    let done = got.find("Then you're done").expect(&got);
    let bang = got.find("! shell · /wor").expect(&got);
    assert!(
        first < block && block < then && then < unrun && unrun < done && done < bang,
        "{got}"
    );
    assert!(got.contains("✓ exit 0"), "{got}");
    // Yours on the right: its box starts further in than the reply's.
    let col = |needle: &str| {
        let l = got.lines().find(|l| l.contains(needle)).unwrap();
        let t = l.split_once('|').unwrap().1;
        t[..t.find('╭').unwrap()].chars().count()
    };
    assert!(col("! shell") > col("╭─ shell"), "{got}");
    // The rows carry the boxes' refs, for enter and the buttons.
    let ls = Renderer::new().render(&s, &o);
    for r in ["t1:sh:0:0", "t1:sh:0:1", runs[1].0.as_str()] {
        assert!(ls.iter().any(|l| l.r#ref == r), "{r}:\n{got}");
    }
}

/// Code: the commands you ran as check lines, enter opening one's box
/// under it; a block not run as its box, ▶ run on it.
#[test]
fn code_has_your_runs_as_check_lines() {
    let (s, runs) = ran_some();
    let got = draw(&s, &in_view(Preset::Code, opts_of(&runs, "", at(20))));
    snap("shellbox_code", &got);
    let row = got
        .lines()
        .find(|l| l.starts_with("t1:sh:0:0") && l.contains("rm ~/Desktop"))
        .expect(&got);
    assert!(
        row.contains("✓ $ rm ~/Desktop/DeveloperIDApplication.p12"),
        "{row}"
    );
    assert!(row.contains("exit 0"), "{row}");
    assert!(got.contains("✓ $ git status"), "{got}");
    assert!(got.contains("▶ run"), "the block not run:\n{got}");
    assert!(got.contains("$ cargo test"), "{got}");
    assert!(!got.contains("removed"), "closed, no output:\n{got}");
    // Opened, its box under it.
    let mut o = in_view(Preset::Code, opts_of(&runs, "t1:sh:0:0", at(20)));
    o.open.insert("t1:sh:0:0".into(), true);
    let got = draw(&s, &o);
    assert!(got.contains("removed"), "{got}");
    assert!(got.contains("→ send"), "{got}");
}

/// Catch-up: what you ran a line each; one running still is still going.
#[test]
fn catch_up_says_what_you_ran() {
    let (s, mut runs) = ran_some();
    let mut going = ShellRun::new("cargo test", "/work/app", at(14));
    going.push("running 3 tests\n");
    runs.push((block_ref(1, 0, 1), going));
    let got = draw(&s, &in_view(Preset::CatchUp, opts_of(&runs, "", at(20))));
    let ran = got.lines().find(|l| l.contains("You ran")).expect(&got);
    assert!(ran.contains("✓ $ rm ~/Desktop"), "{got}");
    assert!(got.contains("✓ $ git status"), "{got}");
    let still = got.lines().find(|l| l.contains("Still going")).expect(&got);
    assert!(still.contains("cargo test · 6s"), "{still}");
}

/// Trail: a run as a line with its time bar, where it started.
#[test]
fn trail_has_your_runs() {
    let (s, runs) = ran_some();
    let got = draw(&s, &in_view(Preset::Trail, opts_of(&runs, "", at(20))));
    let row = got
        .lines()
        .find(|l| l.contains("you ran  git status"))
        .expect(&got);
    assert!(row.contains('█') || row.contains('▏'), "{row}");
    assert!(row.contains("0.3s"), "{row}");
    assert!(row.starts_with(runs[1].0.as_str()), "{row}");
    assert!(got.contains("you ran  rm ~/Desktop"), "{got}");
}

/// A `!` run in a session with no turn: its box is all there is, in
/// every view, as a window draws it too; with a turn after, it stays
/// above it.
#[test]
fn a_bang_run_before_any_turn() {
    let mut s = Session::new();
    s.cwd = "/work/app".into();
    let mut run = finished("ls", "a.txt\n", 0);
    run.bang = true;
    let r = bang_ref(0, run.start);
    assert!(r.starts_with("t0:bang:"));
    for p in [
        Preset::Normal,
        Preset::Chat,
        Preset::Code,
        Preset::CatchUp,
        Preset::Trail,
    ] {
        let o = in_view(p, opts("", &[(&r, run.clone())], at(20)));
        let got = draw(&s, &o);
        assert!(got.contains("! shell · /work/app"), "{p:?}:\n{got}");
        assert!(got.contains("a.txt"), "{p:?}:\n{got}");
        let mut rd = Renderer::new();
        let n = rd.count(&s, &o);
        assert!(n >= 5, "{p:?}: {n}");
        rd.window(&s, &o, 0, n);
        let rows = rd.rows();
        assert_eq!(rows.len(), n, "{p:?}");
        assert!(rows.iter().any(|l| l.r#ref == r), "{p:?}");
    }
    s.feed(sent("now the real work"), at(30));
    let got = draw(&s, &opts("", &[(&r, run)], at(40)));
    let bx = got.find("a.txt").expect(&got);
    let asked = got.find("now the real work").expect(&got);
    assert!(bx < asked, "{got}");
}
