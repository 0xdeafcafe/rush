//! The tests of rush-ui-projects-machine's tidydone.rs that need the whole UI.

use crate::model::Model;
use rush_core::agent::Agent;
use rush_core::time::Ms;
use rush_fleet::Worktree;
use rush_ui_projects_machine::data::*;
use rush_ui_projects_machine::tidydone::*;
use std::collections::HashMap;

fn wt(path: &str, repo: &str, own: bool, agents: &[&str]) -> Worktree {
    let mut w = Worktree::default();
    w.path = path.into();
    w.repo = repo.into();
    w.own = own;
    w.agents = agents.iter().map(|s| s.to_string()).collect();
    w
}

// cleanup.go:316: only rush's own, all its sessions due, past the
// newest three finished in its repository; one whose session is gone
// goes first.
#[test]
fn tidyable_keeps_the_newest_three_and_others_worktrees() {
    let wts = vec![
        wt("/r/.claude/worktrees/a", "/r", true, &["a"]),
        wt("/r/.claude/worktrees/b", "/r", true, &["b"]),
        wt("/r/.claude/worktrees/c", "/r", true, &["c"]),
        wt("/r/.claude/worktrees/d", "/r", true, &["d"]),
        wt("/r/.claude/worktrees/e", "/r", true, &["e"]),
        wt("/r/elsewhere", "/r", false, &["f"]),
        wt("/r/.claude/worktrees/gone", "/r", true, &[]),
        wt("/r/.claude/worktrees/w", "/r", true, &["w"]),
    ];
    let due: HashMap<String, bool> = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|k| (k.to_string(), *k != "b"))
        .collect();
    let finished: HashMap<String, Ms> =
        [("a", 1), ("b", 2), ("c", 5), ("d", 4), ("e", 3), ("f", 1)]
            .iter()
            .map(|(k, t)| (k.to_string(), *t))
            .collect();
    let gone = |w: &Worktree| w.path.ends_with("/gone");
    let got: Vec<&str> = tidyable(&wts, &due, &finished, &gone)
        .into_iter()
        .map(|i| wts[i].path.as_str())
        .collect();
    // c d e are the newest three; b isn't due; f isn't rush's; w works;
    // the one whose session is gone is the oldest of all.
    assert_eq!(got, ["/r/.claude/worktrees/a", "/r/.claude/worktrees/gone"]);
    let finished: HashMap<String, Ms> = HashMap::new();
    let got: Vec<&str> = tidyable(&wts, &due, &finished, &gone)
        .into_iter()
        .map(|i| wts[i].path.as_str())
        .collect();
    assert!(
        got.is_empty(),
        "the gone one is within the newest three: {got:?}"
    );
}

