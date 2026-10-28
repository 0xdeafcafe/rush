//! Reading transcripts: after internal/convo/convo_test.go (tailTranscript,
//! TestTailLongLines, TestShellTurnsAndStyling, TestInjectedPrompts,
//! TestCompactDivider, TestNotice, TestHistory, TestTailSizeDuringFetch,
//! TestHistoryStopsPastItsMoment, TestPromptJoinsTextAroundImages),
//! tailfrom_test.go, hooks_test.go and usermessages_test.go
//! (TestNativeUserPicture), the model's side of each.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use rush_claude::decode_message;
use rush_convo::transcript::{clean_prompt, prompt};
use rush_convo::{Item, Session, Status, Tail, history, injected};
use rush_core::time::parse_rfc3339;

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("rush-convo-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }
    fn file(&self, lines: &[&str]) -> PathBuf {
        let p = self.0.join("s.jsonl");
        fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read_all(path: &PathBuf) -> Session {
    let mut s = Session::new();
    Tail::new(path, decode_message).read(&mut s).unwrap();
    s
}

const LINES: [&str; 7] = [
    r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"fix the test"},"origin":{"kind":"human"}}"#,
    r#"{"type":"assistant","timestamp":"2026-09-23T20:00:02Z","effort":"high","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./..."}}],"usage":{"input_tokens":5,"output_tokens":3}}}"#,
    r#"{"type":"user","timestamp":"2026-09-23T20:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Exit code 1\nFAIL x","is_error":true}]},"toolUseResult":{"stdout":"FAIL x","stderr":""}}"#,
    r#"{"type":"assistant","timestamp":"2026-09-23T20:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Fixed it."}]}}"#,
    r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:00:13Z"}"#,
    r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"caveat"}}"#,
    r#"{"type":"user","timestamp":"2026-09-23T20:05:00Z","message":{"role":"user","content":"<command-name>/compact</command-name><command-args></command-args>"}}"#,
];

/// Read, or fetch then apply as the UI follows a transcript: the same.
fn tail_transcript(name: &str, fetch: bool) {
    let tmp = Tmp::new(name);
    let path = tmp.0.join("s.jsonl");
    fs::write(&path, LINES[..3].join("\n") + "\n" + &LINES[3][..20]).unwrap();
    let mut tl = Tail::new(&path, decode_message);
    let mut s = Session::new();
    let read = |tl: &mut Tail, s: &mut Session| -> bool {
        if fetch {
            tl.fetch().unwrap().apply(s)
        } else {
            tl.read(s).unwrap()
        }
    };
    assert!(read(&mut tl, &mut s), "first read");
    assert_eq!(s.turns.len(), 1);
    let t1 = s.step(s.step_by_call("t1").unwrap());
    assert_eq!((t1.status, t1.exit), (Status::Failed, 1));
    assert_eq!(&*s.turns[0].effort, "high");
    assert_eq!(s.cwd, "/work");
    // The rest of the half-written line and the lines after it arrive later.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    write!(f, "{}\n{}\n", &LINES[3][20..], LINES[4..].join("\n")).unwrap();
    drop(f);
    assert!(read(&mut tl, &mut s), "second read saw nothing");
    assert_eq!(s.turns.len(), 2);
    assert!(!s.turns[0].live);
    assert_eq!(s.turns[0].outcome(), "Fixed it.");
    assert_eq!(s.turns[1].cause, "/compact");
    assert!(!read(&mut tl, &mut s), "nothing new should mean no change");
    // Rewritten shorter: the conversation starts over.
    fs::write(&path, format!("{}\n", LINES[0])).unwrap();
    assert!(read(&mut tl, &mut s));
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].prompt, "fix the test");
}

#[test]
fn tail_transcript_read() {
    tail_transcript("read", false);
}

#[test]
fn tail_transcript_fetch_apply() {
    tail_transcript("fetch", true);
}

