//! The report `rush efficiency` prints and Settings › Usage shows: built
//! from a store of synthetic transcripts and an event log in a temp home.

use std::collections::HashMap;
use std::path::Path;

use rush_core::adapter::Profile;
use rush_state::Time;

use super::*;
use crate::aggregate::AgentPart;
use crate::events::add_event;
use crate::scan::{Bucket, File};
use crate::when::{HOUR, hour_of};

fn now() -> Time {
    Time::parse("2026-11-09T12:00:00Z").unwrap()
}

fn ago(h: i64) -> Time {
    add_secs(now(), -h * HOUR)
}

/// A transcript of one hour's use.
fn file(agent: &str, session: &str, project: &str, first: Time, last: Time, b: Bucket) -> File {
    let mut f = File::of_agent(agent, "/acct", false);
    f.session = session.into();
    f.project = project.into();
    f.model = if agent == "codex" {
        "gpt-6.1-sol".into()
    } else {
        "claude-opus-5-5".into()
    };
    f.first = first;
    f.last = last;
    f.start_ctx = 22_000;
    f.peak_ctx = 300_000;
    f.hours = HashMap::from([(hour_of(first), b)]);
    f
}

fn handover(at: Time, account: &str, from_session: &str) -> Event {
    Event {
        at,
        kind: HANDOVER.into(),
        account: account.into(),
        source: "rush".into(),
        handover: Some(Handover {
            from: "a1".into(),
            from_session: from_session.into(),
            to: "b1".into(),
            agent: "claude".into(),
            model: "opus".into(),
            from_context: 307_000,
            from_cost_usd: 15.5,
            to_context: 22_000,
            threshold: 300_000,
            lineage: 1,
            subagent: false,
        }),
        ..Event::default()
    }
}

// The sessions are ordered by tokens processed, not dollars; a handover
// finds its successor's transcript and counts what its requests didn't
// re-read; one whose successor isn't read yet says so; one outside the
// time is left out.
#[test]
fn builds_from_the_store_and_the_log() {
    crate::testutil::claude();
    let a = file(
        "claude",
        "s-a",
        "/src/rush",
        ago(20),
        ago(10),
        Bucket {
            req: 300,
            input: 30_000,
            cw: 400_000,
            cr: 60_000_000,
            out: 200_000,
            c_cr: 1.0,
            ..Bucket::default()
        },
    );
    let mut b = file(
        "claude",
        "s-b",
        "/src/rush",
        add_secs(ago(10), -60),
        ago(8),
        Bucket {
            req: 140,
            input: 10_000,
            cw: 100_000,
            cr: 20_000_000,
            out: 100_000,
            c_cr: 50.0, // costlier, yet it processed less
            ..Bucket::default()
        },
    );
    b.start_ctx = 21_000;
    let c = file(
        "codex",
        "c1",
        "/src/other",
        ago(5),
        ago(4),
        Bucket {
            req: 50,
            input: 1_000_000,
            cr: 9_000_000,
            out: 50_000,
            ..Bucket::default()
        },
    );
    let store = Store::of(HashMap::from([
        ("/t/a.jsonl".to_string(), a),
        ("/t/b.jsonl".to_string(), b),
        ("/t/c.jsonl".to_string(), c),
    ]));
    let events = vec![
        handover(ago(100), "/acct", "s-a"),
        handover(ago(10), "/acct", "s-a"),
        handover(ago(1), "/elsewhere", "s-x"),
    ];
    let q = query(now(), 2 * DAY, "");
    let r = Report::build(&store, &q, &HashMap::new(), &events);
    let paths: Vec<&str> = r.top.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["/t/a.jsonl", "/t/b.jsonl", "/t/c.jsonl"]);
    let kinds: Vec<&str> = r.agents.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["claude", "codex"]);
    assert_eq!(r.agents[1].1.b.req, 50);
    assert_eq!(r.handovers.len(), 2, "{:?}", r.handovers);
    let s = &r.handovers[0];
    assert_eq!(
        (s.transcript.as_str(), s.requests, s.per_request, s.tokens),
        ("/t/b.jsonl", 140, 285_000, 39_900_000)
    );
    let want = 39_900_000.0 * cache_read_price_of("claude", "claude-opus-5-5") / 1e6;
    assert!(s.usd > 0.0 && (s.usd - want).abs() < 1e-9, "{}", s.usd);
    let lost = &r.handovers[1];
    assert!(lost.transcript.is_empty() && lost.tokens == 0, "{lost:?}");
    assert!(handover_line(lost).contains("isn't read yet"));
    assert_eq!(r.saved().0, 39_900_000);

    // One agent's report leaves the other's out, and its handovers.
    let only = Report::build(
        &store,
        &query(now(), 2 * DAY, "codex"),
        &HashMap::new(),
        &events,
    );
    assert_eq!(only.agents.len(), 1);
    assert!(only.handovers.is_empty());
    assert_eq!(only.top.len(), 1);

    let j = to_json(&r);
    assert_eq!(j["handovers"][0]["savedTokens"], 39_900_000);
    assert_eq!(j["saved"]["estimate"], true);
    assert_eq!(j["top"][0]["processed"], 60_630_000);
    assert_eq!(j["agents"][1]["agent"], "codex");
}

