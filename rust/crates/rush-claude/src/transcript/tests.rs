//! Port of internal/adapters/claude/claude/transcript_test.go.

use super::*;
use std::io::Write;

const BLOCK1: &str = r#"{"type":"assistant","timestamp":"2026-09-23T10:00:00Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"thinking"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":1000,"cache_creation_input_tokens":200,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":200}}}}"#;
const BLOCK2: &str = r#"{"type":"assistant","timestamp":"2026-09-23T10:00:01Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"see https://github.com/a/b/pull/42"}],"usage":{"input_tokens":10,"output_tokens":50,"cache_read_input_tokens":1000,"cache_creation_input_tokens":200,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":200}}}}"#;
const USER_LINE: &str =
    r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}"#;
const NEXT: &str = r#"{"type":"assistant","timestamp":"2026-09-23T10:01:00Z","message":{"id":"m2","model":"claude-opus-5-5","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;

pub(crate) struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "rush-claude-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn append(p: &Path, s: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
    f.write_all(s.as_bytes()).unwrap();
}

#[test]
fn scan_counts_each_message_once_and_resumes() {
    let d = TempDir::new("scan");
    let p = d.0.join("s.jsonl");
    std::fs::write(
        &p,
        format!("{BLOCK1}\n{BLOCK2}\n{USER_LINE}\n{}", &NEXT[..40]),
    )
    .unwrap();
    let mut tot = Totals::default();
    let mut buf = Vec::new();
    scan(&p, &mut tot, &mut buf).unwrap();
    let (c, u) = tot.spend();
    // m1 counted once with its final usage: 10 in, 50 out, 1000 read, 200 1h write.
    let want = (10.0 * 4.0 + 50.0 * 20.0 + 1000.0 * 0.20 + 200.0 * 4.0 * 2.0) / 1e6;
    assert!(
        (c - want).abs() < 1e-12 && u.output == 50,
        "cost {c} want {want}, usage {u:?}"
    );
    assert_eq!(tot.prs, vec!["https://github.com/a/b/pull/42".to_string()]);
    // The half-written line is not consumed until it completes.
    append(&p, &format!("{}\n", &NEXT[40..]));
    scan(&p, &mut tot, &mut buf).unwrap();
    let (c2, u2) = tot.spend();
    assert!(u2.output == 51 && c2 > c, "after resume: {c2} {u2:?}");
    let ds = tot.day_spend(day(tot.last));
    assert!((ds - c2).abs() < 1e-12, "day spend {ds} want {c2}");
}

#[test]
fn scan_resumes_identically_to_one_read() {
    // Split anywhere, the totals come out the same as one read.
    let all = format!("{BLOCK1}\n{BLOCK2}\n{USER_LINE}\n{NEXT}\n");
    let d = TempDir::new("split");
    let one = d.0.join("one.jsonl");
    std::fs::write(&one, &all).unwrap();
    let mut whole = Totals::default();
    scan(&one, &mut whole, &mut Vec::new()).unwrap();
    for cut in [1, 100, all.len() / 2, all.len() - 1] {
        let p = d.0.join(format!("cut{cut}.jsonl"));
        std::fs::write(&p, &all[..cut]).unwrap();
        let mut t = Totals::default();
        let mut buf = Vec::new();
        scan(&p, &mut t, &mut buf).unwrap();
        append(&p, &all[cut..]);
        scan(&p, &mut t, &mut buf).unwrap();
        assert_eq!(t, whole, "cut at {cut}");
    }
}

#[test]
fn scan_starts_over_when_the_file_shrinks() {
    let d = TempDir::new("shrink");
    let p = d.0.join("s.jsonl");
    std::fs::write(&p, format!("{BLOCK1}\n{NEXT}\n")).unwrap();
    let mut t = Totals::default();
    scan(&p, &mut t, &mut Vec::new()).unwrap();
    std::fs::write(&p, format!("{NEXT}\n")).unwrap();
    scan(&p, &mut t, &mut Vec::new()).unwrap();
    assert_eq!(t.spend().1.output, 1);
}

