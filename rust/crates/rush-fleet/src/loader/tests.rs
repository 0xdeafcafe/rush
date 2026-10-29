//! Port of internal/fleet/fleet_test.go, follows_test.go, location_test.go,
//! idle_rows_test.go, skippast_test.go, spawn_test.go, fresh_test.go,
//! compact_test.go and stuck_test.go: the loader's rows, read through fake
//! adapters, fake hosts and made-up process tables.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rush_claude::{Account, Claude};
use rush_core::adapter::{
    Adapter, Compacter, Compaction, Discoverer, Features, Kind, Level, Profile, Programmer,
    Registry, Session,
};
use rush_core::agent::{Agent, State, SubagentTile};
use rush_core::time::{Ms, now};
use rush_sys::proc::{Pid, Proc};

use super::*;
use crate::hosted::{Away, HostInfo, Hosts};
use crate::{Predicates, shell_cmd};

const MINUTE: Ms = 60_000;

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn proc_at(pid: Pid, ppid: Pid, comm: &str, start: Ms) -> Proc {
    Proc {
        pid,
        ppid,
        comm: Arc::from(comm),
        start: UNIX_EPOCH + Duration::from_millis(start.max(0) as u64),
        ..Proc::default()
    }
}

/// An agent of kind k whose program is called k, and which lists one past
/// session and one running when it has a folder.
struct Fake {
    kind: &'static str,
    dir: String,
    /// What its compaction reads: a file holding the window, and how many
    /// times it was read.
    window: Option<(PathBuf, Arc<Mutex<u32>>)>,
}

impl Adapter for Fake {
    fn kind(&self) -> Kind {
        Kind::from(self.kind)
    }
    fn name(&self) -> String {
        self.kind.into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        if self.dir.is_empty() {
            return Vec::new();
        }
        vec![Profile {
            kind: self.kind(),
            name: "past".into(),
            dir: self.dir.clone(),
        }]
    }
    fn discoverer(&self) -> Option<&dyn Discoverer> {
        (!self.dir.is_empty()).then_some(self as &dyn Discoverer)
    }
    fn compacter(&self) -> Option<&dyn Compacter> {
        self.window.as_ref().map(|_| self as &dyn Compacter)
    }
    rush_core::has!(programmer: Programmer);
}

impl Programmer for Fake {
    fn program(&self) -> (String, Vec<String>) {
        (self.kind.into(), Vec::new())
    }
}

impl Discoverer for Fake {
    fn live(&self, _: &Profile) -> Vec<Session> {
        vec![Session {
            id: "11111111-live".into(),
            cwd: "/repo".into(),
            updated_at: now(),
            ..Session::default()
        }]
    }
    fn past(&self, _: &Profile) -> Vec<Session> {
        vec![Session {
            id: "22222222-past".into(),
            cwd: "/repo".into(),
            updated_at: now() - 60 * MINUTE,
            ..Session::default()
        }]
    }
}

impl Compacter for Fake {
    fn compaction(&self, _: &Profile, cwd: &str, _: &[String]) -> Compaction {
        let (f, n) = self.window.as_ref().unwrap();
        *n.lock().unwrap() += 1;
        let w = std::fs::read_to_string(Path::new(cwd).join(f)).unwrap_or_default();
        Compaction {
            window: w.trim().parse().unwrap_or(0),
            headroom: 0,
        }
    }
}

fn fake(kind: &'static str) -> Fake {
    Fake {
        kind,
        dir: String::new(),
        window: None,
    }
}

/// The hosts a test says there are.
#[derive(Clone, Default)]
struct FakeHosts(Arc<Mutex<Vec<HostInfo>>>);

impl Hosts for FakeHosts {
    fn list(&mut self, _: &dyn Fn(&Path, u64) -> bool, _: u64) -> Arc<Vec<HostInfo>> {
        Arc::new(self.0.lock().unwrap().clone())
    }
    fn config(&self, _: &str) -> Option<crate::HostConfig> {
        None
    }
}

/// A process table a test makes up.
struct FakeProcs(Vec<Proc>);

impl ProcSource for FakeProcs {
    fn snapshot(&mut self) -> Table {
        table(self.0.clone())
    }
    fn args(&self, _: Pid) -> Vec<String> {
        Vec::new()
    }
    fn env(&self, _: Pid) -> Vec<String> {
        Vec::new()
    }
}

fn loader() -> Loader {
    Loader::new(Registry::new())
}

fn info(id: &str, state: &str, cwd: &str, updated: Ms) -> HostInfo {
    HostInfo {
        id: id.into(),
        kind: "unknown-test".into(),
        state: state.into(),
        cwd: cwd.into(),
        updated_at: updated,
        ..HostInfo::default()
    }
}

fn test_profile() -> Profile {
    Profile {
        name: "test".into(),
        ..Profile::default()
    }
}

#[test]
fn session_status_beats_a_stale_job_file() {
    let t = now();
    let sess = |status: &str, at: Ms| Session {
        status: status.into(),
        status_at: at,
        ..Session::default()
    };
    let mut ended = Agent::default();
    ended.job.updated_at = t - 20_000;
    assert_eq!(
        apply_status(&mut ended, "working", &sess("idle", t)),
        "blocked"
    );
    assert!(ended.checking);
    // The summary can be rewritten after the reply; the live flag still
    // wins.
    let mut answered = Agent::default();
    answered.job.needs = "want that?".into();
    answered.job.updated_at = t;
    assert_eq!(
        apply_status(&mut answered, "blocked", &sess("busy", t - MINUTE)),
        "working"
    );
    assert!(answered.job.needs.is_empty());
    let mut tending = Agent::default();
    tending.job.in_flight = 2;
    tending.job.background = vec!["shell\x00pnpm test".into()];
    assert_eq!(apply_status(&mut tending, "done", &sess("busy", t)), "done");
    let mut watching = Agent::default();
    watching.job.in_flight = 1;
    assert_eq!(
        apply_status(&mut watching, "done", &sess("busy", t)),
        "working"
    );
}