/// The fixture the sample in the report is printed from.
fn sample() -> Report {
    let part = |b: Bucket| AgentPart {
        sessions: 2,
        subagents: 0,
        b,
    };
    Report {
        from: ago(48),
        to: now(),
        agents: vec![
            (
                "claude".into(),
                part(Bucket {
                    req: 1_204,
                    input: 1_234_000,
                    cw: 3_400_000,
                    cr: 120_000_000,
                    out: 900_000,
                    c_in: 6.17,
                    c_out: 22.5,
                    c_cr: 60.0,
                    c_cw: 21.25,
                    ..Bucket::default()
                }),
            ),
            (
                "codex".into(),
                part(Bucket {
                    req: 310,
                    input: 2_000_000,
                    cr: 40_000_000,
                    out: 300_000,
                    ..Bucket::default()
                }),
            ),
        ],
        top: vec![
            SessionCost {
                path: "/t/a.jsonl".into(),
                project: "/src/rush".into(),
                first: ago(20),
                last: ago(10),
                peak_ctx: 412_000,
                b: Bucket {
                    req: 830,
                    input: 50_000,
                    cw: 1_000_000,
                    cr: 44_050_000,
                    out: 200_000,
                    ..Bucket::default()
                },
                ..SessionCost::default()
            },
            SessionCost {
                path: "/t/s.jsonl".into(),
                project: "/src/rush/".into(),
                sub: true,
                first: ago(3),
                last: ago(2),
                peak_ctx: 96_000,
                b: Bucket {
                    req: 120,
                    input: 20_000,
                    cw: 80_000,
                    cr: 9_000_000,
                    out: 100_000,
                    ..Bucket::default()
                },
                ..SessionCost::default()
            },
        ],
        findings: vec![
            Finding {
                title: "1,680 image reads were of an image the same session had read already".into(),
                detail: "the requests after them re-read about 41M tokens of them, about $20.50 (an estimate). Note what an image showed rather than reading it again, or put the ones to compare on one contact sheet".into(),
                cost: 20.5,
                ..Finding::default()
            },
            Finding {
                title: "Only 84% of input came from the cache".into(),
                detail: "sessions left idle past the cache's life, or switching model, write the whole context again".into(),
                ..Finding::default()
            },
        ],
        handovers: vec![Saving {
            at: ago(10),
            h: Handover {
                from: "a1".into(),
                to: "b1".into(),
                from_context: 307_000,
                to_context: 22_000,
                ..Handover::default()
            },
            transcript: "/t/b.jsonl".into(),
            requests: 140,
            per_request: 285_000,
            tokens: 39_900_000,
            usd: 19.95,
        }],
    }
}

// The text is short and lined up: totals by agent, the sessions, the
// findings with their details under them, the handovers and how their
// saving is worked out.
#[test]
fn prints_the_sample() {
    let r = sample();
    let want = [
        format!("Efficiency, {} to {}", at(ago(48)), at(now())),
        String::new(),
        "            requests  uncached  cache write  cache read   output        cost".into(),
        "  claude       1,204      1.2M         3.4M      120.0M     900k        $110".into(),
        "  codex          310      2.0M            0       40.0M     300k  not priced".into(),
        String::new(),
        "Most tokens processed".into(),
        format!("   45.3M  rush  {}  peak 412k  830 req", span(ago(20), ago(10))),
        format!("    9.2M  rush  {}  peak 96k  120 req  subagent", span(ago(3), ago(2))),
        String::new(),
        "Findings, biggest cost first".into(),
        "   $20.50  1,680 image reads were of an image the same session had read already".into(),
        "           the requests after them re-read about 41M tokens of them, about $20.50 (an estimate).".into(),
        "           Note what an image showed rather than reading it again, or put the ones to compare on".into(),
        "           one contact sheet".into(),
        String::new(),
        "        –  Only 84% of input came from the cache".into(),
        "           sessions left idle past the cache's life, or switching model, write the whole context".into(),
        "           again".into(),
        String::new(),
        "Handovers, saving ≈39.9M tokens by estimate (≈$19.95)".into(),
        format!(
            "  a1 → b1, {}: 307k → 22k, ≈285k a request less × 140 requests ≈ 39.9M tokens, ≈$19.95",
            at(ago(10))
        ),
        "  An estimate: the context the retiring session had at the handover, less what its successor".into(),
        "  started with, times the successor's requests. That is what each request would have re-read".into(),
        "  had the first carried on with the same work; it leaves out a compaction the first might have".into(),
        "  made, and what writing the baton cost. Priced at the cache-read rate.".into(),
    ]
    .map(|l| l + "\n")
    .concat();
    assert_eq!(text(&r), want, "drawn now:\n{}", text(&r));
    assert!(text(&Report::default()).contains("No requests in that time."));
    for (n, want) in [
        (0, "0"),
        (999, "999"),
        (1_680, "1,680"),
        (1_234_567, "1,234,567"),
    ] {
        assert_eq!(count(n), want);
    }
}

