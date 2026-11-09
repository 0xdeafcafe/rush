//! Codex's rollouts read beside Claude Code's transcripts: one store,
//! two sources (Rust-only). Synthetic rollouts in a temp home.

use std::collections::HashMap;
use std::path::Path;

use rush_core::adapter::Profile;
use rush_state::Time;

use crate::aggregate::{Query, RANGES};
use crate::codex::Codex;
use crate::findings::findings;
use crate::scan::{File, scan};
use crate::store::Store;

const AT: &str = "2026-11-08T10:00:00Z";

fn usage(input: i64, cached: i64, out: i64, total: i64) -> String {
    format!(
        r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{out},"reasoning_output_tokens":100,"total_tokens":{total}}}"#
    )
}

fn token_count(input: i64, cached: i64, out: i64, total: i64, window: i64) -> String {
    let last = usage(input, cached, out, input + out);
    let all = usage(0, 0, 0, total);
    format!(
        r#"{{"timestamp":"{AT}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{all},"last_token_usage":{last},"model_context_window":{window}}},"rate_limits":{{"primary":{{"used_percent":12.0}}}}}}}}"#
    )
}

/// A rollout of n requests on model, each sending input tokens of a
/// window, cached all but 5,000; each count told twice, as Codex does
/// when its rate limits move, and one with no info.
fn rollout(model: &str, n: i64, input: i64, window: i64) -> Vec<String> {
    let mut out = vec![
        format!(
            r#"{{"timestamp":"{AT}","type":"session_meta","payload":{{"id":"c1","cwd":"/work/x","originator":"codex_cli_rs","cli_version":"0.200.0"}}}}"#
        ),
        format!(r#"{{"timestamp":"{AT}","type":"turn_context","payload":{{"model":"{model}"}}}}"#),
        format!(
            r#"{{"timestamp":"{AT}","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{{}}}}}}"#
        ),
        format!(
            r#"{{"timestamp":"{AT}","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"token_count"}}]}}}}"#
        ),
    ];
    let mut total = 0;
    for _ in 0..n {
        total += input + 500;
        let l = token_count(input, input - 5_000, 500, total, window);
        out.push(l.clone());
        out.push(l); // the same request told again
    }
    out
}

fn write(path: &Path, lines: &[String]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn scanned(lines: &[String]) -> File {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("rollout.jsonl");
    write(&p, lines);
    let mut f = File::of_agent("codex", "/codex", false);
    scan(&Codex, &p.to_string_lossy(), &mut f, Vec::new()).unwrap();
    f
}

#[test]
fn reads_a_rollout() {
    let f = scanned(&rollout("gpt-6.1-sol", 3, 20_000, 258_000));
    let t = f.totals();
    assert_eq!(t.req, 3, "a count told twice is one request");
    // OpenAI's input includes the cached part.
    assert_eq!(
        (t.input, t.cr, t.out, t.think),
        (15_000, 45_000, 1_500, 300)
    );
    assert_eq!(t.cost(), 0.0, "Codex isn't priced");
    assert_eq!((f.session.as_str(), f.project.as_str()), ("c1", "/work/x"));
    assert_eq!((f.model.as_str(), f.agent()), ("gpt-6.1-sol", "codex"));
    assert_eq!((f.peak(), f.window, t.near), (20_000, 258_000, 0));
}

// Counts near the window, by share of it.
#[test]
fn near_the_window() {
    for (input, window, near) in [
        (242_000, 258_000, 4),
        (232_300, 258_000, 4), // just past 90%
        (232_000, 258_000, 0),
        (242_000, 0, 0), // no window said
    ] {
        let f = scanned(&rollout("gpt-6-astra", 4, input, window));
        assert_eq!(f.totals().near, near, "{input} of {window}");
    }
}

// A subagent's rollout says so in its session_meta's source.
#[test]
fn subagents() {
    let mut lines = rollout("gpt-6.1-sol", 1, 1_000, 258_000);
    lines[0] = format!(
        r#"{{"timestamp":"{AT}","type":"session_meta","payload":{{"id":"c2","cwd":"/w","source":{{"subagent":{{"thread_spawn":{{}}}}}}}}}}"#
    );
    assert!(scanned(&lines).sub);
}

#[test]
fn findings_for_codex() {
    let p = "/codex/sessions/2026/11/08/rollout-a.jsonl".to_string();
    let f = scanned(&rollout("gpt-6-astra", 250, 242_000, 258_000));
    let s = Store::of(HashMap::from([(p, f)]));
    let v = s.view(&Query::new(
        RANGES[0],
        Time::parse("2026-11-08T12:00:00Z").unwrap(),
    ));
    assert_eq!((v.near, v.nearest.near, v.nearest.input), (1, 250, 242_000));
    assert_eq!((v.frontier, v.longest.req), (1, 250));
    assert_eq!(v.agents["codex"].sessions, 1);
    let fs = findings(&v, &HashMap::new());
    let near = fs
        .iter()
        .find(|f| f.title == "1 Codex sessions ran near their context window")
        .expect("near the window");
    assert!(
        near.detail.starts_with(
            "the closest sent 242k of 258k tokens, 250 requests running (gpt-6-astra)"
        ),
        "{}",
        near.detail
    );
    let long = fs
        .iter()
        .find(|f| f.title == "1 long Codex sessions ran on gpt-6-astra")
        .expect("long on astra");
    assert!(
        long.detail.contains("does as well on gpt-6.1-sol"),
        "{}",
        long.detail
    );
    assert!(
        fs.iter().all(|f| f.cost == 0.0),
        "Codex has no prices: {fs:?}"
    );
}

// One store reads both agents' profiles, each through its own source, and
// keeps Codex's apart from the cache Go's build reads.
#[test]
fn one_store_two_sources() {
    crate::testutil::claude();
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().join("rush")));
    rush_state::set_thread_cache_dir(Some(home.path().join("cache")));
    let claude = home.path().join("claude");
    write(
        &claude.join("projects").join("-work-repo").join("s1.jsonl"),
        &[r#"{"type":"assistant","timestamp":"2026-11-08T10:00:00Z","sessionId":"s1","cwd":"/work/repo","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":500,"output_tokens":5}}}"#.to_string()],
    );
    let codex = home.path().join("codex");
    let day = codex.join("sessions").join("2026").join("11").join("08");
    write(
        &day.join("rollout-2026-11-08T10-00-00-c1.jsonl"),
        &rollout("gpt-6.1-sol", 2, 1_000, 258_000),
    );
    write(&day.join("notes.txt"), &["not a rollout".to_string()]);
    let profile = |kind: &str, dir: &Path| Profile {
        kind: kind.into(),
        name: kind.into(),
        dir: dir.to_string_lossy().into_owned(),
    };
    let profiles = [
        profile("claude", &claude),
        profile("codex", &codex),
        profile("kimi", home.path()), // read by no source
    ];
    assert_eq!(crate::source::transcripts(&profiles[1]).len(), 1);
    let s = Store::open();
    assert!(s.refresh(&profiles));
    assert!(!s.refresh(&profiles), "nothing new the second time");
    let v = s.view(&Query::new(
        RANGES[0],
        Time::parse("2026-11-08T12:00:00Z").unwrap(),
    ));
    assert_eq!(v.sessions, 2);
    assert_eq!(
        (v.agents["claude"].sessions, v.agents["claude"].b.req),
        (1, 1)
    );
    assert_eq!(
        (v.agents["codex"].sessions, v.agents["codex"].b.req),
        (1, 2)
    );
    assert!(v.agents["claude"].b.cost() > 0.0);
    // A view of one agent leaves the other out.
    let mut q = Query::new(RANGES[0], Time::parse("2026-11-08T12:00:00Z").unwrap());
    q.agents = vec!["claude".into()];
    let only = s.view(&q);
    assert_eq!((only.sessions, only.total.req), (1, 1));

    s.save();
    let go = std::fs::read_to_string(rush_state::cache_path("efficiency.json")).unwrap();
    assert!(go.contains("s1.jsonl") && !go.contains("rollout-"), "{go}");
    let others = std::fs::read_to_string(rush_state::cache_path("efficiency-agents.json")).unwrap();
    assert!(
        others.contains("rollout-") && others.contains(r#""ag":"codex""#),
        "{others}"
    );
    assert_eq!(Store::open().len(), 2);
    rush_state::set_thread_dir(None);
    rush_state::set_thread_cache_dir(None);
}