#[test]
fn needs_you_only_for_live_unseen_questions() {
    let mut asking = Agent {
        pid: 42,
        ..Agent::default()
    };
    asking.job.state = State::Blocked;
    assert!(asking.needs_you(), "a live unseen question needs you");
    asking.seen = true;
    assert!(
        !asking.needs_you() && asking.waiting(),
        "a seen question waits"
    );
    let mut gone = Agent::default();
    gone.job.state = State::Blocked;
    assert!(!gone.needs_you() && !gone.waiting());
}

#[test]
fn orphan_cmd() {
    for (cmd, want) in [
        (
            r#"/bin/zsh -c source /u/.claude/shell-snapshots/s.sh 2>/dev/null || true && eval 'haven up --agent 2>&1 | tail -8' < /dev/null && pwd -P >| /tmp/claude-781c-cwd"#,
            "haven up --agent 2>&1 | tail -8",
        ),
        (
            r#"/bin/zsh -c source x && eval 'curl -w '"'"'%{http_code}'"'"' u; echo ok' < /dev/null"#,
            "curl -w '%{http_code}' u; echo ok",
        ),
        (
            "/Users/lw/.claude/bin/thing --flag",
            "/Users/lw/.claude/bin/thing --flag",
        ),
    ] {
        assert_eq!(shell_cmd(cmd), want);
    }
    assert_eq!(
        crate::orphan_label("x /Users/u/.claude/jobs/abcdef12/run"),
        "job abcdef12"
    );
    assert_eq!(crate::orphan_label("bash"), "ended session");
}

// A row follows its transcript into a checkout or up a folder, not into
// its memory or rush's own folder.
#[test]
fn follows() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let (repo, mem) = (root.join("repo"), root.join("claude").join("memory"));
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&mem).unwrap();
    let mut l = loader();
    let t = now();
    for (cwd, dir, want) in [
        (s(&repo), s(&mem), false),           // its memory
        (s(&root.join("x")), s(&repo), true), // into a checkout
        (
            s(&root.join("notes").join("a")),
            s(&root.join("notes")),
            true,
        ), // up
        (String::new(), s(&mem), true),       // nowhere yet
    ] {
        assert_eq!(l.follows(&cwd, &dir, t), want, "follows({cwd:?}, {dir:?})");
    }
}

fn checkout(dir: &Path) {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let b = dir.file_name().unwrap().to_string_lossy();
    std::fs::write(
        dir.join(".git").join("HEAD"),
        format!("ref: refs/heads/{b}"),
    )
    .unwrap();
}

#[test]
fn hosted_location_ignores_stale_transcript_cwd() {
    let d = tempfile::tempdir().unwrap();
    let (current, old) = (d.path().join("current"), d.path().join("old"));
    checkout(&current);
    checkout(&old);
    let mut l = loader();
    let p = test_profile();
    let i = HostInfo {
        id: "test".into(),
        cwd: s(&current),
        kind: "test-harness".into(),
        ..HostInfo::default()
    };
    l.spend.insert(
        "test/a:test".into(),
        Spend {
            dir: s(&old),
            ..Spend::default()
        },
    );
    let a = l.hosted_agent(&p, &i, None, now());
    assert_eq!(
        (a.job.cwd.as_str(), a.repo.as_str(), a.branch.as_str()),
        (s(&current).as_str(), s(&current).as_str(), "current")
    );
}

#[test]
fn hosted_location_follows_newer_transcript_cd() {
    let d = tempfile::tempdir().unwrap();
    let (started, moved) = (d.path().join("started"), d.path().join("moved"));
    checkout(&started);
    checkout(&moved);
    let t = now();
    let mut l = loader();
    let p = test_profile();
    // The agent's cwd stays where it started; the transcript saw a cd.
    let i = HostInfo {
        id: "test".into(),
        cwd: s(&started),
        cwd_at: t - 60 * MINUTE,
        kind: "test-harness".into(),
        ..HostInfo::default()
    };
    l.spend.insert(
        "test/a:test".into(),
        Spend {
            dir: s(&moved),
            dir_at: t,
            ..Spend::default()
        },
    );
    let a = l.hosted_agent(&p, &i, None, t);
    assert_eq!(
        (a.job.cwd.as_str(), a.branch.as_str()),
        (s(&moved).as_str(), "moved")
    );
}

#[test]
fn job_cwd_uses_worktree_without_losing_subdirectory() {
    for (cwd, want) in [
        ("/src/main", "/work/fix"),
        ("/work/fix/web", "/work/fix/web"),
        ("/work/fix-other", "/work/fix"),
    ] {
        assert_eq!(job_cwd(cwd, "/work/fix"), want, "{cwd}");
    }
}

#[test]
fn main_checkout_does_not_cache_missing_repository() {
    let d = tempfile::tempdir().unwrap();
    let root = s(d.path());
    let mut seen = HashMap::new();
    assert_eq!(main_checkout_in(&root, &mut seen), "");
    std::fs::create_dir(d.path().join(".git")).unwrap();
    assert_eq!(main_checkout_in(&root, &mut seen), root);
}

