use super::*;

fn lines(s: &str) -> Vec<&str> {
    s.split(['\n', '\r']).filter(|l| !l.is_empty()).collect()
}

#[test]
fn a_commit_and_its_push() {
    let out = "[main e9c4b0b] docs: a short readme\n 1 file changed, 12 insertions(+)\nTo github.com:o/r.git\n   815de49..e9c4b0b  main -> main\n";
    let cmd = "git add . && git commit -m \"docs: a short readme\" && git push";
    let blank = blank_heredocs(cmd);
    let calls = git_calls(cmd, &blank);
    let ls = lines(out);
    let mut cs = commit_cards(&ls, cmd, true);
    cs.extend(push_card(&ls));
    assert_eq!(cs.len(), 2);
    assert_eq!(cs[0].sha, "e9c4b0b");
    assert_eq!((cs[0].files, cs[0].add, cs[0].del), (1, 12, 0));
    assert_eq!(cs[1].remote, "github.com/o/r");
    assert_eq!(cs[1].refs[0].span, "815de49..e9c4b0b");
    assert_eq!(calls.len(), 2);
    let j = join_cards(cs);
    assert_eq!(j.len(), 1);
    assert_eq!(j[0].pushed, "github.com/o/r");
}

#[test]
fn a_go_vet_error_is_a_build_card() {
    let out = "# rush/internal/ui\n./editor.go:41:2: unreachable code\n";
    let c = build_card("go vet", &lines(out)).expect("a card");
    assert_eq!(c.failed, 1);
    assert_eq!(c.fails[0].msg, "editor.go:41: unreachable code");
    assert_eq!(c.verb(), "go vet");
}

#[test]
fn a_go_test_failure() {
    let out = "--- FAIL: TestRowKeySetting (0.00s)\n    rowkeys_test.go:126: got 1, want 2\nFAIL\nFAIL\trush/internal/ui\t0.2s\n";
    let c = test_card(&lines(out)).expect("a card");
    assert_eq!(c.failed, 1);
    assert_eq!(c.fails[0].name, "TestRowKeySetting");
    assert_eq!(c.fails[0].msg, "rowkeys_test.go:126: got 1, want 2");
    assert!(test_card(&lines("ok  \trush/internal/ui\t0.2s\n")).is_none());
}

#[test]
fn a_fast_forward_merge() {
    let cmd = "git merge worktree-agent-1";
    let blank = blank_heredocs(cmd);
    let calls = git_calls(cmd, &blank);
    let out = "Updating 1..2\nFast-forward\n a.go | 2 +-\n 31 files changed, 1808 insertions(+), 1225 deletions(-)\n";
    let c = merge_card(&lines(out), &calls).expect("a card");
    assert_eq!(
        (c.how, c.branch.as_str()),
        ("fast-forward", "worktree-agent-1")
    );
    assert_eq!((c.files, c.add, c.del), (31, 1808, 1225));
    assert_eq!(c.verb(), "git merge");
}

#[test]
fn a_pull_request() {
    let out = "Creating pull request for feat into main in o/r\n\nhttps://github.com/o/r/pull/8\n";
    let c = pr_create_card(out, "gh pr create --title \"Add it\" --body \"Why\"").expect("a card");
    assert_eq!((c.num.as_str(), c.state), ("8", "open"));
    assert_eq!(
        (c.title.as_str(), c.head.as_str(), c.base.as_str()),
        ("Add it", "feat", "main")
    );
    assert_eq!(c.url, "github.com/o/r/pull/8");
}

#[test]
fn a_heredoc_message_and_its_body() {
    let cmd = "git commit -q -m \"$(cat <<'EOF'\nfix: a thing\n\nThe why.\n\nCo-Authored-By: A <a@b>\nEOF\n)\"";
    let m = commit_messages(cmd);
    assert_eq!(m.len(), 1);
    assert!(m[0].quiet);
    let (body, with) = message_body(&m[0].text);
    assert_eq!(body, vec!["The why."]);
    assert_eq!(with, vec!["A"]);
    // A heredoc that only mentions git commit commits nothing.
    assert!(
        git_calls(
            "cat <<EOF\ngit commit -m x\nEOF",
            &blank_heredocs("cat <<EOF\ngit commit -m x\nEOF")
        )
        .is_empty()
    );
}