/// Lines longer than a read's buffer, one after another, each read whole.
#[test]
fn tail_long_lines() {
    let tmp = Tmp::new("long");
    let mut b = String::new();
    for (i, n) in [200 << 10, 90 << 10, 300 << 10].into_iter().enumerate() {
        b += &format!(
            r#"{{"type":"user","timestamp":"2026-09-23T20:0{i}:00Z","message":{{"role":"user","content":"ask {i} {}"}}}}"#,
            "x".repeat(n)
        );
        b += "\n";
        b += &format!(
            r#"{{"type":"assistant","timestamp":"2026-09-23T20:0{i}:01Z","message":{{"id":"m{i}","role":"assistant","content":[{{"type":"text","text":"done {i}"}}]}}}}"#
        );
        b += "\n";
        b += &format!(
            r#"{{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:0{i}:02Z"}}"#
        );
        b += "\n";
    }
    let path = tmp.0.join("s.jsonl");
    fs::write(&path, b).unwrap();
    let s = read_all(&path);
    assert_eq!(s.turns.len(), 3);
    for (i, t) in s.turns.iter().enumerate() {
        assert!(t.prompt.starts_with(&format!("ask {i} x")), "turn {i}");
        assert_eq!(t.outcome(), format!("done {i}"));
    }
}

/// A transcript read from part way through a turn, whose tool result's
/// call was cut off, ends as the whole transcript does.
#[test]
fn tail_from_mid_turn() {
    let lines = [
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"first question"}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"first answer"}]}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:01:00Z","message":{"role":"user","content":"second question"}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T20:01:01Z","message":{"id":"m2","role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"go vet ./..."}}]}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:01:05Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"ok"}]},"toolUseResult":{"stdout":"ok","stderr":""}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T20:01:06Z","message":{"id":"m3","role":"assistant","content":[{"type":"text","text":"second answer"}]}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:02:00Z","message":{"role":"user","content":"third question"}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T20:02:01Z","message":{"id":"m4","role":"assistant","content":[{"type":"tool_use","id":"t4","name":"Bash","input":{"command":"go test ./..."}}]}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:02:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t4","content":"PASS"}]},"toolUseResult":{"stdout":"PASS","stderr":""}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T20:02:10Z","message":{"id":"m5","role":"assistant","content":[{"type":"text","text":"third answer"}]}}"#,
    ];
    let tmp = Tmp::new("from");
    let path = tmp.file(&lines);
    let all = lines.join("\n") + "\n";
    let whole = read_all(&path);

    // Cut inside t2's call: its result arrives with nothing to land on.
    let cut = all.find(r#""tool_use","id":"t2""#).unwrap();
    let mut tl = Tail::from_end(&path, (all.len() - cut) as u64, decode_message);
    let mut s = Session::new();
    tl.read(&mut s).unwrap();
    assert!(s.partial);
    assert_eq!(s.turns.len(), 2);
    assert!(!s.turns[0].live);
    assert_eq!(s.turns[1].prompt, "third question");
    let (w, g) = (whole.turns.last().unwrap(), s.turns.last().unwrap());
    assert_eq!(w.items.len(), g.items.len());
    assert_eq!(w.outcome(), g.outcome());
    assert_eq!(tl.size(), all.len() as u64);

    // A cut that falls between two lines keeps the one after it.
    let at = all
        .find(r#"{"type":"user","timestamp":"2026-09-23T20:02:00Z""#)
        .unwrap();
    let mut tl = Tail::from_end(&path, (all.len() - at) as u64, decode_message);
    let mut s = Session::new();
    tl.read(&mut s).unwrap();
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].prompt, "third question");
}

/// What a start hook handed the agent is kept apart, never a message of
/// yours.
#[test]
fn start_hooks() {
    let tmp = Tmp::new("hooks");
    let path = tmp.file(&[
        r#"{"type":"attachment","attachment":{"type":"hook_success","hookName":"SessionStart:startup","hookEvent":"SessionStart","content":"PONYTAIL MODE ACTIVE — level: full\n\nYou are a lazy senior developer."}}"#,
        r#"{"type":"attachment","attachment":{"type":"hook_success","hookName":"SessionStart:startup","hookEvent":"SessionStart","content":"","stdout":"{\"hookSpecificOutput\":{}}"}}"#,
        r#"{"type":"attachment","attachment":{"type":"hook_success","hookName":"PreToolUse:Bash","hookEvent":"PreToolUse","content":"not a start hook"}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"fix the test"}}"#,
    ]);
    let s = read_all(&path);
    assert_eq!(s.hooks.len(), 1);
    assert_eq!(&*s.hooks[0].event, "SessionStart");
    assert!(s.hooks[0].text.starts_with("PONYTAIL MODE ACTIVE"));
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].prompt, "fix the test");
}