#[test]
fn stopped_rows_refresh_host_state_and_overlay() {
    let mut l = loader();
    let p = test_profile();
    let t = now();
    let d = tempfile::tempdir().unwrap();
    let mut i = info("idle", "stopped", &s(d.path()), t);
    i.name = "Original".into();
    let mut a = l.hosted_agent(&p, &i, None, t);
    a.display_name = "caller mutation".into();
    let key = "test/a:idle".to_string();
    l.store_mut()
        .overlay
        .names
        .insert(key.clone(), "Renamed".into());
    assert_eq!(
        l.hosted_agent(&p, &i, None, t + 1000).display_name,
        "Renamed"
    );
    l.store_mut().overlay.names.remove(&key);
    l.spend.insert(
        key.clone(),
        Spend {
            cost: 42.0,
            ready: true,
            ..Spend::default()
        },
    );
    let got = l.hosted_agent(&p, &i, None, t + 2000);
    assert!(
        got.display_name == "Original" && got.spend.cost == 42.0,
        "{got:?}"
    );
    i.updated_at = t + 3000;
    i.name = "Changed by host".into();
    assert_eq!(l.hosted_agent(&p, &i, None, t + 3000).display_name, i.name);
    i.state = "working".into();
    i.detail = "running a task".into();
    let got = l.hosted_agent(&p, &i, None, t + 4000);
    assert!(got.job.state == State::Working && got.job.detail == i.detail);
    assert!(l.idle_rows.is_empty(), "live row retained in idle cache");
}

#[test]
fn stopped_rows_expire_and_quick_load_does_not_fill_cache() {
    let mut l = loader();
    let p = test_profile();
    let t = now();
    let d = tempfile::tempdir().unwrap();
    let mut i = info("idle", "stopped", &s(d.path()), t);
    i.name = "Original".into();
    l.quick = true;
    l.hosted_base(&p, &i, None, t);
    assert!(l.idle_rows.is_empty(), "quick incomplete reading cached");
    l.quick = false;
    l.hosted_base(&p, &i, None, t);
    i.name = "updated external metadata".into();
    assert_eq!(
        l.hosted_base(&p, &i, None, t + PAST_EVERY).display_name,
        i.name
    );
}

// A sleeping host's pid is the one it had: a load that doesn't sample
// processes mustn't take it as running.
#[test]
fn sleeping_row_has_no_process() {
    let mut l = loader();
    let d = tempfile::tempdir().unwrap();
    let mut i = info("zz", "idle", &s(d.path()), now());
    i.sleeping = true;
    i.host_pid = 1;
    assert_eq!(l.hosted(&test_profile(), &i, None, now()).pid, 0);
}

// A new agent idle before its first turn has nothing finished: it isn't
// your turn.
#[test]
fn new_hosted_agent_is_not_your_turn() {
    let mut l = loader();
    let t = now();
    let d = tempfile::tempdir().unwrap();
    let mut i = info("nw", "idle", &s(d.path()), t);
    i.started_at = t;
    assert!(!l.hosted(&test_profile(), &i, None, t).your_turn(t));
    i.idle_since = t;
    assert!(l.hosted(&test_profile(), &i, None, t).your_turn(t));
}

// Idle while you're away, it's told to keep going at its check-in: not
// your turn. Asking something, it still needs you.
#[test]
fn away_is_not_your_turn() {
    let mut l = loader();
    let t = now();
    let d = tempfile::tempdir().unwrap();
    let mut i = info("aw", "idle", &s(d.path()), t);
    i.started_at = t;
    i.idle_since = t;
    i.away = Some(Away {
        until: t + 60 * MINUTE,
        next: t + MINUTE,
        ..Away::default()
    });
    let a = l.hosted(&test_profile(), &i, None, t);
    assert!(
        !a.your_turn(t) && a.job.detail.starts_with("✈ away 1h00m"),
        "{:?}",
        a.job.detail
    );
    i.state = "blocked".into();
    assert!(
        !l.hosted(&test_profile(), &i, None, t).seen,
        "a question while away is seen"
    );
    // Over, it's your turn, and says what's waiting for you.
    i.state = "idle".into();
    i.detail = "All done: shipped".into();
    if let Some(w) = &mut i.away {
        w.ended = t;
        w.held = 1;
    }
    let a = l.hosted(&test_profile(), &i, None, t);
    assert!(a.your_turn(t), "back: not your turn");
    assert_eq!(
        a.job.detail,
        "✈ back from away · 1 saved for you · All done: shipped"
    );
    i.away = Some(Away {
        looping: true,
        next: t,
        ..Away::default()
    });
    let a = l.hosted(&test_profile(), &i, None, t);
    assert!(
        !a.your_turn(t) && a.job.detail.starts_with("⟳ loop · next check-in"),
        "{}",
        a.job.detail
    );
}

// A turn that ended with decisions waits on you, its host asleep or not.
#[test]
fn decisions_need_you() {
    let mut l = loader();
    let t = now();
    let d = tempfile::tempdir().unwrap();
    let mut i = info("dc", "idle", &s(d.path()), t);
    i.sleeping = true;
    i.started_at = t;
    i.idle_since = t;
    i.decisions = Some(vec!["Scope".into(), "Go".into()]);
    let a = l.hosted(&test_profile(), &i, None, t);
    assert!(
        a.needs_you() && a.job.needs == "2 decisions" && a.pid == 0,
        "{a:?}"
    );
    assert_eq!(a.decisions_key, "Scope\x1fGo\x1f");
}

fn claude_reg(dir: &Path) -> (Registry, Profile) {
    let reg = Registry::new();
    reg.register_ref(Arc::new(Claude::at(Account::at("test", dir))));
    let p = reg.get("claude").unwrap().profiles().remove(0);
    (reg, p)
}

