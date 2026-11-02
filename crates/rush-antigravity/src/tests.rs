//! Port of internal/adapters/antigravity/antigravity_test.go:1-189 and
//! catalog_test.go (in catalog.rs), with a fake agy as a shell script.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use rush_core::adapter::{
    Adapter, Authenticator, Driver, HistoryFollower, Input, OnceReader, Registry, Said, Session,
    StartOptions, Tagged, TailReader, TranscriptLocator, read_history,
};
use rush_core::event::Event;

use crate::conn::launch_args;
use crate::history::history_path;
use crate::{Antigravity, KIND};

#[test]
fn launch_preserves_permission_and_conversation() {
    let args = launch_args(&StartOptions {
        resume: true,
        session_id: "saved".into(),
        model: "gemini-example".into(),
        mode: "plan".into(),
        effort: "high".into(),
        ..StartOptions::default()
    })
    .unwrap();
    let got = args.join(" ");
    assert!(
        got.contains("--conversation saved") && got.contains("--mode plan"),
        "{got}"
    );
    assert!(
        !got.contains("skip-permissions") && !got.contains("experimental-acp"),
        "{got}"
    );
    for o in [
        StartOptions {
            mode: "yolo".into(),
            ..StartOptions::default()
        },
        StartOptions {
            mode: "autoEdit".into(),
            ..StartOptions::default()
        },
        StartOptions {
            fork: true,
            ..StartOptions::default()
        },
        StartOptions {
            resume: true,
            ..StartOptions::default()
        },
        StartOptions {
            api_key: "k".into(),
            ..StartOptions::default()
        },
    ] {
        assert!(
            launch_args(&o).is_err(),
            "silently accepted incompatible options"
        );
    }
    let args = launch_args(&StartOptions {
        mode: "always-proceed".into(),
        ..StartOptions::default()
    })
    .unwrap();
    assert!(args.iter().any(|a| a == "--dangerously-skip-permissions"));
}

const SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' '{"event":"init","conversation_id":"native-id","init":{"model":"test"}}'
i=0
while IFS= read -r input; do
 case "$input" in *'"event":"user"'*) ;; *) exit 9;; esac
 i=$((i+1))
 printf '{"event":"result","result":{"status":"SUCCESS","response":"reply","num_turns":%s,"usage":{"input_tokens":%s}}}\n' "$i" "$i"
done
"#;

#[test]
fn persistent_turns_and_saved_history() {
    let home = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    let bin = bin_dir.path().join("agy");
    std::fs::write(&bin, SCRIPT).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();

    let (tx, rx) = mpsc::channel();
    let c = Antigravity::new()
        .start(
            StartOptions {
                binary: bin.to_string_lossy().into_owned(),
                session_id: "provisional".into(),
                ..StartOptions::default()
            },
            Arc::new(Tagged { key: 0u8, tx }),
        )
        .unwrap();
    assert!(c.pid().is_some());
    assert!(
        c.send(Input {
            images: vec!["x.png".into()],
            ..Input::default()
        })
        .is_err(),
        "image silently dropped"
    );
    assert!(
        c.send(Input {
            text: "/help".into(),
            ..Input::default()
        })
        .is_err()
    );
    for text in ["first", "second"] {
        c.send(Input {
            text: text.into(),
            ..Input::default()
        })
        .unwrap();
        loop {
            match rx
                .recv_timeout(Duration::from_secs(5))
                .expect("turn timed out")
            {
                Said::Event(_, Event::TurnEnd(e)) => {
                    assert_eq!(e.reason, "done", "{e:?}");
                    break;
                }
                Said::Event(..) => {}
                Said::Ended(..) => panic!("process closed"),
            }
        }
    }
    c.close().unwrap();
    // Closed by rush: it ends, with nothing said wrong.
    let mut ended = 0;
    while let Ok(s) = rx.recv_timeout(Duration::from_secs(5)) {
        // ended is last: nothing comes after it.
        match s {
            Said::Ended(_, err) => {
                assert!(err.is_none());
                ended += 1;
                break;
            }
            Said::Event(_, Event::TurnEnd(e)) => panic!("closing said {e:?}"),
            Said::Event(..) => {}
        }
    }
    assert_eq!(ended, 1);
    assert!(
        c.send(Input {
            text: "late".into(),
            ..Input::default()
        })
        .is_err()
    );

    let a = Antigravity::new();
    let s = Session {
        id: "native-id".into(),
        ..Session::default()
    };
    let evs = read_history(&a, &s, 0).unwrap();
    let prompts: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            Event::Message(m) if m.role == "user" => Some(m.parts[0].text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prompts,
        ["first", "second"],
        "lost prompts across init/resume"
    );
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::Init(i) if i.session_id == "native-id"))
    );
    let (_, cut) = a.history_tail(&s, 100).unwrap();
    assert!(cut);
    let (tail, cut) = a.history_tail(&s, 1 << 20).unwrap();
    assert!(!cut);
    assert_eq!(tail.len(), evs.len());
    // Followed, it's the same, then nothing more.
    let mut f = a.follow_history(&s).unwrap();
    let sl = f.next(1 << 20).unwrap();
    assert_eq!(sl.events, evs);
    assert!(f.next(1 << 20).unwrap().events.is_empty());
    assert!(
        !a.transcript_path(&Default::default(), "", "native-id")
            .is_empty()
    );
    assert!(
        history_path(home.path(), "../elsewhere").is_none(),
        "unsafe history path"
    );
    rush_state::set_thread_dir(None);
}