#[test]
fn flags() {
    let args = |s: &[&str]| s.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    for (since, want) in [
        ("90m", Some(90 * 60)),
        ("24h", Some(DAY)),
        ("2d", Some(2 * DAY)),
        ("1w", Some(7 * DAY)),
        ("2", None),
        ("d", None),
        ("0d", None),
        ("2y", None),
    ] {
        assert_eq!(parse_since(since), want, "{since}");
    }
    let f = parse_flags(&args(&["--since=2d", "-agent", "codex", "--json"])).unwrap();
    assert_eq!(
        (f.since, f.agent.as_str(), f.json),
        (2 * DAY, "codex", true)
    );
    assert_eq!(parse_flags(&args(&[])).unwrap().since, 7 * DAY);
    assert!(parse_flags(&args(&["-h"])).unwrap().help);
    for bad in [
        &["--agent", "kimi"][..],
        &["--since", "soon"][..],
        &["--since"][..],
        &["--what"][..],
        &["stray"][..],
    ] {
        assert!(parse_flags(&args(bad)).is_err(), "{bad:?}");
    }
}

fn write(path: &Path, lines: &[&str]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn assistant(at: &str, session: &str, id: &str, cr: i64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{at}","sessionId":"{session}","cwd":"/work/repo","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"hi"}}],"usage":{{"input_tokens":10,"cache_read_input_tokens":{cr},"cache_creation_input_tokens":500,"output_tokens":5}}}}}}"#
    )
}

// rush efficiency end to end, in a temp home: it reads the profiles'
// transcripts and the event log, and prints the report or its JSON; a
// bad flag is exit 2 with the usage.
#[test]
fn the_command() {
    crate::testutil::claude();
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().join("rush")));
    rush_state::set_thread_cache_dir(Some(home.path().join("cache")));
    crate::detect::set_thread_search_path(Some(Vec::new()));
    let claude = home.path().join("claude");
    let dir = claude.join("projects").join("-work-repo");
    write(
        &dir.join("s1.jsonl"),
        &[assistant("2026-11-08T10:00:00Z", "s1", "m1", 299_000).as_str()],
    );
    write(
        &dir.join("s2.jsonl"),
        &[
            assistant("2026-11-08T10:30:00Z", "s2", "m2", 0).as_str(),
            assistant("2026-11-08T10:31:00Z", "s2", "m3", 1_000).as_str(),
        ],
    );
    let acct = claude.to_string_lossy().into_owned();
    let mut ev = handover(Time::parse("2026-11-08T10:31:30Z").unwrap(), &acct, "s1");
    if let Some(h) = ev.handover.as_mut() {
        h.from_context = 300_000;
        h.to_context = 510;
    }
    add_event(ev).unwrap();
    let profiles = [Profile {
        kind: "claude".into(),
        name: "claude".into(),
        dir: acct,
    }];
    let run = |args: &[&str]| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let code = efficiency_cmd(
            &profiles,
            &args,
            Time::parse("2026-11-08T12:00:00Z").unwrap(),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    };
    let (code, out, _) = run(&["--since", "1d"]);
    assert_eq!(code, 0);
    for want in [
        "  claude           3",
        "Most tokens processed",
        "a1 → b1",
        "≈299k a request less × 2 requests ≈ 599k tokens",
        "An estimate:",
    ] {
        assert!(out.contains(want), "{want:?} in:\n{out}");
    }
    let (code, out, _) = run(&["--json", "--agent", "claude"]);
    assert_eq!(code, 0);
    let j: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(j["handovers"][0]["requests"], 2);
    assert_eq!(j["agents"][0]["requests"], 3);
    let (code, out, err) = run(&["--since", "soon"]);
    assert_eq!(code, 2);
    assert!(out.is_empty() && err.contains("--since soon") && err.contains("rush efficiency ["));
    let (code, out, _) = run(&["--since", "1h"]);
    assert_eq!((code, out.contains("No requests in that time.")), (0, true));
    crate::detect::set_thread_search_path(None);
    rush_state::set_thread_dir(None);
    rush_state::set_thread_cache_dir(None);
}
