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