/// agy that goes mid-turn says why, as a turn's error.
#[test]
fn exit_mid_turn_is_an_error() {
    let home = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(home.path().to_path_buf()));
    let bin = home.path().join("agy");
    std::fs::write(
        &bin,
        "#!/bin/sh\nread -r x\necho 'out of quota' >&2\nexit 3\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (tx, rx) = mpsc::channel();
    let c = Antigravity::new()
        .start(
            StartOptions {
                binary: bin.to_string_lossy().into_owned(),
                ..StartOptions::default()
            },
            Arc::new(Tagged { key: 0u8, tx }),
        )
        .unwrap();
    c.send(Input {
        text: "go".into(),
        ..Input::default()
    })
    .unwrap();
    let mut err = String::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Said::Event(_, Event::TurnEnd(e)) => err = e.err,
            Said::Event(..) => {}
            Said::Ended(..) => break,
        }
    }
    assert_eq!(err, "out of quota");
    c.close().unwrap();
    rush_state::set_thread_dir(None);
}

#[test]
fn reads_one_shot_runs() {
    let a = Antigravity::new();
    let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
    let o = a
        .read_once(&[
            "-p".into(),
            "fix it".into(),
            "--model".into(),
            "gemini-x".into(),
        ])
        .unwrap();
    assert_eq!(
        (o.prompt.as_str(), o.model.as_str()),
        ("fix it", "gemini-x")
    );
    let o = a
        .read_once(&args(
            "--prompt=hi --dangerously-skip-permissions --output-format text",
        ))
        .unwrap();
    assert_eq!(
        (o.prompt.as_str(), o.mode.as_str()),
        ("hi", "always-proceed")
    );
    assert!(a.read_once(&args("--output-format json -p hi")).is_none());
    assert!(
        a.read_once(&args("hello")).is_none(),
        "a prompt for its own screen"
    );
    assert!(a.read_once(&args("-p hi --sandbox")).is_none());
    assert!(a.read_once(&args("--model x")).is_none(), "no prompt");
}

#[test]
fn registers() {
    let reg = Registry::with_env("/nonexistent", "/nonexistent");
    let a = Arc::new(Antigravity::with_registry(reg.clone()));
    reg.register_ref(a.clone());
    let got = reg.get(KIND).unwrap();
    assert_eq!(got.name(), "Antigravity CLI");
    assert!(
        got.driver().is_some() && got.history_reader().is_some() && got.runner_snapshot().is_some()
    );
    assert_eq!(got.made().unwrap().maker(), "Google");
    if reg.path(KIND).is_empty() {
        assert!(a.profiles().is_empty(), "not installed, yet a profile");
        assert!(a.sign_in_command(&Default::default()).is_err());
    }
    reg.unregister(KIND);
}