#[test]
fn colour_codes_and_multibyte_text_are_safe() {
    assert_eq!(strip_sgr("\x1b[31mred\x1b[0m é"), "red é");
    assert_eq!(repo_name("https://github.com/o/r.git"), "github.com/o/r");
    assert_eq!(repo_name("git@github.com:o/r.git"), "github.com/o/r");
    assert_eq!(dedent(&["  é a", "    b"]), "é a\n  b");
}

/// A card's kind, what, counts and failures (name, msg, build), as Go's
/// tests compare them.
type Want<'a> = (
    &'a str,
    &'a str,
    (usize, usize, usize),
    Vec<(&'a str, &'a str, bool)>,
);

fn check_cards(name: &str, cmd: &str, out: &str, want: Option<Want>) {
    let got = cards_for(cmd, out, false);
    let Some((kind, what, (failed, passed, skipped), fails)) = want else {
        assert!(got.is_empty(), "{name}: {got:?}");
        return;
    };
    assert_eq!(got.len(), 1, "{name}: {got:?}");
    let c = &got[0];
    assert_eq!(
        (c.kind, c.what.as_str(), c.failed, c.passed, c.skipped),
        (kind, what, failed, passed, skipped),
        "{name}: {c:?}"
    );
    let fs: Vec<(&str, &str, bool)> = c
        .fails
        .iter()
        .map(|f| (f.name.as_str(), f.msg.as_str(), f.build))
        .collect();
    assert_eq!(fs, fails, "{name}");
}

// cards_test.go: TestBuildCards' cargo and rustc rows (a06a610c).
#[test]
fn cargo_build_cards() {
    check_cards(
        "cargo build",
        "cargo build 2>&1 | tail -20",
        "   Compiling demo v0.1.0 (/x/demo)\nerror[E0425]: cannot find value `x` in this scope\n --> src/main.rs:4:20\n  |\n4 |     println!(\"{}\", x);\n  |                    ^ not found in this scope\n\nerror[E0308]: mismatched types\n  --> src/net/conn.rs:12:9\n   |\nerror: could not compile `demo` (bin \"demo\") due to 2 previous errors\n",
        Some((
            "build",
            "cargo build",
            (2, 0, 0),
            vec![
                ("", "main.rs:4: cannot find value `x` in this scope", false),
                ("", "conn.rs:12: mismatched types", false),
            ],
        )),
    );
    check_cards(
        "cargo clippy",
        "cargo +nightly clippy --all-targets -- -D warnings",
        "error: this `if` has identical blocks\n  --> src/lib.rs:9:5\n   |\n   = note: `-D clippy::if-same-then-else` implied by `-D warnings`\n\nerror: could not compile `demo` (lib) due to 1 previous error\n",
        Some((
            "build",
            "cargo +nightly clippy",
            (1, 0, 0),
            vec![("", "lib.rs:9: this `if` has identical blocks", false)],
        )),
    );
    check_cards(
        "cargo check, short",
        "cargo check --message-format=short",
        "src/lib.rs:3:5: error[E0433]: failed to resolve: use of undeclared type `Foo`\nerror: could not compile `demo` (lib) due to 1 previous error\n",
        Some((
            "build",
            "cargo check",
            (1, 0, 0),
            vec![(
                "",
                "lib.rs:3: failed to resolve: use of undeclared type `Foo`",
                false,
            )],
        )),
    );
    check_cards(
        "a linker",
        "cargo build --release",
        "error: linking with `cc` failed: exit status: 1\n  |\n  = note: ld: library not found for -lssl\n\nerror: could not compile `demo` (bin \"demo\") due to 1 previous error\n",
        Some((
            "build",
            "cargo build",
            (1, 0, 0),
            vec![("", "linking with `cc` failed: exit status: 1", false)],
        )),
    );
    check_cards(
        "what a program cargo run started says is its own",
        "cargo run -- --bogus",
        "error: unexpected argument '--bogus' found\n\nUsage: demo [OPTIONS]\n",
        None,
    );
    check_cards(
        "a cargo build that warned is no card",
        "cargo build",
        "warning: unused variable: `x`\n --> src/main.rs:2:9\n    Finished `dev` profile\n",
        None,
    );
}