#[test]
fn scan_keeps_the_error_a_turn_ended_on() {
    let limit = r#"{"type":"assistant","timestamp":"2026-09-23T10:02:00Z","isApiErrorMessage":true,"error":"rate_limit","message":{"id":"x","model":"<synthetic>","content":[{"type":"text","text":"You've hit your session limit · resets 5am (Europe/London)\nmore"}]}}"#;
    let unasked = r#"{"type":"assistant","timestamp":"2026-09-23T10:03:00Z","isApiErrorMessage":true,"message":{"id":"y","model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}}"#;
    let d = TempDir::new("halt");
    let p = d.0.join("s.jsonl");
    std::fs::write(&p, format!("{NEXT}\n{limit}\n{unasked}\n")).unwrap();
    let mut t = Totals::default();
    scan(&p, &mut t, &mut Vec::new()).unwrap();
    let h = t.halt.clone().expect("halt");
    assert_eq!(h.kind, "rate_limit");
    assert_eq!(
        h.text,
        "You've hit your session limit · resets 5am (Europe/London)"
    );
    // A real reply afterwards means it carried on.
    append(&p, &format!("{BLOCK2}\n"));
    scan(&p, &mut t, &mut Vec::new()).unwrap();
    assert!(t.halt.is_none(), "halt survived a reply: {:?}", t.halt);
}

#[test]
fn progress_is_the_last_count_that_moved() {
    let msg = |text: &str| {
        serde_json::to_string(&serde_json::json!([{"type": "text", "text": text}])).unwrap()
    };
    for (text, want) in [
        (
            "Wave done.\n\n**Lint** went 11,065 → **9,052** after W11, typecheck 3,044→2,782.",
            "Lint went 11,065 → 9,052 after W11, typecheck 3,044→2,782",
        ),
        ("| api/secrets | 200 -> 403 |", ""),
        (
            "Candidates dropped 206 → 154. Next up.",
            "Candidates dropped 206 → 154",
        ),
        ("nothing moved", ""),
        ("tests 12k -> ~9.5k left", "tests 12k -> ~9.5k"),
    ] {
        let raw = msg(text);
        let r: &RawValue = serde_json::from_str(&raw).unwrap();
        assert_eq!(progress_in(Some(r)), want, "{text:?}");
    }
}

#[test]
fn scan_counts_compactions() {
    let compact =
        r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}"#;
    let d = TempDir::new("compact");
    let p = d.0.join("s.jsonl");
    std::fs::write(&p, format!("{BLOCK1}\n{compact}\n{NEXT}\n{compact}\n")).unwrap();
    let mut t = Totals::default();
    scan(&p, &mut t, &mut Vec::new()).unwrap();
    assert_eq!((t.compacts, t.context()), (2, 1));
}

