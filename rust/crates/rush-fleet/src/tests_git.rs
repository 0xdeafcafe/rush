//! Port of internal/fleet/worktree_test.go, verdict_test.go, base_test.go,
//! folder_test.go and pr_test.go: real git in temp dirs, gh faked.

use std::path::{Path, PathBuf};
use std::process::Command;

use rush_core::adapter::Pr;
use rush_core::agent::{Agent, PrStatus, Workspace};
use rush_gh::ghpr::{Gh, RunError};

use crate::*;

/// git as the tests run it: an author of its own, and none of the user's
/// config (hooks, signing).
fn git_cmd(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).args(args);
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "t"),
        ("GIT_AUTHOR_EMAIL", "t@t"),
        ("GIT_COMMITTER_NAME", "t"),
        ("GIT_COMMITTER_EMAIL", "t@t"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
    ] {
        c.env(k, v);
    }
    c
}

fn run(dir: &Path, args: &[&str]) {
    let o = git_cmd(dir, args).output().unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn out(dir: &Path, args: &[&str]) -> String {
    let o = git_cmd(dir, args).output().unwrap();
    assert!(o.status.success(), "git {args:?}");
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A temp dir, its symlinks resolved (/var is /private/var on macOS).
fn temp() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().canonicalize().unwrap();
    (d, p)
}

/// The user's git config read by the library's own git calls must not
/// stop a test: these set an identity for the commits git makes itself.
fn fake_gh(root: &str, prs: &str) -> Gh {
    let body = prs.to_string();
    let gh = Gh::new(move |_: &str, _: &[&str]| -> Result<Vec<u8>, RunError> {
        Ok(body.clone().into_bytes())
    });
    gh.look_branches(root).unwrap();
    gh
}

fn agent_in(key: &str, cwd: &Path) -> Agent {
    let mut a = Agent {
        key: key.into(),
        ..Agent::default()
    };
    a.job.cwd = s(cwd);
    a
}

// A worktree goes only when every change in it is committed, and pushed
// or in main.
#[test]
fn worktree_safety() {
    let (_d, root) = temp();
    let (remote, repo) = (root.join("remote.git"), root.join("repo"));
    std::fs::create_dir_all(&repo).unwrap();
    run(&root, &["init", "-q", "--bare", &s(&remote)]);
    run(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-qm", "first"]);
    run(&repo, &["remote", "add", "origin", &s(&remote)]);
    run(&repo, &["push", "-q", "origin", "main"]);
    let wt = repo.join(".claude").join("worktrees").join("feature");
    run(&repo, &["worktree", "add", "-q", "-b", "feature", &s(&wt)]);

    let mut a = agent_in("k", &repo);
    a.spend.dirs = vec![s(&wt.join("sub"))];
    let agents = vec![a];
    let find = || {
        let mut ws = find_worktrees(&agents, None);
        assert!(
            ws.len() == 1
                && ws[0].path == s(&wt)
                && ws[0].agents.len() == 1
                && ws[0].claude
                && ws[0].branch == "feature",
            "found {ws:#?}"
        );
        ws[0].check();
        ws.remove(0)
    };

    // A new file: uncommitted.
    std::fs::write(wt.join("b.txt"), "b\n").unwrap();
    let w = find();
    assert!(!w.safe() && w.changed == 1, "uncommitted: {w:#?}");
    // Committed, not pushed.
    run(&wt, &["add", "."]);
    run(&wt, &["commit", "-qm", "b"]);
    let w = find();
    assert!(!w.safe() && w.unpushed == 1, "{w:#?}");
    assert_eq!(w.losses(), "1 commit not pushed or in main");
    assert!(
        remove_worktree(&w, false).is_err(),
        "removed one with an unpushed commit"
    );
    // Cherry-picked into main, not pushed: main keeps it, so it's safe.
    run(&repo, &["cherry-pick", "feature"]);
    let w = find();
    assert!(w.safe() && !w.pushed(), "in main: {w:#?} {}", w.losses());
    assert!(w.verdict().can_go(), "in local main: {:?}", w.verdict());
    run(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    // Pushed, and an ignored build folder: safe, and it goes.
    run(&wt, &["push", "-q", "origin", "feature"]);
    std::fs::write(wt.join(".gitignore"), "dist/\n.gitignore\n").unwrap();
    std::fs::create_dir_all(wt.join("dist")).unwrap();
    std::fs::write(wt.join("dist").join("x.js"), "x").unwrap();
    let w = find();
    assert!(w.safe() && w.size != 0, "pushed: {w:#?}");
    remove_worktree(&w, false).unwrap();
    assert!(!wt.exists(), "the worktree is still there");
    assert!(
        find_worktrees(&agents, None).is_empty(),
        "git still lists it"
    );
    run(&repo, &["rev-parse", "--verify", "-q", "feature"]); // the branch stays
}

// Forcing removes one that would lose work, after the confirmation.
#[test]
fn worktree_force() {
    let (_d, root) = temp();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-qm", "first"]);
    let wt = root.join("wt");
    run(&repo, &["worktree", "add", "-q", "-b", "x", &s(&wt)]);
    std::fs::write(wt.join("dirty.txt"), "d\n").unwrap();
    let mut ws = find_worktrees(&[agent_in("k", &wt)], None);
    assert_eq!(ws.len(), 1, "{ws:#?}");
    ws[0].check();
    assert!(!ws[0].safe() && ws[0].no_remote, "{:#?}", ws[0]);
    // Never the repository itself, nor a folder holding it, even forced.
    for (p, r) in [(&repo, &repo), (&root, &repo), (&wt, &root)] {
        let bad = Worktree {
            path: s(p),
            repo: s(r),
            ..Worktree::default()
        };
        assert!(remove_worktree(&bad, true).is_err(), "removed {}", bad.path);
    }
    assert!(repo.join("a.txt").exists(), "the repository was touched");
    remove_worktree(&ws[0], true).unwrap();
    assert!(!wt.exists(), "still there");
}

// A checkout something hollowed out loses nothing: the tidy-up removes
// it. An edit beside the deletions still keeps it.
#[test]
fn worktree_deleted_files() {
    let (_d, root) = temp();
    let (remote, repo) = (root.join("remote.git"), root.join("repo"));
    std::fs::create_dir_all(&repo).unwrap();
    run(&root, &["init", "-q", "--bare", &s(&remote)]);
    run(&repo, &["init", "-q", "-b", "main"]);
    for f in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(repo.join(f), format!("{f}\n")).unwrap();
    }
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-qm", "first"]);
    run(&repo, &["remote", "add", "origin", &s(&remote)]);
    run(&repo, &["push", "-q", "origin", "main"]);
    let wt = root.join("wt");
    run(&repo, &["worktree", "add", "-q", "-b", "hollow", &s(&wt)]);
    run(&wt, &["push", "-q", "origin", "hollow"]);
    let find = || {
        let mut ws = find_worktrees(&[agent_in("k", &wt)], None);
        assert_eq!(ws.len(), 1);
        ws[0].check();
        ws.remove(0)
    };
    std::fs::remove_file(wt.join("a.txt")).unwrap();
    std::fs::remove_file(wt.join("b.txt")).unwrap();
    std::fs::write(wt.join("c.txt"), "edited\n").unwrap();
    let w = find();
    assert!(!w.safe() && w.changed == 1 && w.deleted == 2, "{w:#?}");
    run(&wt, &["checkout", "-q", "--", "c.txt"]);
    let w = find();
    assert!(
        w.pushed() && w.changed == 0 && w.deleted == 2,
        "{w:#?} {}",
        w.losses()
    );
    tidy_worktree(&w).unwrap();
    assert!(!wt.exists());
    run(&repo, &["rev-parse", "--verify", "-q", "hollow"]);
}

// A worktree's branch knows the branch it was made from, from its
// reflog, and how far it's gone from it.
#[test]
fn worktree_base() {
    let (_d, root) = temp();
    run(&root, &["init", "-q", "-b", "main"]);
    run(&root, &["commit", "-q", "--allow-empty", "-m", "one"]);
    run(&root, &["branch", "dev"]);
    let tree = root.join("wt");
    run(
        &root,
        &["worktree", "add", "-q", "-b", "feat", &s(&tree), "dev"],
    );
    run(&tree, &["commit", "-q", "--allow-empty", "-m", "two"]);
    run(&tree, &["commit", "-q", "--allow-empty", "-m", "three"]);
    run(&root, &["commit", "-q", "--allow-empty", "-m", "on main"]);
    let f = check_folder(&s(&root), &[s(&tree)], false);
    let st = &f.trees[&s(&tree)];
    assert_eq!(
        (st.base.as_str(), st.base_ahead, st.base_behind),
        ("dev", 2, 0)
    );
    assert_eq!(f.worktrees, 1);
    assert_eq!(f.git.branch, "main");
}

/// A main checkout pushed to a bare origin, with a worktree rush made for
/// session abc (rush/abc, from main) holding one commit of its own,
/// pushed to no remote.
fn landed_repo() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
    let (d, root) = temp();
    let remote = root.join("remote.git");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run(&root, &["init", "-q", "--bare", "-b", "main", &s(&remote)]);
    run(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-qm", "first"]);
    run(&repo, &["remote", "add", "origin", &s(&remote)]);
    run(&repo, &["push", "-q", "-u", "origin", "main"]);
    let base = out(&repo, &["rev-parse", "HEAD"]);
    let wt = repo.join(".claude").join("worktrees").join("abc");
    run(&repo, &["worktree", "add", "-q", "-b", "rush/abc", &s(&wt)]);
    std::fs::write(wt.join("b.txt"), "b\n").unwrap();
    run(&wt, &["add", "."]);
    run(&wt, &["commit", "-qm", "the work"]);
    (d, repo, wt, base)
}

/// Lands the worktree's work on origin's main as GitHub's squash merge
/// would: one new commit with the same change, from another clone.
fn squash(repo: &Path) {
    let up = repo.parent().unwrap();
    let other = up.join("other");
    run(up, &["clone", "-q", &s(&up.join("remote.git")), &s(&other)]);
    std::fs::write(other.join("b.txt"), "b\n").unwrap();
    run(&other, &["add", "."]);
    run(&other, &["commit", "-qm", "the work (#9)"]);
    run(&other, &["push", "-q", "origin", "main"]);
}

/// gh says the branch's pull request #9 is in state at head.
fn merged_pr(repo: &Path, state: &str, head: &str) -> Gh {
    fake_gh(
        &s(repo),
        &format!(
            r#"[{{"number":9,"title":"t","state":"{state}","headRefName":"rush/abc","headRefOid":"{head}","isDraft":false,"statusCheckRollup":[],"reviewDecision":"","url":"u"}}]"#
        ),
    )
}

fn session(wt: &Path, repo: &Path, base: &str) -> Agent {
    let mut a = agent_in("s", wt);
    a.done = true;
    a.workspace = Some(Workspace {
        path: s(wt),
        repo: s(repo),
        branch: "rush/abc".into(),
        base: base.into(),
        target: "main".into(),
        own: true,
    });
    a
}

fn found(agents: &[Agent], gh: Option<&Gh>) -> Worktree {
    let mut ws = find_worktrees(agents, gh);
    assert_eq!(ws.len(), 1, "{ws:#?}");
    ws[0].check();
    ws.remove(0)
}

// The one rule, case by case.
#[test]
fn verdict_rule() {
    let (_d, repo, wt, base) = landed_repo();
    let a = session(&wt, &repo, &base);
    let w = found(std::slice::from_ref(&a), None);
    assert!(
        w.own && w.made == MADE_BY_RUSH && w.target == "main" && w.base == base,
        "{w:#?}"
    );
    let v = w.verdict();
    assert!(
        v.kind == VerdictKind::WouldLose && v.reason.contains("1 commit not pushed"),
        "{v:?}"
    );

    // Its pull request squash-merged: its commit is on no remote and not
    // in main as itself, but the work is.
    squash(&repo);
    let head = out(&wt, &["rev-parse", "HEAD"]);
    let gh = merged_pr(&repo, "MERGED", &head);
    let w = found(std::slice::from_ref(&a), Some(&gh));
    let v = w.verdict();
    assert!(
        v.kind == VerdictKind::CanGo && v.reason.contains("#9 was merged"),
        "{v:?}"
    );
    assert_eq!(w.finished(), (true, "pull request #9 merged".into()));

    // A commit after the merge would be lost with it.
    run(&wt, &["commit", "-q", "--allow-empty", "-m", "after"]);
    let w = found(std::slice::from_ref(&a), Some(&gh));
    assert_eq!(w.verdict().kind, VerdictKind::WouldLose);
    run(&wt, &["reset", "-q", "--hard", &head]);

    // Uncommitted files stay, merged or not.
    std::fs::write(wt.join("c.txt"), "c\n").unwrap();
    let w = found(std::slice::from_ref(&a), Some(&gh));
    let v = w.verdict();
    assert!(
        v.kind == VerdictKind::WouldLose && v.reason.contains("uncommitted"),
        "{v:?}"
    );
    std::fs::remove_file(wt.join("c.txt")).unwrap();

    // A session working in it keeps it.
    let mut busy = session(&wt, &repo, &base);
    busy.key = "b".into();
    busy.done = false;
    busy.pid = 42;
    busy.display_name = "Busy one".into();
    let w = found(&[a.clone(), busy], Some(&gh));
    assert_eq!(
        w.verdict(),
        Verdict {
            kind: VerdictKind::KeepInUse,
            reason: "Busy one is working in it".into()
        }
    );

    // Pushed with its pull request open: kept for the pull request.
    run(&wt, &["push", "-q", "origin", "rush/abc"]);
    let gh = merged_pr(&repo, "OPEN", &head);
    let w = found(std::slice::from_ref(&a), Some(&gh));
    assert_eq!(w.verdict().kind, VerdictKind::KeepPrOpen);
}

// A worktree rush didn't make (a subagent's) is judged by the same rule,
// but never goes by itself.
#[test]
fn foreign_worktree_only_by_hand() {
    let (_d, repo, _wt, _) = landed_repo();
    let sub = repo.join(".claude").join("worktrees").join("agent-x");
    run(&repo, &["worktree", "add", "-q", "-b", "feature", &s(&sub)]);
    let mut a = agent_in("k", &sub);
    a.done = true;
    let mut w = find_worktrees(&[a], None)
        .into_iter()
        .find(|x| x.path == s(&sub))
        .unwrap();
    w.check();
    assert!(
        !w.own && w.made == MADE_BY_SUBAGENT && w.verdict().can_go(),
        "{w:#?} {:?}",
        w.verdict()
    );
    assert_eq!(settle(&w), Ok(Vec::new()));
    assert!(sub.exists(), "removed one not rush's");
}

// Work that landed: the worktree goes, its branch with it, and the main
// checkout, clean and on its target, is fast-forwarded.
#[test]
fn settle_landed() {
    let (_d, repo, wt, base) = landed_repo();
    squash(&repo);
    let head = out(&wt, &["rev-parse", "HEAD"]);
    let mut w = Worktree {
        path: s(&wt),
        repo: s(&repo),
        branch: "rush/abc".into(),
        target: "main".into(),
        base,
        own: true,
        agents: vec!["s".into()],
        pr: Some(PrStatus {
            number: 9,
            state: "merged".into(),
            head_oid: head,
            ..PrStatus::default()
        }),
        ..Worktree::default()
    };
    let (ok, why) = w.landing();
    assert!(ok && !why.is_empty(), "landing: {ok} {why:?}");
    let did = settle(&w).unwrap();
    assert_eq!(did.len(), 3, "{did:?}");
    assert!(!wt.exists(), "worktree still there");
    assert_eq!(out(&repo, &["branch", "--list", "rush/abc"]), "");
    assert_eq!(
        out(&repo, &["rev-parse", "HEAD"]),
        out(&repo, &["rev-parse", "origin/main"])
    );
    assert_eq!(out(&repo, &["log", "-1", "--format=%s"]), "the work (#9)");

    // Its branch merged locally, no pull request: landed on its target
    // too; with nothing of its own yet, it hasn't.
    let fresh = repo.join(".claude").join("worktrees").join("def");
    run(
        &repo,
        &["worktree", "add", "-q", "-b", "rush/def", &s(&fresh)],
    );
    let head = out(&repo, &["rev-parse", "HEAD"]);
    let mut f = Worktree {
        path: s(&fresh),
        repo: s(&repo),
        branch: "rush/def".into(),
        target: "main".into(),
        base: head,
        own: true,
        ..Worktree::default()
    };
    assert!(!f.landing().0, "a worktree with nothing done has landed");
    run(
        &fresh,
        &["commit", "-q", "--allow-empty", "-m", "local work"],
    );
    run(&repo, &["merge", "-q", "--ff-only", "rush/def"]);
    assert_eq!(f.landing(), (true, "its branch is on main".into()));
}

// A worktree moved on to a newer target with nothing of its own hasn't
// landed: it has no work.
#[test]
fn reset_to_newer_target_has_not_landed() {
    let (_d, repo, wt, base) = landed_repo();
    squash(&repo); // someone else's commit on origin's main
    run(&wt, &["fetch", "-q", "origin"]);
    run(&wt, &["reset", "-q", "--hard", "origin/main"]);
    let mut w = Worktree {
        path: s(&wt),
        repo: s(&repo),
        branch: "rush/abc".into(),
        target: "main".into(),
        base,
        own: true,
        ..Worktree::default()
    };
    assert!(!w.landing().0, "reset to a newer main landed");
    run(&wt, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    run(&wt, &["push", "-q", "origin", "HEAD:main"]);
    assert_eq!(w.landing(), (true, "its branch is on main".into()));
}

// git-clean's branches by the same rule: squash-merged ones too.
#[test]
fn landed_branches_rule() {
    let (_d, repo, wt, _) = landed_repo();
    let head = out(&wt, &["rev-parse", "HEAD"]);
    run(&repo, &["worktree", "remove", "--force", &s(&wt)]);
    run(&repo, &["branch", "plain"]); // on main as it is
    squash(&repo);
    run(&repo, &["fetch", "-q", "origin"]);
    let prs = rush_gh::ghpr::parse_branches(
        format!(
            r#"[{{"number":9,"state":"MERGED","headRefName":"rush/abc","headRefOid":"{head}"}}]"#
        )
        .as_bytes(),
    )
    .unwrap()
    .into_iter()
    .map(|(k, v)| (k, from_status(v)))
    .collect();
    assert_eq!(
        landed_branches(&s(&repo), "main", &prs),
        ["plain", "rush/abc"]
    );
    assert_eq!(
        landed_branches(&s(&repo), "main", &Default::default()),
        ["plain"]
    );
}

// A session's pull request is the one GitHub has for its workspace's
// branch, whichever agent runs it, in place of its agent's links.
#[test]
fn pr_matched_by_branch() {
    let (_d, root) = temp();
    let r = s(&root);
    let gh = fake_gh(
        &r,
        r#"[{"number":7,"title":"t","state":"OPEN","headRefName":"rush/abc","headRefOid":"x","isDraft":false,"statusCheckRollup":[],"reviewDecision":"APPROVED","url":"u7"}]"#,
    );
    let linked = |n: i64, state: &str, review: &str| Pr {
        number: n,
        state: state.into(),
        review: review.into(),
        ..Pr::default()
    };
    let mut a = Agent {
        root: r.clone(),
        repo: format!("{r}/.claude/worktrees/abc"),
        workspace: Some(Workspace {
            branch: "rush/abc".into(),
            ..Workspace::default()
        }),
        prs: vec![linked(3, "OPEN", "")],
        ..Agent::default()
    };
    find_pr(&mut a, Some(&gh));
    let p = a.pr.as_ref().unwrap();
    assert!(p.number == 7 && p.review == "approved" && a.prs.len() == 1 && a.prs[0].number == 7);
    // No workspace, but in a linked worktree: its branch.
    let mut b = Agent {
        root: r.clone(),
        repo: format!("{r}/elsewhere"),
        branch: "rush/abc".into(),
        ..Agent::default()
    };
    find_pr(&mut b, Some(&gh));
    assert_eq!(b.pr.as_ref().map(|p| p.number), Some(7));
    // In the main checkout its branch is everyone's: its agent's link
    // stands.
    let mut c = Agent {
        root: r.clone(),
        repo: r.clone(),
        branch: "rush/abc".into(),
        prs: vec![linked(3, "MERGED", "APPROVED")],
        ..Agent::default()
    };
    find_pr(&mut c, Some(&gh));
    let p = c.pr.as_ref().unwrap();
    assert!(
        p.number == 3 && p.state == "merged" && p.review == "approved",
        "{p:?}"
    );
}

// A folder is its repository's main checkout: git says its branch, how
// far it is from its upstream and what's uncommitted, and the same of each
// worktree an agent is in.
#[test]
fn check_folder_states() {
    let (_d, root) = temp();
    let (remote, repo) = (root.join("remote.git"), root.join("repo"));
    std::fs::create_dir_all(&repo).unwrap();
    run(&root, &["init", "-q", "--bare", &s(&remote)]);
    run(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-qm", "first"]);
    run(&repo, &["remote", "add", "origin", &s(&remote)]);
    run(&repo, &["push", "-q", "-u", "origin", "main"]);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-qm", "second"]);
    std::fs::write(repo.join("c.txt"), "c\n").unwrap();
    std::fs::write(repo.join(".git").join("info").join("exclude"), ".claude/\n").unwrap();
    let wt = repo.join(".claude").join("worktrees").join("feature");
    run(&repo, &["worktree", "add", "-q", "-b", "feature", &s(&wt)]);
    let other = root.join("other");
    run(&repo, &["worktree", "add", "-q", "-b", "other", &s(&other)]);
    std::fs::write(wt.join("d.txt"), "d\n").unwrap();
    std::fs::write(wt.join("e.txt"), "e\n").unwrap();

    assert_eq!(main_checkout(&s(&wt.join("sub"))), s(&repo));
    let at = |r: &Path| Agent {
        repo: s(r),
        root: s(&repo),
        ..Agent::default()
    };
    let agents = vec![at(&repo), at(&wt), at(&wt), Agent::default()];
    let wants = folder_wants(&agents);
    assert!(
        wants.len() == 1 && wants[&s(&repo)] == [s(&wt)],
        "{wants:?}"
    );

    let f = check_folder(&s(&repo), &wants[&s(&repo)], false);
    let g = &f.git;
    assert!(
        g.branch == "main"
            && g.upstream
            && g.ahead == 1
            && g.behind == 0
            && g.changed == 1
            && g.err.is_empty(),
        "{g:?}"
    );
    assert_eq!(f.worktrees, 2);
    let g = &f.trees[&s(&wt)];
    assert!(
        g.branch == "feature" && !g.upstream && g.changed == 2,
        "{g:?}"
    );
    assert!(
        !f.trees.contains_key(&s(&other)),
        "checked a worktree no agent is in"
    );

    // Whole, every worktree, the remote and the last commits.
    let f = check_folder(&s(&repo), &[], true);
    assert!(f.linked.len() == 2 && f.trees.len() == 2 && f.trees[&s(&other)].branch == "other");
    assert!(f.recent.len() == 2 && f.recent[0].subject == "second" && f.recent[0].at != 0);
    assert_eq!(f.remote, s(&remote).trim_end_matches(".git"));
}

#[test]
fn short_remotes() {
    for (u, want) in [
        (
            "git@github.com:0xdeafcafe/rush.git",
            "github.com/0xdeafcafe/rush",
        ),
        (
            "https://github.com/langwatch/langwatch.git",
            "github.com/langwatch/langwatch",
        ),
        (
            "ssh://git@gitlab.example.com:22/team/app.git",
            "gitlab.example.com/team/app",
        ),
    ] {
        assert_eq!(short_remote(u), want, "{u}");
    }
}
