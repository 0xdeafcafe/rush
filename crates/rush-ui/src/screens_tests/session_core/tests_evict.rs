//! What a session out of sight holds of its runs, let go after
//! EVICT_AFTER (evict.rs; Rust-only): when it is and isn't, what stays,
//! and that opening it again reads them back and shows the same screen.
//! No sleeps: time is the model's now.

use std::path::Path;
use std::sync::{Arc, Weak};

use rush_convo::Session;
use rush_core::agent::{Agent, State};
use rush_core::text::Line;
use rush_core::time::Ms;

use rush_ui_base::spawns::SPAWN_PREFIX;

use super::evict::{self, EVICT_AFTER};
use super::subs;
use super::{conn, core};
use crate::model::Model;
use crate::session::{SessionView, Source};

const NOW: Ms = 1_793_190_896_789;
const MIN: Ms = 60_000;
const OPEN: &str = "default/open";
const AWAY: &str = "default/away";

fn agent(key: &str, path: &str) -> Agent {
    let mut a = Agent {
        key: key.into(),
        kind: "claude".into(),
        display_name: format!("session {key}"),
        repo: "/src/project".into(),
        ..Agent::default()
    };
    a.job.id = key.rsplit('/').next().unwrap_or(key).into();
    a.job.session_id = a.job.id.clone();
    a.job.state = State::Done;
    a.job.cwd = "/src/project".into();
    a.job.transcript_path = path.into();
    a
}

fn model(agents: Vec<Agent>) -> Model {
    let reg = rush_core::adapter::Registry::new();
    reg.register(rush_claude::Claude::new());
    let mut m = crate::new_model("0.1.0");
    m.reg = reg;
    m.clock = Some(NOW);
    m.now = NOW;
    m.w = 120;
    m.h = 40;
    m.roots = vec!["/src/project".into(); agents.len()];
    m.agents = agents;
    m
}

fn view(key: &str) -> SessionView {
    let mut v = SessionView::new(key.into(), Source::Hosted, 1);
    v.read = true;
    v
}

/// AWAY holds a hosted agent's conversation and a subagent's read for
/// Changes: weak handles to both.
fn holding(m: &mut Model) -> [Weak<Session>; 2] {
    let r = &mut conn(m, AWAY).runs;
    let (kid, sub) = (Arc::new(Session::new()), Arc::new(Session::new()));
    r.full.insert(format!("{SPAWN_PREFIX}toolu_1"), kid.clone());
    r.full.insert("a1".into(), sub.clone());
    [Arc::downgrade(&kid), Arc::downgrade(&sub)]
}

/// A session's runs are let go only once it's been out of sight for
/// EVICT_AFTER, never while it shows anywhere or a read of them is out;
/// what isn't the transcript's (the box's draft, the pick) stays.
#[test]
fn a_session_out_of_sight_lets_go_of_its_runs() {
    enum Where {
        Nowhere,
        Open,
        Beside,
        Tile,
    }
    use Where::*;
    // (case, where AWAY shows, how long since it last showed, a read
    // out, let go)
    let cases = [
        ("a minute away", Nowhere, MIN, false, false),
        ("just short", Nowhere, EVICT_AFTER - 1, false, false),
        ("EVICT_AFTER away", Nowhere, EVICT_AFTER, false, true),
        ("an hour away", Nowhere, 60 * MIN, false, true),
        ("opened", Open, 60 * MIN, false, false),
        ("beside the list", Beside, 60 * MIN, false, false),
        ("a pinned tile", Tile, 60 * MIN, false, false),
        ("a read out", Nowhere, 60 * MIN, true, false),
    ];
    for (case, at, since, reading, gone) in cases {
        let mut m = model(vec![agent(OPEN, ""), agent(AWAY, "")]);
        m.session = Some(view(OPEN));
        let held = holding(&mut m);
        let c = conn(&mut m, AWAY);
        c.shown_at = NOW;
        c.input.set("half a thought");
        c.runs.run_pick = 2;
        c.runs.reading = reading;
        match at {
            Nowhere => {}
            Open => m.session = Some(view(AWAY)),
            Beside => m.pane = Some(view(AWAY)),
            Tile => {
                m.tiles.insert(AWAY.into(), view(AWAY));
            }
        }
        m.now = NOW + since;
        evict::sweep(&mut m);
        for (w, what) in held.iter().zip(["hosted agent", "subagent"]) {
            assert_eq!(
                w.upgrade().is_none(),
                gone,
                "{case}: the {what}'s conversation"
            );
        }
        let c = conn(&mut m, AWAY);
        assert_eq!(c.runs.full.is_empty(), gone, "{case}");
        assert_eq!(
            c.runs.full_again, gone,
            "{case}: read whole for Changes again"
        );
        assert_eq!(
            c.input.string(),
            "half a thought",
            "{case}: the draft stays"
        );
        assert_eq!(c.runs.run_pick, 2, "{case}: the pick stays");
        assert!(
            core(&mut m).conns.contains_key(AWAY),
            "{case}: its side stays"
        );
    }
}

