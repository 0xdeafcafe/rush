//! A fake `claude` that replays recorded stream-json (headless_test.go).

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::event::tests::*;
use super::*;

/// A script at dir/claude that runs body, its own args and environment
/// written beside log.
pub(crate) fn fake_claude(dir: &Path, body: &str) -> (PathBuf, PathBuf) {
    let log = dir.join("stdin.log");
    let l = log.display();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"{l}.args\"\nenv > \"{l}.env\"\n{}",
        body.replace("$LOG", &l.to_string())
    );
    let bin = dir.join("claude");
    std::fs::write(&bin, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

/// Plays one turn: it asks permission for a tool, then echoes back the
/// answer it got, and logs every stdin line.
fn one_turn() -> String {
    format!(
        r#"read -r prompt; printf '%s\n' "$prompt" >> "$LOG"
echo '{INIT}'
echo '{TOOL_USE}'
echo '{ASK}'
read -r answer; printf '%s\n' "$answer" >> "$LOG"
echo '{TOOL_RESULT}'
echo '{RESULT}'
while read -r more; do printf '%s\n' "$more" >> "$LOG"; done
"#
    )
}

enum Got {
    Ev(Event),
    End(Option<Ended>),
}

fn run(o: &Options) -> (Arc<Session>, mpsc::Receiver<Got>) {
    let (s, r) = start(o).unwrap();
    let (tx, rx) = mpsc::channel();
    let t2 = tx.clone();
    r.spawn(
        s.clone(),
        move |ev| {
            let _ = tx.send(Got::Ev(ev));
        },
        move |e| {
            let _ = t2.send(Got::End(e));
        },
    );
    (s, rx)
}

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn session_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, log) = fake_claude(dir.path(), &one_turn());
    let (s, rx) = run(&Options {
        dir: dir.path().display().to_string(),
        resume: "s-1".into(),
        permission_mode: "acceptEdits".into(),
        binary: bin.display().to_string(),
        ..Options::default()
    });
    s.send("say hi").unwrap();
    let mut got = 0;
    loop {
        let Got::Ev(ev) = rx.recv_timeout(WAIT).expect("an event") else {
            panic!("ended early after {got} events");
        };
        got += 1;
        match ev {
            Event::PermissionRequest(r) => s.allow(&r, None, true).unwrap(),
            Event::Result(_) => break,
            _ => {}
        }
    }
    assert_eq!(got, 5);
    s.interrupt().unwrap();
    s.stop(Duration::from_secs(2)).unwrap();
    assert!(matches!(rx.recv_timeout(WAIT), Ok(Got::End(None))));

    let args = std::fs::read_to_string(format!("{}.args", log.display())).unwrap();
    for want in [
        "--permission-prompt-tool stdio",
        "--resume s-1",
        "--permission-mode acceptEdits",
        "--input-format stream-json",
    ] {
        assert!(args.contains(want), "args {args:?} missing {want:?}");
    }
    let env = std::fs::read_to_string(format!("{}.env", log.display())).unwrap();
    assert!(env.contains("CLAUDE_CODE_QUESTION_PREVIEW_FORMAT=markdown"));
    let b = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = b.trim().lines().collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    let prompt: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(prompt["type"], "user");
    assert_eq!(prompt["message"]["content"], "say hi");
    let answer: Value = serde_json::from_str(lines[1]).unwrap();
    let r = &answer["response"];
    assert_eq!(answer["type"], "control_response");
    assert_eq!(
        (&r["subtype"], &r["request_id"], &r["response"]["behavior"]),
        (&json!("success"), &json!("req-1"), &json!("allow"))
    );
    assert_eq!(r["response"]["updatedInput"]["command"], "echo hi");
    assert!(r["response"]["updatedPermissions"].is_array());
    assert_eq!(r["response"]["decisionClassification"], "user_permanent");
    assert!(
        lines[2].contains(r#""subtype":"interrupt""#),
        "{}",
        lines[2]
    );
}

#[test]
fn an_unknown_request_is_refused_and_a_failure_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, log) = fake_claude(
        dir.path(),
        r#"echo '{"type":"control_request","request_id":"x-1","request":{"subtype":"hook_callback"}}'
read -r answer; printf '%s\n' "$answer" >> "$LOG"
echo 'broken: no key' >&2
exit 3
"#,
    );
    let (s, rx) = run(&Options {
        binary: bin.display().to_string(),
        ..Options::default()
    });
    let Got::End(Some(e)) = rx.recv_timeout(WAIT).unwrap() else {
        panic!("the refused request reached the handler");
    };
    assert!(e.exited);
    assert_eq!(e.text, "exit status 3: broken: no key");
    let b = std::fs::read_to_string(&log).unwrap();
    let v: Value = serde_json::from_str(b.trim()).unwrap();
    assert_eq!(v["response"]["subtype"], "error");
    assert_eq!(v["response"]["request_id"], "x-1");
    assert_eq!(v["response"]["error"], "not supported by rush");
    // An exit is no failure to stop.
    s.stop(Duration::from_secs(1)).unwrap();
    assert!(s.send("late").is_err(), "stopped sessions take nothing");
}

#[test]
fn tap_sees_every_line_and_skip_keeps_them_from_the_handler() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, _) = fake_claude(
        dir.path(),
        &format!("echo '{INIT}'\necho '{DELTA}'\necho '{RESULT}'\n"),
    );
    let tapped = Arc::new(Mutex::new(Vec::new()));
    let t = tapped.clone();
    let (_s, rx) = run(&Options {
        binary: bin.display().to_string(),
        tap: Some(Arc::new(move |l: &[u8]| {
            lock(&t).push(String::from_utf8_lossy(l).into_owned())
        })),
        skip: Some(Arc::new(|l: &[u8]| {
            l.starts_with(br#"{"type":"stream_event""#)
        })),
        ..Options::default()
    });
    let mut kinds = Vec::new();
    while let Ok(Got::Ev(ev)) = rx.recv_timeout(WAIT) {
        kinds.push(match ev {
            Event::Init(_) => "init",
            Event::Result(_) => "result",
            _ => "other",
        });
    }
    assert_eq!(kinds, ["init", "result"]);
    assert_eq!(lock(&tapped).len(), 3);
}

