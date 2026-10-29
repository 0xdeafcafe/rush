//! Frames of the list and a session over fixture agents, as
//! `rush --render` draws them; nothing read from ~/.claude.
//! UPDATE_SNAPSHOTS=1 writes the snapshots again.

use std::collections::HashMap;
use std::path::PathBuf;

use rush_core::agent::{Agent, State, SubagentStats};
use rush_core::time::Ms;
use rush_draw::{Key, KeyMods};
use rush_ui::{Fleet, Model};

const NOW: Ms = 1_793_190_896_789;
const MIN: Ms = 60_000;
const HOUR: Ms = 60 * MIN;

#[allow(clippy::too_many_arguments)]
fn agent(
    id: &str,
    name: &str,
    state: State,
    pid: i32,
    cpu: f32,
    mem_mb: u64,
    cost: f64,
    ago: Ms,
) -> Agent {
    let mut a = Agent {
        key: format!("default/{id}"),
        display_name: name.into(),
        repo: "/src/checkout".into(),
        branch: "main".into(),
        pid,
        cpu,
        mem: mem_mb << 20,
        interactive: true,
        ..Agent::default()
    };
    a.job.id = id.into();
    a.job.state = state;
    a.job.cwd = "/src/checkout".into();
    a.job.created_at = NOW - ago - 20 * MIN;
    a.job.updated_at = NOW - ago;
    a.spend.cost = cost;
    a.spend.first = NOW - ago - 20 * MIN;
    a.spend.last = NOW - ago;
    a.spend.ready = true;
    a
}

fn fixture() -> Vec<Agent> {
    let mut ask = agent(
        "a1",
        "fix the flaky tax rounding test",
        State::Blocked,
        101,
        1.0,
        332,
        0.19,
        2 * MIN,
    );
    ask.job.needs = "asks: Round half up, or half to even?".into();
    let mut pay = agent(
        "b2",
        "add Apple Pay to checkout",
        State::Working,
        102,
        31.0,
        874,
        2.62,
        MIN,
    );
    pay.subs = SubagentStats {
        spawned: 1,
        direct: 1,
        nested: 0,
    };
    pay.spend.context = 140_000;
    pay.job.detail = "subagent: draft the rollout notes".into();
    pay.branch = "feat/apple-pay".into();
    let tokens = agent(
        "c3",
        "move the design tokens to CSS variables",
        State::Working,
        103,
        79.0,
        316,
        3.10,
        MIN / 2,
    );
    let idle = agent(
        "d4",
        "set up caching for CI",
        State::Idle,
        104,
        0.0,
        120,
        1.10,
        10 * MIN,
    );
    let mut past = agent(
        "e5",
        "why does the ledger import time out on big files?",
        State::Stopped,
        0,
        0.0,
        0,
        1.71,
        3 * HOUR,
    );
    past.past = true;
    let old = agent(
        "f6",
        "rename Invoice to Bill everywhere",
        State::Done,
        0,
        0.0,
        0,
        0.42,
        72 * HOUR,
    );
    vec![ask, pay, tokens, idle, past, old]
}

fn model() -> Model {
    // As the binary does: the list names each agent through the registry.
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| rush_core::adapter::global().register(rush_claude::Claude::new()));
    let mut m = Model::new("0.1.0");
    m.clock = Some(NOW);
    m.launch_dir = "/src/checkout".into();
    let agents = fixture();
    let roots = agents.iter().map(|a| a.repo.clone()).collect();
    m.set_fleet(Fleet {
        agents,
        roots,
        previews: HashMap::new(),
        priced: true,
    });
    m
}

fn snapshot(name: &str, got: &str) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

fn plain(m: &mut Model, w: u16, h: u16, keys: &[(Key, KeyMods)]) -> String {
    rush_ui::frame(m, w, h, keys, false)
}

#[test]
fn list_at_120() {
    let mut m = model();
    let f = plain(&mut m, 120, 40, &[]);
    snapshot("list_120x40", &f);
    let rows: Vec<&str> = f.lines().collect();
    assert!(rows[0].contains("rush   ● 1 needs you [ctrl+n] next   ✻ 2 working   3 finished"));
    assert!(rows[6].starts_with("  AGENTS▾"));
    assert!(rows[6].ends_with("RUNNING   CPU    RAM TOKENS    COST   TIME"));
    // What needs you floats to the top, picked.
    assert_eq!(rows[7].trim_end(), "┃ Needs you  1 · $0.19");
    assert!(rows[8].starts_with("▌ └─ ? fix the flaky tax rounding test ✻ Claude Co…"));
    assert!(rows[8].contains("asks: Round half up,…"));
    assert!(f.contains("┃ Working  2 · $5.72"));
    assert!(f.contains(" ▸ Earlier  1 · $0.42"));
}