/// A session first met by the sweep (its side made by a card, never
/// opened) counts from then, not from the epoch; the one open is noted
/// as shown each sweep, so it's let go only EVICT_AFTER after it closed.
#[test]
fn time_away_counts_from_when_it_last_showed() {
    let mut m = model(vec![agent(OPEN, ""), agent(AWAY, "")]);
    let held = holding(&mut m);
    evict::sweep(&mut m);
    m.now = NOW + EVICT_AFTER - 1;
    evict::sweep(&mut m);
    assert!(held[0].upgrade().is_some(), "first seen at NOW");
    m.now = NOW + EVICT_AFTER;
    evict::sweep(&mut m);
    assert!(held[0].upgrade().is_none(), "EVICT_AFTER on");

    let mut m = model(vec![agent(OPEN, ""), agent(AWAY, "")]);
    let held = holding(&mut m);
    m.session = Some(view(AWAY));
    for t in [0, 10 * MIN, 20 * MIN] {
        m.now = NOW + t;
        evict::sweep(&mut m);
    }
    m.session = None;
    m.now = NOW + 20 * MIN + EVICT_AFTER - 1;
    evict::sweep(&mut m);
    assert!(held[0].upgrade().is_some(), "shown till 20 min in");
    m.now = NOW + 20 * MIN + EVICT_AFTER;
    evict::sweep(&mut m);
    assert!(held[0].upgrade().is_none());
}

fn write(p: &Path, lines: &[serde_json::Value]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let s: String = lines.iter().map(|l| l.to_string() + "\n").collect();
    std::fs::write(p, s).unwrap();
}

fn ts(s: u32) -> String {
    format!("2026-09-23T20:{:02}:{:02}Z", s / 60, s % 60)
}

/// A session whose second turn's shell call ran an agent rush hosted,
/// and which started a subagent; the agent's and the subagent's
/// transcripts beside it.
fn transcripts(d: &Path) -> (String, String) {
    let user = |t, c: serde_json::Value| serde_json::json!({"type":"user","timestamp":ts(t),"message":{"role":"user","content":c}});
    let said = |t, id: &str, c: serde_json::Value| {
        serde_json::json!({"type":"assistant","timestamp":ts(t),
            "message":{"id":id,"role":"assistant","model":"claude-opus-4-1","content":c}})
    };
    let parent = d.join("proj/s1.jsonl");
    write(
        &parent,
        &[
            user(0, "tidy the parser".into()),
            said(
                1,
                "m1",
                serde_json::json!([{"type":"text","text":"Tidied."}]),
            ),
            user(2, "have a worker port the tests".into()),
            said(
                3,
                "m2",
                serde_json::json!([{"type":"tool_use","id":"toolu_1","name":"Bash",
                    "input":{"command":"rush spawn --name porter 'port the tests'"}}]),
            ),
            user(
                4,
                serde_json::json!([{"type":"tool_result","tool_use_id":"toolu_1","content":"started"}]),
            ),
            said(
                5,
                "m3",
                serde_json::json!([{"type":"text","text":"The porter is on it."}]),
            ),
        ],
    );
    let kid = d.join("proj/kids/porter.jsonl");
    write(
        &kid,
        &[
            user(10, "port the tests".into()),
            said(
                11,
                "k1",
                serde_json::json!([{"type":"tool_use","id":"toolu_k1","name":"Edit",
                    "input":{"file_path":"/src/project/parser_test.go","old_string":"a","new_string":"b"}}]),
            ),
            user(
                12,
                serde_json::json!([{"type":"tool_result","tool_use_id":"toolu_k1","content":"ok"}]),
            ),
            said(
                13,
                "k2",
                serde_json::json!([{"type":"text","text":"Ported all nine tests."}]),
            ),
        ],
    );
    let sub = d.join("proj/s1/subagents");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("agent-a1.meta.json"),
        r#"{"agentType":"Explore","description":"find the parser's callers"}"#,
    )
    .unwrap();
    let side = |mut v: serde_json::Value| {
        v["isSidechain"] = true.into();
        v
    };
    write(
        &sub.join("agent-a1.jsonl"),
        &[
            side(user(20, "find the parser's callers".into())),
            side(said(
                21,
                "s1",
                serde_json::json!([{"type":"text","text":"Three callers."}]),
            )),
        ],
    );
    (
        parent.to_string_lossy().into_owned(),
        kid.to_string_lossy().into_owned(),
    )
}