// cards_test.go: TestTestCards' cargo and nextest rows (a06a610c).
#[test]
fn cargo_test_cards() {
    check_cards(
        "cargo",
        "cargo test",
        "test tests::a ... ok\ntest tests::b ... FAILED\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured\n",
        Some(("tests", "test", (1, 1, 0), vec![("tests::b", "", false)])),
    );
    check_cards(
        "cargo, what it panicked with",
        "cargo test 2>&1 | tail -40",
        "running 3 tests\ntest tests::a ... ok\ntest tests::b ... FAILED\ntest tests::c ... FAILED\n\nfailures:\n\n---- tests::b stdout ----\nsetting up\nthread 'tests::b' panicked at src/lib.rs:12:9:\nassertion `left == right` failed\n  left: 1\n right: 2\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n---- tests::c stdout ----\nthread 'tests::c' (8812) panicked at src/net/mod.rs:40:5:\nno route\n\n\nfailures:\n    tests::b\n    tests::c\n\ntest result: FAILED. 1 passed; 2 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\nerror: test failed, to rerun pass `--lib`\n",
        Some((
            "tests",
            "test",
            (2, 1, 1),
            vec![
                (
                    "tests::b",
                    "lib.rs:12: assertion `left == right` failed\n  left: 1\n right: 2",
                    false,
                ),
                ("tests::c", "mod.rs:40: no route", false),
            ],
        )),
    );
    check_cards(
        "cargo, before 1.73",
        "cargo test",
        "test it_adds ... FAILED\n\nfailures:\n\n---- it_adds stdout ----\nthread 'it_adds' panicked at 'assertion failed: 1 + 1 == 3', tests/add.rs:3:5\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored\n",
        Some((
            "tests",
            "test",
            (1, 0, 0),
            vec![("it_adds", "add.rs:3: assertion failed: 1 + 1 == 3", false)],
        )),
    );
    check_cards(
        "a doctest",
        "cargo test --doc",
        "test src/lib.rs - add (line 5) ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored\n",
        Some((
            "tests",
            "test",
            (1, 0, 0),
            vec![("src/lib.rs - add (line 5)", "", false)],
        )),
    );
    check_cards(
        "cargo, didn't compile",
        "cargo test -p api",
        "   Compiling api v0.1.0\nerror[E0599]: no method named `send` found for struct `Conn`\n  --> crates/api/src/lib.rs:20:14\n   |\nerror: could not compile `api` (lib test) due to 1 previous error\n",
        Some((
            "tests",
            "test",
            (1, 0, 0),
            vec![(
                "api",
                "lib.rs:20: no method named `send` found for struct `Conn`",
                true,
            )],
        )),
    );
    check_cards(
        "nextest",
        "cargo nextest run --workspace",
        "    Starting 3 tests across 1 binary\n        PASS [   0.004s] demo tests::a\n        FAIL [   0.005s] demo tests::b\n──── STDOUT:             demo tests::b\n\nrunning 1 test\ntest tests::b ... FAILED\n──── STDERR:             demo tests::b\nthread 'tests::b' panicked at src/lib.rs:12:9:\nassertion failed: ok\nnote: run with `RUST_BACKTRACE=1`\n\n     Summary [   0.006s] 3 tests run: 2 passed, 1 failed, 0 skipped\n        FAIL [   0.005s] demo tests::b\nerror: test run failed\n",
        Some((
            "tests",
            "test",
            (1, 2, 0),
            vec![("tests::b", "lib.rs:12: assertion failed: ok", false)],
        )),
    );
    check_cards(
        "nextest, newer",
        "cargo nextest r",
        "        FAIL [   0.005s] (2/3) demo::bin/demo tests::b\n  stderr ───\n--- STDERR: demo::bin/demo tests::b ---\nthread 'tests::b' panicked at src/main.rs:7:5:\nboom\n\n     Summary [   0.006s] 3 tests run: 2 passed, 1 timed out\n",
        Some((
            "tests",
            "test",
            (1, 2, 0),
            vec![("tests::b", "main.rs:7: boom", false)],
        )),
    );
}
