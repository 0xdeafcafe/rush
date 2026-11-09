//! Claude Code's own transcripts read through its adapter: efficiency's
//! figures as they come from a real agent's files (Go's
//! claude_scan_test.go).

use std::collections::HashMap;

use rush_core::agent::TokenUsage;
use rush_state::Time;

use crate::aggregate::{Metric, Query, RANGES};
use crate::claude::Claude;
use crate::detect::{Found, Status};
use crate::findings::findings;
use crate::scan::{File, TOOL_BASH, TOOL_READ, scan};
use crate::store::Store;

// Lines as Claude Code writes them, cut to what the scan reads.
fn transcript() -> Vec<String> {
    let x = "x".repeat(1000);
    let y = "y".repeat(500);
    vec![
        r#"{"type":"attachment","timestamp":"2026-09-20T10:00:00Z","sessionId":"s1","cwd":"/work/repo","attachment":{"type":"hook_success","hookEvent":"SessionStart","command":"node \"${CLAUDE_PLUGIN_ROOT}/src/hooks/caveman-activate.js\""}}"#.into(),
        r#"{"type":"user","timestamp":"2026-09-20T10:00:01Z","message":{"role":"user","content":"<command-name>/caveman</command-name>\n<command-args>ultra</command-args>"}}"#.into(),
        // One message written as two lines: its usage is counted once, the last.
        r#"{"type":"assistant","timestamp":"2026-09-20T10:00:02Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":50000,"output_tokens":5}}}"#.into(),
        r#"{"type":"assistant","timestamp":"2026-09-20T10:00:03Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"FOO=1 rtk git status"}},{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/work/repo/a.go"}}],"usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":50000,"output_tokens":40,"output_tokens_details":{"thinking_tokens":30}}}}"#.into(),
        format!(r#"{{"type":"user","timestamp":"2026-09-20T10:00:04Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"{x}"}},{{"type":"tool_result","tool_use_id":"t2","content":[{{"type":"text","text":"{y}"}}]}}]}}}}"#),
        r#"{"type":"attachment","timestamp":"2026-09-20T10:00:05Z","attachment":{"type":"hook_success","hookEvent":"PreToolUse","command":"rtk hook claude"}}"#.into(),
        r#"{"type":"assistant","timestamp":"2026-09-20T11:30:00Z","message":{"id":"m2","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"rtk gain -d"}},{"type":"tool_use","id":"t4","name":"Skill","input":{"skill":"caveman:caveman"}},{"type":"tool_use","id":"t5","name":"mcp__serena__find_symbol","input":{}}],"usage":{"input_tokens":1,"cache_read_input_tokens":50000,"cache_creation_input_tokens":100,"output_tokens":20}}}"#.into(),
        // A tool result mentioning compact_boundary is still a tool result.
        r#"{"type":"user","timestamp":"2026-09-20T11:30:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t3","content":"\"compact_boundary\""}]}}"#.into(),
        r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-09-20T11:31:00Z","compactMetadata":{"trigger":"auto","preTokens":500000,"postTokens":20000}}"#.into(),
        r#"{"type":"attachment","timestamp":"2026-09-20T11:32:00Z","attachment":{"type":"invoked_skills","skills":[{"name":"graphify","path":"userSettings:graphify"}]}}"#.into(),
    ]
}

fn write(dir: &std::path::Path, lines: &[String]) -> String {
    let p = dir.join("s1.jsonl");
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p.to_string_lossy().into_owned()
}

fn scanned(acct: &str) -> (tempfile::TempDir, String, File) {
    crate::testutil::claude();
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), &transcript());
    let mut f = File::new(acct, false);
    scan(&Claude, &p, &mut f, Vec::new()).unwrap();
    (dir, p, f)
}

#[test]
fn scans() {
    let (_d, _p, f) = scanned("");
    assert_eq!(
        (f.session.as_str(), f.project.as_str()),
        ("s1", "/work/repo")
    );
    let tot = f.totals();
    assert_eq!(tot.req, 2, "two messages are two requests");
    assert!(
        tot.out == 60 && tot.think == 30 && tot.cw == 50100 && tot.cr == 50000,
        "{tot:?}"
    );
    assert_eq!((f.start(), f.peak()), (50010, 50101));
    let quoted = r#"\"compact_boundary\""#.len() as i64;
    assert_eq!(tot.calls[TOOL_BASH], 2);
    assert_eq!(tot.bytes[TOOL_BASH], 1000 + quoted);
    assert_eq!((tot.calls[TOOL_READ], tot.bytes[TOOL_READ]), (1, 500));
    assert!(f.compacts.len() == 1 && f.compacts[0].auto && f.compacts[0].pre == 500000);
    assert_eq!(f.reads.get("/work/repo/a.go"), Some(&1));
    for k in [
        "bash:rtk",
        "bash:rtk/admin",
        "hook:rtk hook claude",
        "skill:caveman:caveman",
        "cmd:/caveman",
        "mcp:serena",
        "skill:graphify",
    ] {
        assert!(
            f.uses.contains_key(k),
            "didn't see {k:?}; saw {:?}",
            f.uses.keys()
        );
    }
    assert_eq!(f.hours.len(), 2, "two hours of use");
    let want = crate::price::cost(
        "claude-opus-5-5",
        &TokenUsage {
            input: 11,
            output: 60,
            cache_read: 50000,
            cache_write_5m: 50100,
            ..TokenUsage::default()
        },
        false,
    );
    assert!(want > 0.0);
    assert!(
        (tot.cost() - want).abs() < 1e-9,
        "cost {} want {want}",
        tot.cost()
    );
}

