//! #new and #with's words, after Go's internal/ui/newcmd_test.go
//! (TestNewRefuses, TestNewWords' task and quotes) and with_test.go
//! (TestWith's refusal): what doesn't name a route says why, and nothing
//! changes a default.

use rush_core::adapter::Registry;
use rush_state::Store;

use super::newcmd::{new_preview, parse_new, skip_catalogs, with_command};
use super::state::{accounts, cfg, set_store};
use crate::model::Model;

fn model() -> (Model, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let mut m = Model::new("0.1.0");
    let r = Registry::new();
    r.register(rush_claude::Claude::new());
    m.reg = r;
    m.w = 120;
    m.h = 40;
    set_store(&mut m, Store::default());
    skip_catalogs(&mut m);
    (m, d)
}

// What can't run says why, and what to try.
#[test]
fn new_refuses() {
    let (m, _d) = model();
    for (arg, want) in [
        (
            "nosuch@anthropic-sub task",
            "no harness called nosuch · claude-code",
        ),
        ("claude-code@nowhere task", "no provider called nowhere"),
        (
            "@sub task",
            "@sub is the harness's own provider: name the harness, codex@sub",
        ),
    ] {
        let e = parse_new(&m, arg).expect_err(arg);
        assert!(e.contains(want), "{arg}: {e}, want {want:?}");
    }
    let p = new_preview(&m, "#new nosuch@x y")
        .expect("previewed")
        .plain();
    assert!(p.starts_with("✗ no harness called nosuch"), "{p}");
    assert!(new_preview(&m, "#done").is_none());
    assert!(new_preview(&m, "hello").is_none());
}

// A first word that names no route is the task's; quotes keep a word
// that would be taken.
#[test]
fn new_words_are_the_task() {
    let (m, _d) = model();
    let n = parse_new(&m, "fix the flaky test").unwrap();
    assert_eq!(n.task, "fix the flaky test");
    assert_eq!(n.o.kind, "claude");
    let n = parse_new(&m, r#""high risk, plan it""#).unwrap();
    assert_eq!(
        (n.task.as_str(), n.o.effort.as_str()),
        ("high risk, plan it", "")
    );
    let p = new_preview(&m, "#new fix it").unwrap().plain();
    assert!(p.contains("once, defaults untouched"), "{p}");
    let p = new_preview(&m, "#with").unwrap().plain();
    assert!(p.contains("next session"), "{p}");
}

// #with an unknown route takes nothing and changes no default.
#[test]
fn with_refuses() {
    let (mut m, _d) = model();
    let before = cfg(&m).default_agent();
    with_command(&mut m, "nosuch@nowhere");
    assert!(accounts(&mut m).start_over.is_none());
    let f = m.flash.clone().unwrap();
    assert!(f.0.starts_with("no harness called nosuch") && f.2, "{f:?}");
    assert_eq!(cfg(&m).default_agent(), before);
}