/// Opens AWAY as the list does, its runs read as the dock reads them.
fn open(m: &mut Model, kid: &str) {
    let i = m.agents.iter().position(|a| a.key == AWAY).unwrap();
    m.open(i);
    m.full = true;
    subs::refresh(m, AWAY);
    conn(m, AWAY)
        .runs
        .spawns
        .entry("toolu_1".into())
        .or_insert_with(|| rush_ui_base::spawns::SpawnRun {
            step: "toolu_1".into(),
            kind: "claude".into(),
            name: "porter".into(),
            prompt: "port the tests".into(),
            path: kid.into(),
            born: NOW - 60 * MIN,
            ..Default::default()
        });
    for _ in 0..3 {
        subs::kick(m, AWAY);
        subs::refresh(m, AWAY);
    }
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

/// A session let go of is read back when opened again: its hosted
/// agent's conversation under the step that ran it, and the subagent
/// Changes had read whole, so the screen is the one it left, Changes'
/// count with it.
#[test]
fn opened_again_after_letting_go_it_looks_the_same() {
    let d = tempfile::tempdir().unwrap();
    let (parent, kid) = transcripts(d.path());
    let mut m = model(vec![agent(AWAY, &parent)]);
    m.sel = AWAY.into();
    // Read and drawn at one time, before and after: a turn its agent left
    // open is timed to when it's read.
    let later = NOW + EVICT_AFTER;
    m.now = later;
    m.clock = Some(later);
    open(&mut m, &kid);
    // Changes shown once: every run read whole for it.
    conn(&mut m, AWAY).runs.full_want = true;
    subs::kick(&mut m, AWAY);
    subs::refresh(&mut m, AWAY);
    conn(&mut m, AWAY).runs.full_want = false;
    let helpers = |m: &Model| m.session.as_ref().map_or(0, |v| v.sess.helpers.len());
    let kids = |m: &Model| {
        m.session.as_ref().map_or(0, |v| {
            v.o.kids
                .values()
                .flatten()
                .filter(|k| k.sess.as_ref().is_some_and(|s| !s.turns.is_empty()))
                .count()
        })
    };
    assert_eq!((kids(&m), helpers(&m)), (1, 2), "read whole before");
    let before = screen(&mut m);
    assert!(
        before.iter().any(|l| l.contains("porter")),
        "the agent's row:\n{}",
        before.join("\n")
    );

    // Back to the list, then out of sight long enough.
    m.session = None;
    conn(&mut m, AWAY).shown_at = NOW;
    assert_eq!(evict::sweep(&mut m), 1);
    assert!(conn(&mut m, AWAY).runs.full.is_empty(), "let go");

    open(&mut m, &kid);
    assert_eq!((kids(&m), helpers(&m)), (1, 2), "read whole again");
    assert_eq!(screen(&mut m), before);
}