#[test]
fn narrow_lists_stack_and_drop_columns() {
    let mut m = model();
    let f = plain(&mut m, 70, 30, &[]);
    snapshot("list_70x30", &f);
    let head = f.lines().nth(6).unwrap();
    assert!(!head.contains("CPU") && !head.contains("TOKENS") && !head.contains("LATEST"));
    assert!(head.contains("RAM") && head.contains("COST") && head.contains("TIME"));
    // Stacked: the question hangs under its name.
    assert!(f.contains("╰ asks: Round half up"));
    let f = plain(&mut m, 50, 24, &[]);
    snapshot("list_50x24", &f);
    assert!(f.lines().next().unwrap().contains("RUSH"));
}

#[test]
fn keys_move_and_fold() {
    let mut m = model();
    let down = (Key::Down, KeyMods::NONE);
    plain(&mut m, 120, 40, &[down]);
    assert_eq!(m.sel, "§Working");
    plain(&mut m, 120, 40, &[down, down]);
    assert_eq!(m.sel, "default/c3");
    // Enter on a heading folds it.
    let up = (Key::Up, KeyMods::NONE);
    let enter = (Key::Enter, KeyMods::NONE);
    let f = plain(&mut m, 120, 40, &[up, up, enter]);
    assert!(f.contains("▸ Working  2 · $5.72"), "{f}");
    // ctrl+n goes to what needs you.
    plain(&mut m, 120, 40, &[(Key::Char('n'), KeyMods::CTRL)]);
    assert_eq!(m.sel, "default/a1");
}

#[test]
fn opens_a_session_read_only() {
    let dir = std::env::temp_dir().join(format!("rush-ui-render-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"fix the test"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./..."}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
            r#"{"type":"user","timestamp":"2026-10-28T12:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]},"toolUseResult":{"stdout":"ok","stderr":""}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-28T12:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Fixed it."}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-10-28T12:00:13Z"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let mut m = model();
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].job.transcript_path = path.to_string_lossy().into_owned();
    m.sel = m.agents[i].key.clone();
    let f = plain(&mut m, 100, 30, &[(Key::Enter, KeyMods::NONE)]);
    assert!(m.session.is_some());
    assert!(f.contains("set up caching for CI   ·  idle"), "{f}");
    assert!(f.contains("esc back to the list"), "{f}");
    assert!(f.contains("fix the test"), "{f}");
    assert!(f.contains("Fixed it."), "{f}");
    // ↑ picks a row; esc drops it, esc again goes back to the list.
    plain(&mut m, 100, 30, &[(Key::Up, KeyMods::NONE)]);
    assert!(!m.session.as_ref().unwrap().sel.is_empty());
    let esc = (Key::Esc, KeyMods::NONE);
    plain(&mut m, 100, 30, &[esc]);
    assert!(m.session.as_ref().unwrap().sel.is_empty());
    let f = plain(&mut m, 100, 30, &[esc]);
    assert!(m.session.is_none());
    assert!(f.contains("AGENTS▾"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn columns_drop_by_width() {
    let m = model();
    // RUNNING CPU RAM TOKENS COST TIME
    let cases: &[(usize, [usize; 6])] = &[
        (120, [10, 6, 7, 7, 8, 7]),
        (96, [10, 6, 7, 7, 8, 7]),
        (95, [0, 0, 7, 7, 8, 7]),
        (76, [0, 0, 7, 7, 8, 7]),
        (75, [0, 0, 7, 0, 8, 7]),
        (63, [0, 0, 0, 0, 8, 7]),
        (43, [0, 0, 0, 0, 0, 7]),
    ];
    for &(w, want) in cases {
        assert_eq!(m.col_widths(w), want, "at {w}");
    }
    let mut m = model();
    m.cfg.hide_columns = vec!["cpu".into(), "cost".into()];
    assert_eq!(m.col_widths(120), [10, 0, 7, 7, 0, 7]);
}

#[test]
fn rows_say_what_each_agent_is_doing() {
    let mut m = model();
    plain(&mut m, 140, 40, &[]);
    let row = |m: &Model, id: &str| {
        let i = m.agents.iter().position(|a| a.job.id == id).unwrap();
        m.agent_line(i, 136, 140, false, 40, false).plain()
    };
    // marker, name, badge … RUNNING CPU RAM TOKENS COST TIME
    let cases: &[(&str, &str, &str)] = &[
        (
            "a1",
            " ? fix the flaky tax rounding test ✻",
            "1%   332M          $0.19   2m ● ",
        ),
        (
            "b2",
            " ✳ add Apple Pay to checkout ✻",
            "↳1   31%   874M   140k   $2.62    21m ",
        ),
        (
            "c3",
            " ✳ move the design tokens to CSS variables ✻",
            "79%   316M          $3.10    20m ",
        ),
        (
            "d4",
            " ◦ set up caching for CI ✻",
            "0%   120M          $1.10    10m ",
        ),
        (
            "e5",
            " · why does the ledger import time out on big files? ✻",
            "$1.71     3h ",
        ),
    ];
    for &(id, start, end) in cases {
        let r = row(&m, id);
        assert!(r.starts_with(start), "{id}: {r:?}");
        assert!(r.ends_with(end), "{id}: {r:?}");
    }
    let (s, _, _) = m.row_summary(m.agents.iter().position(|a| a.job.id == "a1").unwrap());
    assert_eq!(s, "asks: Round half up, or half to even?");
}

/// A transcript of n turns, each a question and its answer, in a temp
/// folder; the agent d4 reads it.
fn long_session(m: &mut Model, name: &str, lines: &[String]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rush-ui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].job.transcript_path = path.to_string_lossy().into_owned();
    m.sel = m.agents[i].key.clone();
    dir
}

fn turns(n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..n {
        out.push(format!(
            r#"{{"type":"user","timestamp":"2026-10-28T12:{:02}:00Z","cwd":"/src/checkout","message":{{"role":"user","content":"question {i}"}}}}"#,
            i % 60
        ));
        out.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-10-28T12:{:02}:05Z","message":{{"id":"m{i}","role":"assistant","content":[{{"type":"text","text":"answer {i}"}}]}}}}"#,
            i % 60
        ));
    }
    out
}

