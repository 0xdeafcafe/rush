//! What each harness is told at its start: the same rush guidance by
//! whichever channel it reads, Claude Code's --append-system-prompt,
//! Codex's developerInstructions, atop the first message for an ACP agent
//! with no system prompt. Rust-only.

use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    self, Conn, Driver, Feature, Kind, Profile, Registry, Sink, StartOptions,
};

use super::tests::{Kept, with_conn};
use super::*;
use crate::fake::Fake;
use crate::testing::Home;

struct Harness {
    fake: Fake,
    claude: bool,
    seen: Arc<Mutex<Vec<StartOptions>>>,
}

impl adapter::Adapter for Harness {
    fn kind(&self) -> Kind {
        self.fake.kind()
    }
    fn name(&self) -> String {
        self.fake.name()
    }
    fn features(&self) -> adapter::Features {
        self.fake.features()
    }
    fn level(&self) -> adapter::Level {
        self.fake.level()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn claude_transcripts(&self) -> bool {
        self.claude
    }
    rush_core::has!(driver: Driver);
}

impl Driver for Harness {
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        self.seen.lock().unwrap().push(o.clone());
        Driver::start(&self.fake, o, sink)
    }
}

/// Starts a session of kind in a registry holding Claude Code, Codex and
/// Kimi as their adapters say what they take, and gives what its agent
/// was started with and the host.
fn started(id: &str, kind: &'static str, headless: bool) -> (StartOptions, Host) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = Registry::new();
    for (k, claude, prompt) in [
        ("claude", true, true),
        ("codex", false, true),
        ("kimi", false, false),
    ] {
        let mut features = vec![Feature::Run, Feature::Resume, Feature::Mcp];
        if prompt {
            features.push(Feature::Prompt);
        }
        reg.register(Harness {
            fake: Fake { kind: k, features },
            claude,
            seen: if k == kind {
                seen.clone()
            } else {
                Arc::default()
            },
        });
    }
    std::fs::create_dir_all(crate::paths::dir(id)).unwrap();
    let (h, _rx) = build(
        reg,
        Hooks::quiet(),
        Config {
            id: id.into(),
            kind: kind.into(),
            headless,
            ..Config::default()
        },
    );
    let (g, r) = h.start(h.lock());
    drop(g);
    r.unwrap();
    let o = seen.lock().unwrap().last().cloned().expect("started");
    (o, h)
}

/// Every harness gets rush's guidance: the task list, scratch folder,
/// decisions cards, shell boxes and views. One whose own program isn't
/// Claude Code is told how rush draws replies too, as Claude Code's own
/// prompt tells it.
#[test]
fn every_harness_is_told_rushs_guidance() {
    let _home = Home::new();
    for (kind, formatting) in [("claude", false), ("codex", true), ("kimi", true)] {
        let (o, _h) = started(&format!("g{kind}"), kind, false);
        for (what, part) in [
            ("task list", TASKS_PROMPT),
            ("scratch", SCRATCH_PROMPT),
            ("decisions", crate::decisions::DECISIONS_PROMPT),
            ("shell boxes", SHELL_PROMPT),
            ("views", crate::views::VIEWS_PROMPT),
        ] {
            assert!(o.prompt.contains(part), "{kind}: no {what}");
        }
        assert_eq!(
            o.prompt.contains(FORMAT_PROMPT),
            formatting,
            "{kind}: formatting"
        );
    }
}

/// A harness with a system prompt takes it there; one without (Kimi over
/// ACP) has it atop its first message, once, which the conversation
/// doesn't show as yours.
#[test]
fn a_harness_without_a_system_prompt_has_it_atop_the_first_message() {
    let _home = Home::new();
    for (kind, atop) in [("codex", false), ("kimi", true)] {
        let id = format!("f{kind}");
        let (o, h) = started(&id, kind, false);
        let c = Arc::new(Kept::default());
        with_conn(&h, c.clone());
        h.lock().info.state = "idle".into();
        h.send("first", &[], false).unwrap();
        h.lock().info.state = "idle".into();
        h.send("second", &[], false).unwrap();
        let got = c.got.lock().unwrap();
        let want = if atop {
            format!(
                "<system-reminder>\n{}\n</system-reminder>\n\nfirst",
                o.prompt
            )
        } else {
            "first".to_string()
        };
        assert_eq!(got[0].text, want, "{kind}");
        assert_eq!(got[1].text, "second", "{kind}");
        assert_eq!(
            rush_convo::transcript::clean_prompt(&got[0].text),
            "first",
            "{kind}"
        );
    }
}

/// A headless run is read by the program that asked for it: nothing of
/// what rush draws, formatting included.
#[test]
fn a_headless_run_is_told_nothing_rush_draws() {
    let _home = Home::new();
    for kind in ["claude", "codex", "kimi"] {
        let (o, _h) = started(&format!("h{kind}"), kind, true);
        assert!(o.prompt.contains(SCRATCH_PROMPT), "{kind}");
        for part in [SHELL_PROMPT, FORMAT_PROMPT, TASKS_PROMPT] {
            assert!(!o.prompt.contains(part), "{kind}");
        }
    }
}