// An idle row's transcript isn't asked to be watched; a working row's is.
#[test]
fn idle_rows_are_not_watched() {
    let d = tempfile::tempdir().unwrap();
    let (reg, p) = claude_reg(d.path());
    let tr = reg.transcript_path("claude", &p, "/work/app", "sid1");
    std::fs::create_dir_all(Path::new(&tr).parent().unwrap()).unwrap();
    std::fs::write(&tr, "{}\n").unwrap();
    let mut l = Loader::new(reg);
    let t = now();
    let mut i = info("w", "stopped", "/work/app", t);
    i.kind = "claude".into();
    i.session_id = "sid1".into();
    assert_eq!(l.hosted_base(&p, &i, None, t).job.transcript_path, tr);
    assert!(
        !l.sub_watch.contains(&PathBuf::from(&tr)),
        "an idle row's transcript is watched"
    );
    i.state = "working".into();
    l.hosted_base(&p, &i, None, t);
    assert!(
        l.sub_watch.contains(&PathBuf::from(&tr)),
        "a working row's isn't"
    );
}

/// A loader reading one fake agent with a past and a live session, home
/// being Claude Code in a folder of its own.
fn past_loader(d: &Path) -> Loader {
    let (reg, _) = claude_reg(&d.join("claude"));
    reg.register_ref(Arc::new(Fake {
        kind: "pastfake",
        dir: s(&d.join("past")),
        window: None,
    }));
    let mut l = Loader::new(reg);
    l.set_procs(Box::new(FakeProcs(Vec::new())));
    l
}

fn of_kind(snap: &Snapshot, k: &str) -> Vec<Agent> {
    snap.agents
        .iter()
        .filter(|a| a.kind.as_str() == k)
        .cloned()
        .collect()
}

// SkipPast leaves past sessions out of a reading, running ones in, and
// the next reading after it's turned off has them again.
#[test]
fn skip_past_leaves_past_sessions_out() {
    let d = tempfile::tempdir().unwrap();
    let mut l = past_loader(d.path());
    l.watch(3_600_000);
    let count = |l: &mut Loader| {
        let rows = of_kind(&l.load(now()), "pastfake");
        (
            rows.iter().filter(|a| a.past).count(),
            rows.iter().filter(|a| a.interactive).count(),
        )
    };
    l.handle().skip_past(true);
    assert_eq!(count(&mut l), (0, 1));
    l.handle().skip_past(false);
    assert_eq!(count(&mut l), (1, 1));
}

// An agent hidden in the overlay is in no reading, however it's found.
#[test]
fn hidden_agents_are_in_no_reading() {
    let d = tempfile::tempdir().unwrap();
    let mut l = past_loader(d.path());
    let keys = |l: &mut Loader| -> Vec<String> {
        of_kind(&l.load(now()), "pastfake")
            .into_iter()
            .map(|a| a.key)
            .collect()
    };
    let all = keys(&mut l);
    assert_eq!(all.len(), 2, "{all:?}");
    l.store_mut().hide(&all[0]);
    let got = keys(&mut l);
    assert!(
        got.len() == 1 && got[0] != all[0],
        "hidden {} still listed: {got:?}",
        all[0]
    );
}

// A hidden row is still read, so what counted its subagents is kept from
// one reading to the next.
#[test]
fn hidden_row_keeps_its_subagents() {
    let d = tempfile::tempdir().unwrap();
    let (reg, p) = claude_reg(d.path());
    let tr = reg.transcript_path("claude", &p, "/work/app", "sid1");
    let subs = Path::new(tr.strip_suffix(".jsonl").unwrap()).join("subagents");
    std::fs::create_dir_all(&subs).unwrap();
    std::fs::write(&tr, "{}\n").unwrap();
    std::fs::write(subs.join("agent-a1.meta.json"), r#"{"toolUseId":"t1"}"#).unwrap();
    std::fs::write(subs.join("agent-a1.jsonl"), "{}\n").unwrap();
    let hosts = FakeHosts::default();
    let mut i = info("h1", "working", "/work/app", now());
    i.kind = "claude".into();
    i.session_id = "sid1".into();
    hosts.0.lock().unwrap().push(i);
    let mut l = Loader::new(reg);
    l.set_hosts(Box::new(hosts));
    l.set_procs(Box::new(FakeProcs(Vec::new())));
    let key = format!("{}/a:h1", p.name);
    l.load(now());
    let e = l.subs.get(&key).expect("its runs weren't counted");
    assert!(e.runs.is_some() && e.st.spawned == 1, "{:?}", e.st);
    l.store_mut().hide(&key);
    assert!(
        l.load(now()).agents.iter().all(|a| a.key != key),
        "a hidden row is listed"
    );
    assert!(
        l.subs.get(&key).is_some_and(|e| e.runs.is_some()),
        "a hidden row's counter was dropped"
    );
}

// A print-mode run a session's shell ran is that session's: found through
// the shell between them, and counted with its subagents. One a
// session's process runs itself (a host's agent) isn't.
#[test]
fn spawn_of() {
    let tab = table(vec![
        proc_at(10, 1, "", 0),  // rush host
        proc_at(11, 10, "", 0), // its agent
        proc_at(12, 11, "", 0), // zsh -c
        proc_at(13, 12, "", 0), // claude -p
        proc_at(20, 1, "", 0),  // claude -p from a terminal
    ]);
    let mut parents: HashSet<Pid> = [10, 11, 13, 20].into();
    assert_eq!(crate::machine::spawn_of(Some(&tab), 13, &parents), 11);
    assert_eq!(crate::machine::spawn_of(Some(&tab), 11, &parents), 0);
    assert_eq!(crate::machine::spawn_of(Some(&tab), 20, &parents), 0);
    let host = Agent {
        key: "h".into(),
        pid: 10,
        ..Agent::default()
    };
    let mut l = loader();
    let got = l.fold_spawns(Some(&tab), vec![host], vec![("c".into(), 13)], &mut parents);
    assert_eq!(got[0].subs.direct, 1);
}

fn spawn_reg() -> Registry {
    let reg = Registry::new();
    reg.register(fake("claude"));
    reg.register(fake("codex"));
    reg
}

// A codex exec a session's shell ran is that session's while it runs,
// though Codex says no process: found by when its process started. Once
// finished it stays the session's.
#[test]
fn spawned_codex() {
    let at = now() - MINUTE;
    let tab = table(vec![
        proc_at(11, 1, "claude", at - 60 * MINUTE),
        proc_at(12, 11, "zsh", at),
        proc_at(13, 12, "codex", at + 1000),
        proc_at(20, 1, "codex", at - 60 * MINUTE), // codex in a terminal
    ]);
    let row = |key: &str, kind: &str| Agent {
        key: key.into(),
        kind: Kind::from(kind),
        ..Agent::default()
    };
    let mut parent = row("default/i:parent00", "claude");
    parent.pid = 11;
    parent.interactive = true;
    let mut kid = row("codex/i:kid00000", "codex");
    kid.interactive = true;
    kid.headless = true;
    kid.job.created_at = at;
    let mut term = row("codex/i:term0000", "codex");
    term.interactive = true;
    term.job.created_at = at - 60 * MINUTE;
    let mut l = Loader::new(spawn_reg());
    l.ask_inline = true;
    let got = l.fold_spawns(
        Some(&tab),
        vec![parent, kid, term],
        Vec::new(),
        &mut [11].into(),
    );
    let keys: Vec<&str> = got.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["default/i:parent00", "codex/i:term0000"]);
    assert!(
        got[0].subs.direct == 1 && got[0].subs.spawned == 1,
        "{:?}",
        got[0].subs
    );
    assert!(l.links["codex/i:kid00000"] == "default/i:parent00" && l.links_dirty);

    // Finished: no process, a past row, folded by the link.
    let mut parent = row("default/i:parent00", "claude");
    parent.past = true;
    let mut kid = row("codex/i:kid00000", "codex");
    kid.past = true;
    let got = l.fold_spawns(
        Some(&table(Vec::new())),
        vec![parent, kid.clone()],
        Vec::new(),
        &mut HashSet::new(),
    );
    assert!(
        got.len() == 1 && got[0].subs.spawned == 1 && got[0].subs.direct == 0,
        "{got:?}"
    );
    // One whose session isn't listed stays a row of its own.
    assert_eq!(
        l.fold_spawns(None, vec![kid], Vec::new(), &mut HashSet::new())
            .len(),
        1
    );
}