/// A message rush's inbox handed over mid-turn is yours, said mid-turn.
#[test]
fn told_mid_turn() {
    let tmp = Tmp::new("told");
    let path = tmp.file(&[
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"go"}}"#,
        r#"{"type":"attachment","attachment":{"type":"hook_additional_context","hookEvent":"PostToolUse","content":["The user sent you this message directly while you work. Take it into account, then carry on:\n\nalso the docs"]}}"#,
    ]);
    let s = read_all(&path);
    assert!(matches!(&s.turns[0].items[0], Item::Interject(it) if it.text == "also the docs"));
}

#[test]
fn shell_turns() {
    let tmp = Tmp::new("shell");
    let path = tmp.file(&[
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"<bash-input>ls /tmp</bash-input>"}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:00:01Z","message":{"role":"user","content":"<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr>"}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:01:00Z","message":{"role":"user","content":"/design:design-critique look at @internal/ui/view.go and https://example.com [Image #1]"}}"#,
    ]);
    let s = read_all(&path);
    assert_eq!(s.turns.len(), 2);
    assert_eq!(s.turns[0].prompt, "! ls /tmp");
    assert!(!s.turns[0].live);
    let st = s.step(s.step_by_call("you-1").unwrap());
    assert_eq!(st.status, Status::Ok);
    assert!(st.output.contains("a\nb"));
    assert_eq!(st.call().input.command, "ls /tmp");
    assert_eq!(st.out().stdout, "a\nb");
}

#[test]
fn injected_prompts() {
    let cases = [
        (
            "<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n<summary>Agent \"lint lane\" finished</summary>\n</task-notification>",
            "background task · completed",
            r#"Agent "lint lane" finished"#,
        ),
        (
            r#"<cross-session-message from="uds:/x" from-name="agent wrapper">rush: my edits are committed</cross-session-message>"#,
            "message from agent wrapper",
            "rush: my edits are committed",
        ),
        (
            "<rush-pr-news pr=\"o/r#8\">\nrush is following o/r#8 for you (rush pr follow). Only trusted authors' words are shown.\n\nrush can't reach GitHub; it keeps trying.\n</rush-pr-news>",
            "rush · o/r#8",
            "rush can't reach GitHub; it keeps trying.",
        ),
        (
            "<rush-pr-news pr=\"o/r#9\">\nrush is following o/r#9 for you (rush pr follow).\n\nNew since rush last told you:\n- check lint failed\n- a review from bot\n</rush-pr-news>",
            "rush · o/r#9",
            "check lint failed",
        ),
        (
            "<agent-message from=\"x\"><b>done</b> with it</agent-message>",
            "a subagent reported back",
            "done  with it",
        ),
    ];
    for (input, from, text) in cases {
        assert_eq!(
            injected(input),
            Some((from.to_owned(), text.to_owned())),
            "{input}"
        );
    }
    assert_eq!(injected("please fix <b>this</b>"), None);
}

#[test]
fn compact_divider() {
    let tmp = Tmp::new("compact");
    let path = tmp.file(&[
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"do the thing"}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T20:00:05Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Done."}]}}"#,
        r#"{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:00:06Z"}"#,
        r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-09-23T20:01:00Z","compactMetadata":{"trigger":"auto","preTokens":592789,"postTokens":11658}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T20:01:01Z","message":{"role":"user","content":"This session is being continued from a previous conversation that ran out of context. Summary: stuff"}}"#,
    ]);
    let s = read_all(&path);
    assert_eq!(s.turns.len(), 1, "the summary opened a turn");
    match s.turns[0].items.last().unwrap() {
        Item::Compact { compact, summary } => {
            assert_eq!(
                (&*compact.trigger, compact.before, compact.after),
                ("auto", 592789, 11658)
            );
            assert!(summary.ends_with("Summary: stuff"));
        }
        it => panic!("{it:?}"),
    }
    assert_eq!(s.context, 11658);
}