// Reading a transcript in pieces, as it's written, comes to the same as
// reading it whole; a half-written line waits.
#[test]
fn scans_in_pieces() {
    let (_d, _p, whole) = scanned("");
    let dir = tempfile::tempdir().unwrap();
    let q = dir.path().join("s1.jsonl");
    let qs = q.to_string_lossy().into_owned();
    let mut f = File::default();
    let mut written = String::new();
    for l in transcript() {
        let mut half = l.len() / 2;
        while !l.is_char_boundary(half) {
            half -= 1;
        }
        written += &l[..half];
        std::fs::write(&q, &written).unwrap();
        scan(&Claude, &qs, &mut f, Vec::new()).unwrap();
        written += &l[half..];
        written.push('\n');
        std::fs::write(&q, &written).unwrap();
        scan(&Claude, &qs, &mut f, Vec::new()).unwrap();
        f = f.clone(); // as the store does between runs
    }
    assert_eq!(whole.totals(), f.totals());
}

#[test]
fn views_and_findings() {
    let (_d, p, f) = scanned("/acct");
    let s = Store::of(HashMap::from([(p, f)]));
    let at = Time::parse("2026-09-20T23:00:00Z").unwrap();
    let v = s.view(&Query::new(RANGES[0], at));
    assert!(v.total.req == 2 && v.sessions == 1 && v.compacts == 1 && v.autos == 1);
    for id in ["rtk", "caveman", "serena", "graphify"] {
        assert!(v.uses.contains_key(id), "saver uses: {:?}", v.uses.keys());
    }
    assert!(!v.first_use["caveman"].is_zero());
    assert_eq!(v.series(Metric::Ctx).values.len(), v.points.len());
    let mut q = Query::new(RANGES[0], at);
    q.accounts = vec!["/other".into()];
    assert_eq!(
        s.view(&q).total.req,
        0,
        "another account's view counted this one"
    );
    let mut q = Query::new(RANGES[0], at);
    q.project = "/work/rep".into();
    assert_eq!(
        s.view(&q).total.req,
        0,
        "a folder's prefix isn't the folder"
    );
    q.project = "/work".into();
    assert_eq!(
        s.view(&q).total.req,
        2,
        "a parent folder includes its sessions"
    );
    let found = HashMap::from([(
        "rtk".to_string(),
        Found {
            status: Status::Partial,
            wants: "its hook isn't in settings.json".into(),
            ..Found::default()
        },
    )]);
    let fs = findings(&v, &found);
    assert_eq!(fs[0].fix, "rtk", "a half set up saver comes first: {fs:?}");
    let top = s.top_sessions(&Query::new(RANGES[0], at), 5);
    assert!(top.len() == 1 && top[0].b.req == 2);
}

#[test]
fn retires() {
    let (_d, _p, f) = scanned("/acct");
    let s = Store::of(HashMap::new());
    s.retire(&f);
    let at = Time::parse("2026-09-20T23:00:00Z").unwrap();
    let v = s.view(&Query::new(RANGES[0], at));
    assert!(v.total.req == 2 && !v.first_use["rtk"].is_zero());
}

#[test]
fn lists_and_refreshes() {
    crate::testutil::claude();
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().join("rush")));
    rush_state::set_thread_cache_dir(Some(home.path().join("cache")));
    let acct = home.path().join("claude");
    let proj = acct.join("projects").join("-work-repo");
    std::fs::create_dir_all(proj.join("s1").join("subagents")).unwrap();
    write(&proj, &transcript());
    std::fs::write(
        proj.join("s1").join("subagents").join("a.jsonl"),
        transcript().join("\n") + "\n",
    )
    .unwrap();
    let p = rush_core::adapter::Profile {
        kind: crate::AGENT.into(),
        name: "x".into(),
        dir: acct.to_string_lossy().into_owned(),
    };
    let paths = crate::source::transcripts(&p);
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(paths[1].contains("/subagents/"));
    let s = Store::open();
    assert!(s.refresh(std::slice::from_ref(&p)));
    assert!(
        !s.refresh(std::slice::from_ref(&p)),
        "nothing new the second time"
    );
    let at = Time::parse("2026-09-20T23:00:00Z").unwrap();
    let v = s.view(&Query::new(RANGES[0], at));
    assert_eq!((v.sessions, v.subagents), (1, 1));
    // The cache carries it into the next run.
    s.save();
    assert_eq!(Store::open().len(), 2);
    // A deleted transcript is retired, its totals kept.
    std::fs::remove_file(proj.join("s1.jsonl")).unwrap();
    assert!(s.refresh(std::slice::from_ref(&p)));
    assert_eq!(s.view(&Query::new(RANGES[0], at)).total.req, 4);
    rush_state::set_thread_dir(None);
    rush_state::set_thread_cache_dir(None);
}