// A run an agent's stand-in hosted is listed with the rush session that
// ran it, running or not; one another agent ran is found by its process.
#[test]
fn hosted_spawns() {
    let rush_row = |key: &str, id: &str, pid: Pid| {
        let mut a = Agent {
            key: key.into(),
            rush: true,
            pid,
            ..Agent::default()
        };
        a.job.id = id.into();
        a
    };
    let parent = rush_row("default/a:parent00", "parent00", 0);
    let kid = rush_row("codex/a:kid00000", "kid00000", 0);
    let orphan = rush_row("codex/a:orphan00", "orphan00", 40);
    let meta = |by: &str| HashMap::from([("spawnedBy".to_string(), by.to_string())]);
    let hosted = vec![
        HostInfo {
            id: "parent00".into(),
            ..HostInfo::default()
        },
        HostInfo {
            id: "kid00000".into(),
            meta: meta("parent00"),
            ..HostInfo::default()
        },
        HostInfo {
            id: "orphan00".into(),
            meta: meta("shell"),
            ..HostInfo::default()
        },
    ];
    let mut l = loader();
    l.ask_inline = true;
    let agents = vec![parent.clone(), kid.clone(), orphan.clone()];
    let spawned = l.hosted_spawns(&hosted, &agents, Vec::new());
    assert_eq!(spawned, [(orphan.key.clone(), 40)]);
    let empty = table(Vec::new());
    let got = l.fold_spawns(Some(&empty), agents, spawned, &mut HashSet::new());
    let keys: Vec<&str> = got.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, [parent.key.as_str(), orphan.key.as_str()]);
    assert_eq!(got[0].subs.spawned, 1);
    assert!(!got[0].busy(), "a finished spawn keeps its parent busy");

    // A spawn still working keeps its idle parent out of Your turn.
    let t = now();
    let mut p2 = parent.clone();
    p2.job.state = State::Done;
    p2.job.updated_at = t;
    let mut k2 = kid.clone();
    k2.job.state = State::Working;
    let got = l.fold_spawns(
        Some(&empty),
        vec![p2.clone(), k2.clone()],
        Vec::new(),
        &mut HashSet::new(),
    );
    assert!(
        got[0].busy() && !got[0].your_turn(t),
        "parent with a working spawn"
    );

    // A spawn asking you something stays listed, so its ask can be seen.
    k2.job.state = State::Blocked;
    assert_eq!(
        l.fold_spawns(Some(&empty), vec![p2, k2], Vec::new(), &mut HashSet::new())
            .len(),
        2
    );
}

