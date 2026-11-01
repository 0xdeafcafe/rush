//! rush-voice through its public API: the wire, the bundle's makings and
//! what the window is told. Nothing here asks macOS for the microphone or
//! speech recognition.

use std::path::Path;

use rush_voice::{Command, Live, bundle, is_bundle_program, level, split};

#[test]
fn commands_on_the_wire() {
    let cases = [
        (
            Command::Start {
                device: "BuiltInMicrophoneDevice".into(),
                context: Vec::new(),
            },
            r#"{"cmd":"start","device":"BuiltInMicrophoneDevice"}"#,
        ),
        (
            Command::Start {
                device: String::new(),
                context: Vec::new(),
            },
            r#"{"cmd":"start","device":""}"#,
        ),
        (
            Command::Start {
                device: String::new(),
                context: vec!["Redis".into(), "rushd".into()],
            },
            r#"{"cmd":"start","device":"","context":["Redis","rushd"]}"#,
        ),
        (Command::Stop, r#"{"cmd":"stop"}"#),
        (Command::Cancel, r#"{"cmd":"cancel"}"#),
    ];
    for (cmd, want) in cases {
        assert_eq!(serde_json::to_string(&cmd).unwrap(), want);
        assert_eq!(serde_json::from_str::<Command>(want).unwrap(), cmd);
    }
}

#[test]
fn changes_on_the_wire() {
    let cases = [
        (
            Live {
                phase: "listening".into(),
                settled: "hello".into(),
                guessing: "the world".into(),
                level: 42,
                error: String::new(),
            },
            r#"{"phase":"listening","settled":"hello","guessing":"the world","level":42,"error":""}"#,
        ),
        (
            Live {
                phase: "failed".into(),
                error: "no mic".into(),
                ..Live::default()
            },
            r#"{"phase":"failed","settled":"","guessing":"","level":0,"error":"no mic"}"#,
        ),
    ];
    for (live, want) in cases {
        assert_eq!(serde_json::to_string(&live).unwrap(), want);
        assert_eq!(serde_json::from_str::<Live>(want).unwrap(), live);
    }
    // Missing fields are empty, as yap may leave some out.
    let l: Live = serde_json::from_str(r#"{"phase":"done","settled":"hi"}"#).unwrap();
    assert_eq!(
        l,
        Live {
            phase: "done".into(),
            settled: "hi".into(),
            ..Live::default()
        }
    );
}

#[test]
fn which_changes_end_it() {
    for (phase, end) in [
        ("listening", false),
        ("finishing", false),
        ("done", true),
        ("failed", true),
        ("hidden", true),
        ("", false),
    ] {
        let l = Live {
            phase: phase.into(),
            ..Live::default()
        };
        assert_eq!(l.is_end(), end, "{phase}");
    }
}

#[test]
fn settled_and_guessing() {
    let cases = [
        ("", false, "", ""),
        ("hello", false, "", "hello"),
        ("hello there", false, "", "hello there"),
        ("hello there you", false, "hello", "there you"),
        ("  one two   three four ", false, "one two", "three four"),
        ("hello there you", true, "hello there you", ""),
        ("  spaced   out. ", true, "spaced out.", ""),
    ];
    for (text, last, settled, guessing) in cases {
        assert_eq!(
            split(text, last),
            (settled.to_string(), guessing.to_string()),
            "{text:?} {last}"
        );
    }
}

#[test]
fn loudness() {
    let loud = vec![0.5f32; 256];
    let quiet = vec![0.0001f32; 256];
    // -35 dB, halfway between silent (-55) and loud (-15).
    let half = vec![10f32.powf(-35.0 / 20.0); 256];
    for (samples, want) in [
        (&[][..], 0),
        (&quiet[..], 0),
        (&loud[..], 100),
        (&half[..], 50),
    ] {
        assert_eq!(level(samples), want);
    }
}

#[test]
fn the_bundles_program() {
    let cases = [
        ("/x/voice/rush voice.app/Contents/MacOS/rush-voice", true),
        ("/x/voice/rush voice.app/Contents/MacOS/rush", false),
        ("/usr/local/bin/rush-voice", false),
        ("/x/rush.app/Contents/MacOS/rush-menubar", false),
    ];
    for (p, want) in cases {
        assert_eq!(is_bundle_program(Path::new(p)), want, "{p}");
    }
    assert!(!rush_voice::bundled(), "the test binary isn't the helper");
}

#[test]
fn the_plist() {
    let p = bundle::plist(7);
    for want in [
        "<key>CFBundleIdentifier</key><string>dev.rush.voice</string>",
        "<key>CFBundleExecutable</key><string>rush-voice</string>",
        "<key>CFBundleVersion</key><string>7</string>",
        "<key>LSUIElement</key><true/>",
        "<key>LSMinimumSystemVersion</key><string>26.0</string>",
        "<key>NSMicrophoneUsageDescription</key><string>rush listens while you hold its push-to-talk key, and only then.</string>",
        "<key>NSSpeechRecognitionUsageDescription</key><string>rush turns what you say into text on this Mac with Apple's on-device speech model; nothing leaves your Mac.</string>",
    ] {
        assert!(p.contains(want), "{want}");
    }
}

#[test]
fn where_things_are() {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    assert_eq!(rush_voice::socket(), d.path().join("voice/live.sock"));
    assert_eq!(
        rush_voice::app_path(),
        d.path().join("voice/rush voice.app")
    );
    assert!(is_bundle_program(&bundle::program()));
    rush_state::set_thread_dir(None);
}

#[test]
fn versions() {
    for (v, want) in [("26.1\n", 26), ("15.6.1", 15), ("", 0), ("x", 0)] {
        assert_eq!(bundle::major(v), want, "{v:?}");
    }
}

#[test]
fn stamp_follows_the_rush_its_copied_from() {
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("rush");
    std::fs::write(&bin, b"one").unwrap();
    let a = bundle::stamp(&bin);
    assert_eq!(a, bundle::stamp(&bin));
    std::fs::write(&bin, b"rush two").unwrap();
    assert_ne!(a, bundle::stamp(&bin), "a new rush is a new helper");
}

/// Listing the Mac's microphones asks for no permission.
#[test]
fn devices_input_first() {
    let ds = rush_voice::devices();
    if !cfg!(target_os = "macos") {
        assert!(ds.is_empty());
        return;
    }
    assert!(ds.iter().all(|d| !d.id.is_empty()));
    assert!(ds.iter().filter(|d| d.default).count() <= 1);
    if ds.iter().any(|d| d.default) {
        assert!(ds[0].default, "the Mac's input comes first");
    }
    if std::env::var_os("RUSH_VOICE_SHOW").is_some() {
        eprintln!("available: {}", rush_voice::available());
        for d in &ds {
            eprintln!("{d:?}");
        }
    }
}

/// Builds a real helper bundle (signing it, registering it): only when
/// asked, as the menu bar app's test does.
#[test]
fn build_app_when_asked() {
    if std::env::var_os("RUSH_BUILD_APP").is_none() || !cfg!(target_os = "macos") {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    let app = rush_voice::app_path();
    assert_eq!(rush_voice::build_app(), Ok(true));
    let run = |prog: &str, args: &[&std::ffi::OsStr]| {
        std::process::Command::new(prog)
            .args(args)
            .status()
            .unwrap()
            .success()
    };
    assert!(run(
        "/usr/bin/codesign",
        &["--verify".as_ref(), "--verbose".as_ref(), app.as_os_str()]
    ));
    assert!(run(
        "/usr/bin/plutil",
        &[
            "-lint".as_ref(),
            app.join("Contents/Info.plist").as_os_str()
        ]
    ));
    assert_eq!(
        rush_voice::build_app(),
        Ok(false),
        "nothing changed, nothing built"
    );
    bundle::unregister(&app);
    rush_state::set_thread_dir(None);
}