// A session that reads one screenshot again and again: each read is an
// image counted at the model's tier, every read after the first is a
// repeat, and every request re-reads the images sent before it until a
// compaction drops them (Rust-only).
fn image_transcript(reads: usize) -> Vec<String> {
    let png = crate::image::tests::png(1920, 1080);
    let mut out = Vec::new();
    let usage = r#""usage":{"input_tokens":10,"cache_read_input_tokens":100000,"cache_creation_input_tokens":0,"output_tokens":5}"#;
    for i in 0..reads {
        out.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-09-20T10:00:{i:02}Z","sessionId":"s1","cwd":"/work/repo","message":{{"id":"m{i}","model":"claude-opus-5-5","content":[{{"type":"tool_use","id":"t{i}","name":"Read","input":{{"file_path":"/work/repo/shot.png"}}}}],{usage}}}}}"#
        ));
        out.push(format!(
            r#"{{"type":"user","timestamp":"2026-09-20T10:00:{i:02}Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{i}","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{png}"}}}}]}}]}}}}"#
        ));
    }
    out.push(format!(
        r#"{{"type":"assistant","timestamp":"2026-09-20T10:01:00Z","message":{{"id":"last","model":"claude-opus-5-5","content":[{{"type":"text","text":"done"}}],{usage}}}}}"#
    ));
    out
}

#[test]
fn counts_images_and_their_rereads() {
    crate::testutil::claude();
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), &image_transcript(12));
    let mut f = File::new("/acct", false);
    scan(&Claude, &p, &mut f, Vec::new()).unwrap();
    let t = f.totals();
    // 1920×1080 is 2,691 tokens on Opus 5.5's high-resolution tier.
    let one = 2691;
    assert_eq!((t.img, t.img_tok, t.img_rep), (12, 12 * one, 11), "{t:?}");
    // Image i is first sent by request i+1 and re-read by each after it,
    // to the 13th: 11 + 10 + … + 1 reads, the repeats' 10 + … + 1.
    assert_eq!(t.img_carry, 66 * one);
    assert_eq!(t.img_rep_carry, 55 * one);
    assert_eq!(f.img_reads.get("/work/repo/shot.png"), Some(&12));
    assert_eq!(f.img_max, 12 * one);
    assert_eq!(t.bytes[TOOL_READ], 0, "an image isn't text");

    let s = Store::of(HashMap::from([(p, f)]));
    let at = Time::parse("2026-09-20T23:00:00Z").unwrap();
    let v = s.view(&Query::new(RANGES[0], at));
    assert_eq!((v.img_rep_sessions, v.worst_image_reads), (1, 12));
    assert_eq!(v.img_heavy, 1, "a third of its peak context was images");
    let read = crate::price::cache_read_price("claude-opus-5-5");
    assert!((v.img_carry - (66 * one) as f64 * read / 1e6).abs() < 1e-9);
    let fs = findings(&v, &HashMap::new());
    let titles: Vec<&str> = fs.iter().map(|f| f.title.as_str()).collect();
    for want in [
        "11 image reads were of an image the same session had read already",
        "1 sessions were heavy on images",
    ] {
        let Some(f) = fs.iter().find(|f| f.title == want) else {
            panic!("no {want:?} in {titles:?}");
        };
        assert!(f.cost > 0.0 && f.detail.contains("an estimate"), "{f:?}");
    }
    assert!(
        fs.iter()
            .any(|f| f.detail.contains("shot.png, read 12 times"))
    );
}

// A compaction drops the images: the requests after it re-read none.
#[test]
fn compaction_drops_images() {
    crate::testutil::claude();
    let dir = tempfile::tempdir().unwrap();
    let mut lines = image_transcript(3);
    let last = lines.pop().unwrap();
    lines.push(r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-09-20T10:00:30Z","compactMetadata":{"trigger":"auto","preTokens":500000,"postTokens":20000}}"#.into());
    lines.push(last);
    let p = write(dir.path(), &lines);
    let mut f = File::new("", false);
    scan(&Claude, &p, &mut f, Vec::new()).unwrap();
    // Only the third request re-read an image (the first); the one after
    // the compaction, none.
    assert_eq!(f.totals().img_carry, 2691);
}