// A finished codex exec never seen running is found by its prompt in the
// transcript of the session working when it began, written just before,
// and folded from then on; one no session asked is looked for once.
#[test]
fn spawn_asked_by() {
    let d = tempfile::tempdir().unwrap();
    let at = now() - 60 * MINUTE;
    let prompt = "Make version 2 of the macOS app icon for a developer tool";
    let line = |ts: Ms, text: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"codex exec \"{text}\""}}}}]}}}}"#,
            rush_core::time::format_rfc3339(ts)
        ) + "\n"
    };
    let parent = d.path().join("parent.jsonl");
    let later = d.path().join("later.jsonl"); // says it after the run began
    std::fs::write(&parent, line(at - 2000, prompt)).unwrap();
    std::fs::write(&later, line(at + MINUTE, prompt)).unwrap();
    let mk = |key: &str, path: &Path| {
        let mut a = Agent {
            key: key.into(),
            kind: Kind::from("claude"),
            past: true,
            ..Agent::default()
        };
        a.job.transcript_path = s(path);
        a.job.created_at = at - 60 * MINUTE;
        a.job.updated_at = at + 60 * MINUTE;
        a
    };
    let mut kid = Agent {
        key: "codex/i:kid00000".into(),
        kind: Kind::from("codex"),
        past: true,
        headless: true,
        ..Agent::default()
    };
    kid.job.name = format!("{prompt} called RUSH…");
    kid.job.created_at = at;
    kid.job.updated_at = at + MINUTE;
    let mut lone = Agent {
        key: "codex/i:lone0000".into(),
        kind: Kind::from("codex"),
        past: true,
        headless: true,
        ..Agent::default()
    };
    lone.job.name = "Something nobody here ever asked for".into();
    lone.job.created_at = at;
    // Another run of the same prompt holds it too, and ran nothing.
    let mut retry = mk("codex/i:retry000", &parent);
    retry.job.name = format!("{} something else", &kid.job.name[..30]);
    retry.kind = Kind::from("codex");
    retry.headless = true;
    let rows = || {
        vec![
            retry.clone(),
            mk("default/i:later000", &later),
            mk("default/i:parent00", &parent),
            kid.clone(),
            lone.clone(),
        ]
    };
    let mut l = Loader::new(spawn_reg());
    l.ask_inline = true;
    l.fold_spawns(None, rows(), Vec::new(), &mut HashSet::new());
    l.take_in();
    assert_eq!(
        l.links.get(&kid.key).map(String::as_str),
        Some("default/i:parent00"),
        "{:?}",
        l.links
    );
    assert_eq!(l.links.get(&lone.key).map(String::as_str), Some(""));
    let got = l.fold_spawns(None, rows(), Vec::new(), &mut HashSet::new());
    assert!(got.len() == 4 && got[2].subs.spawned == 1, "{got:?}");
}

/// The loader's watcher, started.
fn watched(l: &mut Loader) -> &rush_sys::fswait::Watcher {
    l.watch(3_600_000);
    let w = l.watching.as_mut().unwrap();
    w.start();
    w.w.as_ref().unwrap()
}

fn begin(l: &mut Loader, t: Ms) {
    l.drain(t);
    l.began(t);
}

// A cache that looked at a watched file is trusted until the file
// changes, a sweep is due, or a load can't tell what changed.
#[cfg(target_os = "macos")]
#[test]
fn fresh() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("t.jsonl");
    std::fs::write(&f, "a\n").unwrap();
    let mut l = loader();
    watched(&mut l);
    let mut t = now();
    begin(&mut l, t); // the first: can't tell, sweeps
    l.watching
        .as_ref()
        .unwrap()
        .w
        .as_ref()
        .unwrap()
        .watch(&[&f]);
    let at = l.checked();
    assert!(!l.fresh(&f, at), "fresh in a sweep");
    begin(&mut l, t); // told f is newly watched
    assert!(!l.fresh(&f, at), "trusted across being newly watched");
    let at = l.checked();
    begin(&mut l, t);
    assert!(l.fresh(&f, at), "a quiet watched file isn't fresh");
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&f)
        .unwrap()
        .write_all(b"b\n")
        .unwrap();
    // The watcher hears of it before the next load drains it.
    l.watching
        .as_ref()
        .unwrap()
        .w
        .as_ref()
        .unwrap()
        .wait(None, Some(Duration::from_secs(5)));
    begin(&mut l, t);
    assert!(!l.fresh(&f, at), "trusted after a write");
    begin(&mut l, t);
    assert!(
        !l.fresh(&f, at),
        "a write missed by a load that didn't look"
    );
    let at = l.checked();
    begin(&mut l, t);
    assert!(l.fresh(&f, at), "looked at since the write, it isn't fresh");
    l.quick = true;
    assert!(!l.fresh(&f, at), "fresh in a quick load");
    l.quick = false;
    t += 30_000;
    begin(&mut l, t);
    assert!(!l.fresh(&f, at), "fresh in a sweep");
    assert!(!l.fresh(&d.path().join("unwatched"), at));
}