#[test]
fn stop_ends_one_that_wont_go() {
    let dir = tempfile::tempdir().unwrap();
    // Ignores its stdin closing, and TERM.
    let (bin, _) = fake_claude(
        dir.path(),
        &format!("trap '' TERM\necho '{INIT}'\nwhile :; do sleep 1; done\n"),
    );
    let (s, rx) = run(&Options {
        binary: bin.display().to_string(),
        ..Options::default()
    });
    assert!(matches!(rx.recv_timeout(WAIT), Ok(Got::Ev(Event::Init(_)))));
    s.stop(Duration::from_millis(50)).unwrap();
    let Got::End(Some(e)) = rx.recv_timeout(WAIT).unwrap() else {
        panic!()
    };
    assert_eq!(e.text, "signal: killed");
    // SAFETY: kill(2) with signal 0 only asks.
    assert_ne!(unsafe { libc::kill(-s.pid(), 0) }, 0, "the group is gone");
}

#[test]
fn content_places_images_at_their_markers() {
    let im = |d: &str| Image {
        media_type: "image/png".into(),
        data: d.as_bytes().to_vec(),
    };
    let (a, b, c) = (im("a"), im("b"), im("c"));
    let kinds = |blocks: Vec<Value>| {
        blocks
            .iter()
            .map(|bl| {
                if bl["type"] == "text" {
                    return format!("text:{}", bl["text"].as_str().unwrap());
                }
                let data = STANDARD
                    .decode(bl["source"]["data"].as_str().unwrap())
                    .unwrap();
                format!("image:{}", String::from_utf8(data).unwrap())
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    for (text, imgs, want) in [
        (
            "see [Image #2] then [Image #1] ok",
            vec![a.clone(), b.clone(), c.clone()],
            "text:see [Image #2] | image:b | text: then [Image #1] | image:a | text: ok | image:c",
        ),
        (
            "no markers here",
            vec![a.clone(), b.clone()],
            "image:a | image:b | text:no markers here",
        ),
        // A marker for an image that isn't there stays text.
        (
            "[Image #5] and [Image #1]",
            vec![a.clone()],
            "text:[Image #5] and [Image #1] | image:a",
        ),
    ] {
        assert_eq!(kinds(content(text, &imgs)), want, "{text}");
    }
}

#[test]
fn runs_session_by_its_args() {
    let sid = "8c76706f-1c00-4aed-9c6d-7509f3033943";
    let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    for (args, want) in [
        (
            a(&[
                "claude",
                "-p",
                "--input-format",
                "stream-json",
                "--resume",
                sid,
            ]),
            true,
        ),
        (
            a(&[
                "claude",
                "-p",
                "--input-format",
                "stream-json",
                "--session-id",
                sid,
            ]),
            true,
        ),
        (a(&["claude", "--resume", sid]), false),
        (
            a(&[
                "claude",
                "-p",
                "--input-format",
                "stream-json",
                "--resume",
                "other",
            ]),
            false,
        ),
        (a(&["vim", sid]), false),
    ] {
        assert_eq!(runs_session(&args, sid), want, "{args:?}");
    }
}

#[test]
fn rewinds_files_through_a_claude_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, log) = fake_claude(
        dir.path(),
        r#"read -r req; printf '%s\n' "$req" >> "$LOG"
echo '{"type":"control_response","response":{"subtype":"success","request_id":"rush-1","response":{"canRewind":true,"filesChanged":["a.go"],"insertions":3}}}'
while read -r more; do :; done
"#,
    );
    let o = Options {
        binary: bin.display().to_string(),
        resume: "s-1".into(),
        ..Options::default()
    };
    let r = rewind_files(&o, "u-9", true).unwrap();
    assert_eq!(
        r,
        FileRewind {
            can_rewind: true,
            files: vec!["a.go".into()],
            insertions: 3,
            ..FileRewind::default()
        }
    );
    let sent = std::fs::read_to_string(&log).unwrap();
    assert!(
        sent.contains(r#""subtype":"rewind_files""#) && sent.contains(r#""user_message_id":"u-9""#)
    );
    let env = std::fs::read_to_string(format!("{}.env", log.display())).unwrap();
    assert!(env.contains(CHECKPOINT_ENV));
    assert!(rewind_files(&Options::default(), "u", false).is_err());
}

#[test]
fn recaps_with_a_throwaway_fork() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, log) = fake_claude(
        dir.path(),
        r#"printf '%s\n' '{"type":"result","is_error":false,"result":"  - tried X\n"}'"#,
    );
    let o = Options {
        binary: bin.display().to_string(),
        resume: "s-1".into(),
        model: "haiku".into(),
        ..Options::default()
    };
    assert_eq!(recap(&o, "fix it").unwrap(), "- tried X");
    let args = std::fs::read_to_string(format!("{}.args", log.display())).unwrap();
    assert!(args.starts_with("-p --resume s-1 --fork-session --no-session-persistence --max-turns 1 --output-format json --model haiku I'm about to rewind"));
    let (bad, _) = fake_claude(dir.path(), "echo 'no key' >&2\nexit 1\n");
    let e = recap(
        &Options {
            binary: bad.display().to_string(),
            ..o
        },
        "x",
    )
    .unwrap_err();
    assert_eq!(e.to_string(), "couldn't recap: exit status 1: no key");
}