#[test]
fn notice() {
    let tmp = Tmp::new("notice");
    let path = tmp.file(&[
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"/rush"}}"#,
        r#"{"type":"system","subtype":"informational","level":"warning","timestamp":"2026-09-23T20:00:01Z","content":"Unknown command: /rush. Did you mean /stop?"}"#,
    ]);
    let s = read_all(&path);
    assert!(matches!(
        s.turns[0].items.last(),
        Some(Item::Notice { text, level }) if &**text == "Unknown command: /rush. Did you mean /stop?" && &**level == "warning"
    ));
}

#[test]
fn history_before() {
    let tmp = Tmp::new("history");
    let path = tmp.file(&[
        r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"first ask"}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-23T20:00:05Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Done."}]}}"#,
        r#"{"type":"user","timestamp":"2026-09-23T21:00:00Z","message":{"role":"user","content":"after the host started"}}"#,
    ]);
    let s = history(&path, parse_rfc3339("2026-09-23T20:30:00Z"), decode_message);
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].prompt, "first ask");
    assert!(!s.turns[0].live);
}

/// History keeps what was said before its moment, and stops reading once
/// the transcript is well past it.
#[test]
fn history_stops_past_its_moment() {
    let tmp = Tmp::new("past");
    let line = |at: &str, text: &str| {
        format!(
            r#"{{"type":"user","timestamp":"{at}","message":{{"role":"user","content":"{text}"}}}}"#
        )
    };
    let (a, b, c) = (
        line("2026-09-23T20:00:00Z", "before"),
        line("2026-09-23T20:10:00Z", "after"),
        line("2026-09-23T20:30:00Z", "long after"),
    );
    let path = tmp.file(&[&a, &b, &c, "not json at all, never reached"]);
    let s = history(&path, parse_rfc3339("2026-09-23T20:05:00Z"), decode_message);
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].prompt, "before");
    // It read no further than the line past its moment.
    assert_eq!(s.applied(), 2 + 1);
}

#[test]
fn fetch_size() {
    let tmp = Tmp::new("size");
    let line = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
    let path = tmp.0.join("s.jsonl");
    fs::write(&path, format!("{line}\n").repeat(2000)).unwrap();
    let mut tl = Tail::new(&path, decode_message);
    let f = tl.fetch().unwrap();
    assert_eq!(f.size, fs::metadata(&path).unwrap().len());
    assert_eq!(tl.size(), f.size);
    let mut s = Session::new();
    assert!(f.apply(&mut s));
    assert_eq!(s.turns.len(), 2000);
}

/// A message split around its images at their markers reads back as the
/// text that was typed.
#[test]
fn prompt_joins_text_around_images() {
    let raw = r#"[{"type":"text","text":"see [Image #1]"},{"type":"image"},{"type":"text","text":" and [Image #2]"},{"type":"image"},{"type":"text","text":" ok"}]"#;
    let (text, images) = prompt(raw).unwrap();
    assert_eq!(text, "see [Image #1] and [Image #2] ok");
    assert_eq!(images.len(), 2);
    let (text, _) =
        prompt(r#"[{"type":"text","text":"one"},{"type":"text","text":"two"}]"#).unwrap();
    assert_eq!(text, "one\ntwo");
    assert!(prompt(r#"[{"type":"tool_result"}]"#).is_none());
    assert!(prompt(r#""  ""#).is_none());
    assert_eq!(
        clean_prompt("hi <system-reminder>x</system-reminder> there"),
        "hi  there"
    );
}

/// A subagent's own transcript: its sidechain lines are the story, and a
/// prompt's picture is kept.
#[test]
fn subagent_picture() {
    let tmp = Tmp::new("sub");
    let path = tmp.file(&[
        r#"{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":[{"type":"text","text":"look"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"aW1hZ2U="}}]}}"#,
    ]);
    let mut s = Session::new();
    Tail::subagent(&path, decode_message).read(&mut s).unwrap();
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.turns[0].prompt, "look");
    assert_eq!(s.turns[0].pictures.len(), 1);
    assert_eq!(s.turns[0].pictures[0].data, "aW1hZ2U=");
    // The main tail skips them.
    assert!(read_all(&path).turns.is_empty());
}