// A row with no subagents folder, rewound to a conversation that has one
// in the same quiet project, counts the new one's runs.
#[cfg(target_os = "macos")]
#[test]
fn subagents_follow_rewind() {
    let d = tempfile::tempdir().unwrap();
    let proj = d.path();
    let (t1, t2) = (proj.join("s1.jsonl"), proj.join("s2.jsonl"));
    std::fs::write(&t1, "{}\n").unwrap();
    std::fs::write(&t2, "{}\n").unwrap();
    let subs = proj.join("s2").join("subagents");
    std::fs::create_dir_all(&subs).unwrap();
    std::fs::write(subs.join("agent-a1.meta.json"), r#"{"toolUseId":"t1"}"#).unwrap();
    std::fs::write(subs.join("agent-a1.jsonl"), "{}\n").unwrap();
    let (reg, _) = claude_reg(&proj.join("home"));
    let mut l = Loader::new(reg);
    watched(&mut l);
    let t = now();
    let k = Kind::from("claude");
    for _ in 0..3 {
        // looked at, told newly watched, trusted
        begin(&mut l, t);
        l.subagents(&k, "k", &s(&t1), false, t);
        let paths = l.sub_watch.clone();
        l.watching
            .as_ref()
            .unwrap()
            .w
            .as_ref()
            .unwrap()
            .watch(&paths);
    }
    assert!(
        l.fresh(proj, l.subs["k"].chk),
        "the project folder isn't trusted"
    );
    begin(&mut l, t);
    assert_eq!(l.subagents(&k, "k", &s(&t2), false, t).0.spawned, 1);
}

// A transcript found is trusted while it's watched and quiet; removed, it
// isn't found there again.
#[cfg(target_os = "macos")]
#[test]
fn transcript_of_trusted() {
    let d = tempfile::tempdir().unwrap();
    let (reg, p) = claude_reg(d.path());
    let at = reg.transcript_path("claude", &p, "/work/app", "sid1");
    std::fs::create_dir_all(Path::new(&at).parent().unwrap()).unwrap();
    std::fs::write(&at, "{}\n").unwrap();
    let mut l = Loader::new(reg);
    watched(&mut l);
    let t = now();
    for _ in 0..3 {
        begin(&mut l, t);
        assert_eq!(l.transcript_of(&p, "/work/app", "sid1", t), at);
        let paths = l.sub_watch.clone();
        l.watching
            .as_ref()
            .unwrap()
            .w
            .as_ref()
            .unwrap()
            .watch(&paths);
    }
    let fk = |p: &Profile| FoundKey {
        kind: p.kind.0.clone(),
        name: p.name.clone(),
        dir: p.dir.clone(),
        cwd: "/work/app".into(),
        sid: "sid1".into(),
    };
    assert!(l.fresh(Path::new(&at), l.found[&fk(&p)].1), "not trusted");
    std::fs::remove_file(&at).unwrap();
    l.watching
        .as_ref()
        .unwrap()
        .w
        .as_ref()
        .unwrap()
        .wait(None, Some(Duration::from_secs(5)));
    l.drain(t);
    if let Some((_, c)) = l.found.get(&fk(&p)) {
        assert!(
            !l.fresh(Path::new(&at), *c),
            "a removed transcript is still trusted"
        );
    }
}

// A row's compaction is kept a while, then read again; a row no longer
// listed keeps nothing.
#[test]
fn compactions() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("window"), "400000").unwrap();
    let reads = Arc::new(Mutex::new(0));
    let reg = Registry::new();
    reg.register(Fake {
        kind: "claude",
        dir: String::new(),
        window: Some((PathBuf::from("window"), reads.clone())),
    });
    let mut l = Loader::new(reg);
    let row = || {
        let mut a = Agent {
            key: "k".into(),
            kind: Kind::from("claude"),
            past: true,
            ..Agent::default()
        };
        a.job.cwd = s(cwd.path());
        a
    };
    let t = now();
    let mut a = vec![row()];
    l.compactions(&mut a, &[], t);
    assert_eq!(a[0].compaction.window, 400_000);
    std::fs::write(cwd.path().join("window"), "300000").unwrap();
    let mut b = vec![row()];
    l.compactions(&mut b, &[], t + 1000);
    assert_eq!(b[0].compaction.window, 400_000, "kept for a while");
    l.compactions(&mut b, &[], t + COMPACT_EVERY);
    assert_eq!(b[0].compaction.window, 300_000, "read again");
    assert_eq!(*reads.lock().unwrap(), 2);
    l.compactions(&mut [], &[], t);
    assert!(l.compact.is_empty(), "a row no longer listed keeps nothing");
}

// A process's environment is read once while it's the same process.
#[test]
fn env_of_read_once_per_process() {
    let me = std::process::id() as Pid;
    if rush_sys::proc::env(me).is_empty() {
        return; // no process environments here
    }
    let start = now() - MINUTE;
    let mut l = loader();
    l.prev_tab = Some(Arc::new(table(vec![proc_at(me, 1, "x", start)])));
    assert!(l.env_of(me).is_some(), "not read");
    let st = l.prev_tab.as_ref().unwrap().procs[&me].start;
    l.envs.insert(me, (st, vec!["KEPT=1".into()]));
    assert_eq!(l.env_of(me).unwrap(), ["KEPT=1"]);
    l.prev_tab = Some(Arc::new(table(vec![proc_at(me, 1, "x", start + 1000)])));
    assert_ne!(
        l.env_of(me).unwrap(),
        ["KEPT=1"],
        "another process given the first's environment"
    );
}

#[test]
fn stuck() {
    let t = now();
    let mut a = Agent::default();
    a.job.state = State::Working;
    a.job.updated_at = t - 31 * MINUTE;
    assert!(a.stuck(t), "quiet 31m at work should be stuck");
    a.subagents = vec![SubagentTile {
        id: "s".into(),
        mod_time: t - 6 * MINUTE,
        ..SubagentTile::default()
    }];
    assert!(
        !a.stuck(t),
        "a subagent writing 6m ago means it is not stuck"
    );
    assert_eq!(a.stuck_subs(t).len(), 1, "a subagent quiet 6m is stuck");
    a.job.state = State::Done;
    a.subagents.clear();
    assert!(!a.stuck(t), "a finished agent is never stuck");
}

#[test]
fn stuck_subs_running_a_command() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("agent-s.jsonl");
    let t = now();
    let call = |id: &str| {
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{}}}}]}}}}"#
        )
    };
    let result = |id: &str| {
        format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"ok"}}]}}}}"#
        )
    };
    let write = |lines: &[String]| std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    // Each quiet time is one of its own: the answer for a mod time is kept.
    let stuck = |quiet: Ms| {
        let a = Agent {
            subagents: vec![SubagentTile {
                id: "s".into(),
                path: s(&path),
                mod_time: t - quiet,
                ..SubagentTile::default()
            }],
            ..Agent::default()
        };
        a.stuck_subs(t).len() == 1
    };
    write(&[call("a")]);
    assert!(
        !stuck(6 * MINUTE),
        "a subagent 6m into a command is not stuck"
    );
    assert!(stuck(31 * MINUTE), "a subagent 31m into a command is stuck");
    write(&[call("a"), call("b"), result("a")]);
    assert!(
        !stuck(7 * MINUTE),
        "one of two commands still running is not stuck"
    );
    write(&[call("a"), result("a")]);
    assert!(
        stuck(8 * MINUTE),
        "a subagent waiting on the model 8m is stuck"
    );
}