#[test]
fn dir_follows_writes_over_cd_and_cwd() {
    let line = |cwd: &str, content: &str| {
        format!(
            r#"{{"type":"assistant","cwd":"{cwd}","message":{{"id":"x","content":[{content}]}}}}"#
        )
    };
    let bash =
        |cmd: &str| format!(r#"{{"type":"tool_use","name":"Bash","input":{{"command":"{cmd}"}}}}"#);
    let mut t = Totals::default();
    let mut step = |l: String, want: &str| {
        consume(&mut t, l.as_bytes());
        assert_eq!(t.dir, want, "after {l}");
    };
    step(line("/r/main", r#"{"type":"text","text":"hi"}"#), "/r/main");
    step(
        line("/r/main", &bash("cd /r/pr6900 && git status")),
        "/r/pr6900",
    );
    step(line("/r/main", &bash("ls")), "/r/pr6900");
    step(line("/r/main", &bash("cd /tmp/x && ls")), "/r/pr6900");
    step(
        line("/r/other", r#"{"type":"text","text":"moved"}"#),
        "/r/other",
    );
    step(line("/r/other", &bash(r#"cd \"/r/q q\"; make"#)), "/r/q q");
    step(
        line(
            "/r/main",
            r#"{"type":"tool_use","name":"Write","input":{"file_path":"/Users/me/.claude/projects/p/memory/m.md"}}"#,
        ),
        "/r/main",
    );
    step(
        line(
            "/r/main",
            r#"{"type":"tool_use","name":"Edit","input":{"file_path":"/r/wt/a/b.go"}}"#,
        ),
        "/r/wt/a",
    );
    // Once it writes, git run from the main checkout doesn't move it back.
    step(line("/r/main", &bash("cd /r/main && git log")), "/r/wt/a");
    step(
        line("/r/other", r#"{"type":"text","text":"moved"}"#),
        "/r/wt/a",
    );
    step(
        line(
            "/r/other",
            &format!(
                r#"{{"type":"tool_use","name":"Edit","input":{{"file_path":"/r/wt2/c.go"}}}},{}"#,
                bash("cd /r/main")
            ),
        ),
        "/r/wt2",
    );
    // EnterWorktree: the move is known before the next reply.
    step(
        r#"{"type":"relocated","sessionId":"s","relocatedCwd":"/r/.claude/worktrees/w"}"#.into(),
        "/r/.claude/worktrees/w",
    );
    step(
        line("/r/.claude/worktrees/w", r#"{"type":"text","text":"in"}"#),
        "/r/.claude/worktrees/w",
    );
}

#[test]
fn pr_urls_match_go() {
    let mut got = Vec::new();
    pr_urls(
        br#"x https://github.com/a/b/pull/1 https://github.com/a/pull/2 https://github.com/o.r-g/r_e/pull/33x https://github.com/a/b/pull/"#,
        |u| got.push(u.to_string()),
    );
    assert_eq!(
        got,
        vec![
            "https://github.com/a/b/pull/1",
            "https://github.com/o.r-g/r_e/pull/33"
        ]
    );
}

#[test]
fn preview_reads_the_tail() {
    let d = TempDir::new("preview");
    let p = d.0.join("s.jsonl");
    let lines = [
        r#"{"type":"user","timestamp":"2026-09-23T10:00:00Z","message":{"role":"user","content":"fix the build"}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T10:00:01Z","effort":"high","message":{"id":"a","model":"claude-opus-5-5","content":[{"type":"text","text":" Looking. "},{"type":"tool_use","name":"Bash","input":{"command":"cd /r && pnpm test"}}],"usage":{"input_tokens":3,"cache_read_input_tokens":10,"cache_creation_input_tokens":5,"output_tokens":1}}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T10:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T10:00:03Z","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>x</system-reminder>"}]}}"#,
    ];
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    let pv = read_preview(&p, 1 << 20);
    assert_eq!(pv.text, "Looking.");
    assert_eq!(pv.tool, "Bash");
    assert_eq!(pv.tool_arg, "cd /r && pnpm test");
    assert_eq!(pv.doing, "running pnpm test");
    assert_eq!(pv.model, "claude-opus-5-5");
    assert_eq!(pv.effort, "high");
    assert_eq!(pv.last_user, "fix the build");
    assert_eq!(pv.context, 18);
    assert_eq!(
        pv.at,
        rush_core::time::parse_rfc3339("2026-09-23T10:00:03Z")
    );
    assert_eq!(pv.first.text, "fix the build");
    let roles: Vec<&str> = pv.recent.iter().map(|e| e.role.as_str()).collect();
    assert_eq!(roles, ["user", "assistant", "tool"]);
    assert_eq!(pv.recent[2].text, "Bash\0cd /r && pnpm test");
    // A window that misses the start still finds the first prompt.
    let pv = read_preview(&p, 300);
    assert_eq!(pv.first.text, "fix the build");
    assert!(read_preview(&d.0.join("none"), 10).tool.is_empty());
}

#[test]
fn recent_keeps_the_last_n() {
    let ls: Vec<String> = (0..5)
        .map(|i| format!(r#"{{"type":"user","message":{{"content":"m{i}"}}}}"#))
        .collect();
    let got = recent_events(ls.iter().map(|s| s.as_bytes()), 2);
    let texts: Vec<&str> = got.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, ["m3", "m4"]);
}

#[test]
fn subagents_found_and_counted() {
    let d = TempDir::new("subs");
    let main = d.0.join("s.jsonl");
    std::fs::write(&main, "{}\n").unwrap();
    let sub = d.0.join("s").join("subagents");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("agent-a.jsonl"), "{}\n").unwrap();
    std::fs::write(sub.join("agent-a.meta.json"), r#"{"spawnDepth":1}"#).unwrap();
    std::fs::write(sub.join("agent-b.jsonl"), "{}\n").unwrap();
    std::fs::write(sub.join("agent-b.meta.json"), r#"{"spawnDepth":2}"#).unwrap();
    std::fs::write(sub.join("agent-c.meta.json"), "{}").unwrap();
    assert_eq!(subagent_transcripts(&main).len(), 2);
    let now = rush_core::time::now();
    let st = read_subagent_stats(&main, now);
    assert_eq!((st.spawned, st.direct, st.nested), (3, 1, 1));
    let st = read_subagent_stats(&main, now + 120_000);
    assert_eq!((st.spawned, st.direct, st.nested), (3, 0, 0));
}