/// Home goes to the conversation's real top, and the view stays full;
/// end follows it again.
#[test]
fn home_and_end_in_a_long_session() {
    let mut m = model();
    let dir = long_session(&mut m, "home", &turns(80));
    let key = |k: Key| (k, KeyMods::NONE);
    let f = plain(&mut m, 100, 30, &[key(Key::Enter)]);
    assert!(f.contains("answer 79"), "{f}");
    let f = plain(&mut m, 100, 30, &[key(Key::Home)]);
    assert!(f.contains("question 0"), "{f}");
    assert!(!f.contains("answer 79"), "{f}");
    // As many rows of the conversation as before: none scrolled off the top.
    let body = |f: &str| {
        f.lines()
            .filter(|l| l.contains("question") || l.contains("answer"))
            .count()
    };
    let top = body(&f);
    assert!(top >= 5, "{f}");
    // Again: it's already there.
    let again = plain(&mut m, 100, 30, &[key(Key::Char('g'))]);
    assert_eq!(body(&again), top, "{again}");
    // A page down and back up again lands where it was.
    plain(&mut m, 100, 30, &[key(Key::PageDown)]);
    let back = plain(&mut m, 100, 30, &[key(Key::PageUp)]);
    assert!(back.contains("question 0"), "{back}");
    let f = plain(&mut m, 100, 30, &[key(Key::End)]);
    assert!(f.contains("answer 79"), "{f}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A transcript that ended mid-turn, of an agent that has stopped, doesn't
/// spin: its turn is ended (Go's followTail).
#[test]
fn a_stopped_session_ends_its_live_turn() {
    let mut m = model();
    let lines = vec![
        r#"{"type":"user","timestamp":"2026-10-28T12:00:00Z","cwd":"/src/checkout","message":{"role":"user","content":"run it"}}"#.to_string(),
        r#"{"type":"assistant","timestamp":"2026-10-28T12:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 100"}}]}}"#.to_string(),
    ];
    let dir = long_session(&mut m, "stopped", &lines);
    let i = m.agents.iter().position(|a| a.job.id == "d4").unwrap();
    m.agents[i].pid = 0;
    m.agents[i].job.state = State::Stopped;
    plain(&mut m, 100, 30, &[(Key::Enter, KeyMods::NONE)]);
    let v = m.session.as_ref().unwrap();
    assert!(v.sess.live().is_none(), "the turn still runs");
    assert_eq!(v.sess.info.cost, m.agents[i].spend.cost);
    assert!(!m.spinning());
    std::fs::remove_dir_all(&dir).unwrap();
}