// One its host marked stalled is stuck at once.
#[test]
fn stalled_is_stuck() {
    let t = now();
    let mut a = Agent::default();
    a.job.state = State::Working;
    a.job.updated_at = t - 6 * MINUTE;
    assert!(!a.stuck(t), "stuck after 6m without a stall");
    a.stalled = t - 6 * MINUTE;
    assert!(a.stuck(t), "a stalled agent at work isn't stuck");
    a.job.state = State::Done;
    assert!(!a.stuck(t), "a finished agent is stuck");
}

// The machine's processes: the daemon, a spare, an agent's own, one in a
// terminal, an ended session's leftovers; each with what's under it.
#[test]
fn machine_rows() {
    let reg = spawn_reg();
    let mut tab = table(vec![
        proc_at(100, 1, "claude", 0),   // daemon
        proc_at(101, 100, "claude", 0), // spare pty host
        proc_at(102, 101, "claude", 0), // its spare, counted inside it
        proc_at(200, 1, "rush", 0),     // a rush host: an agent's own
        proc_at(201, 200, "claude", 0), // its agent: under it
        proc_at(300, 1, "claude", 0),   // in a terminal
        proc_at(400, 1, "zsh", 0),      // an ended session's shell
        proc_at(401, 400, "node", 0),
        proc_at(500, 1, "zsh", 0), // not an agent's
    ]);
    let args: HashMap<Pid, &str> = HashMap::from([
        (100, "claude daemon run"),
        (101, "claude --bg-pty-host x"),
        (102, "claude --bg-spare"),
        (201, "claude -p --output-format stream-json"),
        (300, "claude"),
        (
            400,
            "/bin/zsh -c source /u/.claude/shell-snapshots/s.sh && eval 'npm run dev' < /dev/null",
        ),
        (500, "zsh"),
    ]);
    let host = Agent {
        key: "default/a:h".into(),
        display_name: "my host".into(),
        pid: 200,
        ..Agent::default()
    };
    let mut cmdline = |p: &Proc| args.get(&p.pid).copied().unwrap_or("").to_string();
    let m = crate::machine::machine(&reg, "claude", &mut tab, None, &[host], &mut cmdline);
    let rows: Vec<(Pid, crate::Role, &str, usize)> = m
        .rows
        .iter()
        .map(|r| (r.pid, r.role, r.label.as_str(), r.procs))
        .collect();
    use crate::Role::*;
    assert_eq!(
        rows,
        [
            (300, Other, "claude (interactive)", 1),
            (100, Daemon, "daemon", 1),
            (200, Worker, "my host", 2),
            (101, Spare, "spare (pre-warmed, unclaimed)", 2),
            (400, Orphan, "ended session", 2),
        ]
    );
    assert_eq!(m.rows[4].cmd, "npm run dev");
    assert_eq!((m.spares, m.orphans), (1, 1));
}

// What the reaper ends: a process an agent's tool started, once the agent
// has exited and launchd adopted it; never one still under an agent.
#[test]
fn reaper() {
    let reg = spawn_reg();
    let t = now() - MINUTE;
    let agent = Agent {
        display_name: "a".into(),
        pid: 10,
        ..Agent::default()
    };
    let mut r = crate::Reaper::new();
    let before = table(vec![
        proc_at(10, 1, "claude", t),
        proc_at(11, 10, "zsh", t),
        proc_at(12, 11, "node", t),
    ]);
    assert!(
        r.scan(&reg, &before, std::slice::from_ref(&agent))
            .is_empty()
    );
    // The agent exited: its shell went with it, the server was adopted.
    let after = table(vec![proc_at(12, 1, "node", t)]);
    let left = r.scan(&reg, &after, std::slice::from_ref(&agent));
    assert_eq!(left.len(), 1);
    assert_eq!((left[0].pid, left[0].agent.as_str()), (12, "a"));
    // Told once.
    assert!(
        r.scan(&reg, &after, std::slice::from_ref(&agent))
            .is_empty()
    );
    // A pid reused by another process isn't the one noted.
    let mut r = crate::Reaper::new();
    r.scan(&reg, &before, std::slice::from_ref(&agent));
    let reused = table(vec![proc_at(12, 1, "node", t + 1000)]);
    assert!(r.scan(&reg, &reused, &[agent]).is_empty());
}

// Something a session deliberately left running under a rush host (a
// daemon its agent started) stays while that agent still runs.
#[test]
fn reaper_leaves_detached_while_running() {
    let reg = spawn_reg();
    let t = now() - MINUTE;
    let host = Agent {
        display_name: "h".into(),
        pid: 20,
        ..Agent::default()
    };
    let mut r = crate::Reaper::new();
    let tab = table(vec![
        proc_at(20, 1, "rush", t),
        proc_at(21, 20, "claude", t),
        proc_at(22, 21, "node", t),
    ]);
    assert!(r.scan(&reg, &tab, std::slice::from_ref(&host)).is_empty());
    // nohup'd: adopted by launchd while its agent still runs.
    let tab = table(vec![
        proc_at(20, 1, "rush", t),
        proc_at(21, 20, "claude", t),
        proc_at(22, 1, "node", t),
    ]);
    assert!(r.scan(&reg, &tab, std::slice::from_ref(&host)).is_empty());
    // Its agent gone: left behind.
    let tab = table(vec![proc_at(20, 1, "rush", t), proc_at(22, 1, "node", t)]);
    assert_eq!(r.scan(&reg, &tab, &[host]).len(), 1);
}

#[allow(dead_code)]
fn unused(_: SystemTime) {}