fn agent(key: &str) -> Agent {
    Agent {
        key: key.into(),
        display_name: key.into(),
        rush: true,
        job: rush_core::agent::Job {
            id: key.into(),
            state: rush_core::agent::State::Done,
            ..Default::default()
        },
        workspace: Some(rush_core::agent::Workspace {
            path: format!("/r/.claude/worktrees/{key}"),
            repo: "/r".into(),
            branch: format!("rush/{key}"),
            target: "main".into(),
            own: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn model(follows: bool) -> Model {
    let mut m = crate::new_model("t");
    m.follows = follows;
    m.agents = vec![agent("a")];
    m.roots = vec!["/r".into()];
    let d = m.kept::<Data>();
    d.cfg_read = true;
    m
}

// Only the rush that leads tidies done work or settles landed work.
#[test]
fn only_the_lead_tidies_and_settles() {
    let now = 10 * 3_600_000;
    let mut m = model(true);
    assert!(tidy_plan(&mut m, now).is_none());
    assert!(settle_looks(&mut m, now).is_none());
    let mut m = model(false);
    assert!(tidy_plan(&mut m, now).is_some());
    assert!(tidy_plan(&mut m, now).is_none(), "one out at a time");
    let looks = settle_looks(&mut m, now).expect("looks");
    assert_eq!(looks.len(), 1);
    assert_eq!(looks[0].w.branch, "rush/a");
}

// One asking you something, or with a pull request open, isn't looked
// at.
#[test]
fn settle_passes_over_open_work() {
    let mut m = model(false);
    m.agents[0].prs = vec![rush_core::adapter::Pr {
        state: "OPEN".into(),
        ..Default::default()
    }];
    assert!(settle_looks(&mut m, 3_600_000).is_none());
}

// onLanded: marked done once, with the news; a worktree kept is said
// once, without its folder's prefix, a click away in Projects.
#[test]
fn landed_marks_done_and_says_so() {
    let mut m = model(false);
    let l = Landed {
        done: vec![("a".into(), "pull request #9 merged".into())],
        did: vec!["a: removed a, deleted branch rush/a".into()],
        stays: HashMap::new(),
        waits: HashMap::new(),
    };
    on_landed(&mut m, l);
    assert!(m.agents[0].done);
    assert!(
        m.said
            .iter()
            .any(|s| s.text == "work landed: a: removed a, deleted branch rush/a"),
        "{:?}",
        m.said.iter().map(|s| &s.text).collect::<Vec<_>>()
    );
    let mut m = model(false);
    let mut stays = HashMap::new();
    stays.insert(
        "a".into(),
        "a stays: a isn't safe to remove: 1 file with uncommitted changes".into(),
    );
    on_landed(
        &mut m,
        Landed {
            done: vec![("a".into(), "its branch is on main".into())],
            did: Vec::new(),
            stays,
            waits: HashMap::new(),
        },
    );
    let texts: Vec<&str> = m.said.iter().map(|s| s.text.as_str()).collect();
    assert!(
        texts.contains(
            &"kept “a”'s worktree: 1 file with uncommitted changes · open it in Projects"
        ),
        "{texts:?}"
    );
    assert!(
        texts.contains(&"work landed: 1 put away in Earlier"),
        "{texts:?}"
    );
}

// onTidied: what went and what was kept, in Go's words.
#[test]
fn tidied_says_what_went_and_what_stayed() {
    let mut m = model(false);
    on_tidied(
        &mut m,
        Tidied {
            wts: vec![wt("/r/.claude/worktrees/a", "/r", true, &["a"])],
            gone: vec!["/r/.claude/worktrees/a".into()],
            kept: vec![(
                "/r/.claude/worktrees/b".into(),
                "b isn't safe to remove: 1 commit not pushed or in main".into(),
            )],
            freed: 2 << 30,
            ..Tidied::default()
        },
    );
    let texts: Vec<&str> = m.said.iter().map(|s| s.text.as_str()).collect();
    assert!(
        texts.contains(&"kept b's worktree: 1 commit not pushed or in main · open it in Projects"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("removed the worktrees of done work: “a”")
                && t.ends_with("nothing lost, branches kept")),
        "{texts:?}"
    );
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let o = std::process::Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {:?}", o);
}

/// A repo with a worktree whose branch has landed on main, a model
/// whose session "a" is put away over it, and its look.
fn landed(name: &str) -> (tempfile::TempDir, Model, std::path::PathBuf, Vec<Look>) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let (remote, repo) = (root.join("remote.git"), root.join("repo"));
    std::fs::create_dir_all(&repo).unwrap();
    git(
        &root,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "first"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    let wt = repo.join(".claude/worktrees").join(name);
    let branch = format!("rush/{name}");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", &branch, wt.to_str().unwrap()],
    );
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&wt)
        .output()
        .unwrap();
    let base = String::from_utf8(head.stdout).unwrap().trim().to_string();
    std::fs::write(wt.join("b.txt"), "b\n").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-qm", "the work"]);
    git(&repo, &["merge", "-q", "--ff-only", &branch]);
    git(&repo, &["push", "-q", "origin", "main"]);
    let mut m = model(false);
    let mut a = agent(name);
    a.pid = 1;
    a.done = true;
    let ws = a.workspace.as_mut().unwrap();
    ws.path = wt.to_string_lossy().into_owned();
    ws.repo = repo.to_string_lossy().into_owned();
    ws.branch = branch;
    ws.base = base;
    m.agents = vec![a];
    m.roots = vec![ws_repo(&m)];
    write_info(name, "idle", false, i64::from(std::process::id()));
    let looks = settle_looks(&mut m, 10 * 3_600_000).expect("looks");
    (d, m, wt, looks)
}

/// What the host of session id last published in info.json.
fn write_info(id: &str, state: &str, sleeping: bool, pid: i64) {
    let dir = rush_host::dir(id);
    std::fs::create_dir_all(&dir).unwrap();
    let info = rush_host::Info {
        id: id.into(),
        state: state.into(),
        sleeping,
        host_pid: pid,
        ..Default::default()
    };
    std::fs::write(dir.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
}

fn ws_repo(m: &Model) -> String {
    m.agents[0].workspace.as_ref().unwrap().repo.clone()
}

fn branch_exists(repo: &str, b: &str) -> bool {
    let o = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")])
        .current_dir(repo)
        .output()
        .unwrap();
    o.status.success()
}

/// Runs one pass of the look for landed work with the session running
/// or not, and takes in what it found.
fn pass(m: &mut Model, running: bool) {
    let looks = settle_looks(m, 10 * 3_600_000);
    let Some(looks) = looks else { return };
    let l = settle_with(looks, &|_| running);
    on_landed(m, l);
}

// A landed branch of a live session: put away, but its worktree and
// branch stay; once it has stopped, the next pass removes them.
#[test]
fn landed_live_session_keeps_its_worktree_until_it_stops() {
    let (_d, mut m, wt, looks) = landed("live");
    let repo = ws_repo(&m);
    m.agents[0].done = false;
    let l = settle_with(looks, &|_| true);
    assert!(l.waits.contains_key("live"), "{:?}", l.waits.keys());
    on_landed(&mut m, l);
    assert!(m.agents[0].done, "put away at once");
    assert!(wt.exists(), "worktree removed under a running session");
    assert!(branch_exists(&repo, "rush/live"));
    // Still running on the next pass: nothing happens, and it isn't
    // said again.
    pass(&mut m, true);
    assert!(wt.exists());
    assert!(m.kept::<Tidy>().waiting.contains("live"));
    // Stopped: it goes, with its branch.
    pass(&mut m, false);
    assert!(!wt.exists(), "worktree kept after the session stopped");
    assert!(!branch_exists(&repo, "rush/live"));
    assert!(!m.kept::<Tidy>().waiting.contains("live"));
}

// A message before it stops brings it back from put away: the
// worktree is kept, even when it stops after.
#[test]
fn a_message_before_it_stops_keeps_the_worktree() {
    let (_d, mut m, wt, looks) = landed("msg");
    m.agents[0].done = false;
    on_landed(&mut m, settle_with(looks, &|_| true));
    assert!(m.agents[0].done);
    crate::screens::cmdbar::stops::undone(&mut m, "msg");
    assert!(!m.agents[0].done);
    pass(&mut m, false);
    pass(&mut m, false);
    assert!(wt.exists(), "a session you wrote to lost its worktree");
    assert!(!m.kept::<Tidy>().waiting.contains("msg"));
}

// The host's say: running while its process lives, whatever it
// published; gone once stopped with its process gone, or asleep.
#[test]
fn host_running_reads_the_host() {
    let me = i64::from(std::process::id());
    let write = write_info;
    write("hr-idle", "idle", false, me);
    write("hr-working", "working", false, me);
    write("hr-stopping", "stopped", false, me);
    write("hr-stopped", "stopped", false, 0);
    write("hr-asleep", "idle", true, 0);
    write("hr-asleep-up", "idle", true, me);
    assert!(host_running("hr-idle", 0));
    assert!(host_running("hr-working", 0));
    assert!(host_running("hr-stopping", 0), "process still there");
    assert!(!host_running("hr-stopped", 0));
    assert!(!host_running("hr-asleep", 0));
    assert!(!host_running("hr-asleep-up", 0));
    assert!(!host_running("hr-nothing-at-all", 0), "no host left");
    assert!(host_running("", std::process::id() as i32));
    assert!(!host_running("", 0));
}

// The same, end to end through the host's own files: idle and alive
// keeps the worktree, stopped removes it, asleep removes it.
#[test]
fn worktree_follows_the_hosts_state() {
    let me = i64::from(std::process::id());
    for (name, state, sleeping, pid, goes) in [
        ("e2e-idle", "idle", false, me, false),
        ("e2e-stopped", "stopped", false, 0, true),
        ("e2e-asleep", "idle", true, 0, true),
    ] {
        let (_d, mut m, wt, looks) = landed(name);
        m.agents[0].done = false;
        write_info(name, state, sleeping, pid);
        on_landed(&mut m, settle(looks));
        assert!(m.agents[0].done, "{name}: put away");
        assert_eq!(!wt.exists(), goes, "{name}");
    }
}
