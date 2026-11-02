//! @ in a session's box: its agents under "agents", then its folder's
//! files under "files", ranked as a file finder ranks them; only files
//! once a / or . is typed; tab puts the file's path in the box; and what
//! each kind of harness is sent: @path to one that reads it, the plain
//! path to one that doesn't. The folder is a temp dir, read at once (no
//! loop: handed-off work runs at once).

use std::sync::Arc;

use rush_core::adapter::{Adapter, Features, Kind, Level, Profile};
use rush_core::agent::{Agent, State};
use rush_core::text::Line;
use rush_ui_session_cards::mention::{Pick, mention_picks, pick_rows};
use rush_ui_session_cards::mentionfiles::send_files;

use super::host::{Fake, Op};
use super::tests::{KEY, agent, open, press};
use super::{conn, live};
use crate::model::Model;

/// A harness that reads @path in a message as that file, as Claude Code
/// does.
struct Reads;

impl Adapter for Reads {
    fn kind(&self) -> Kind {
        Kind::from("reads")
    }
    fn name(&self) -> String {
        "Reads Code".into()
    }
    fn features(&self) -> Features {
        Features::new()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn reads_at_paths(&self) -> bool {
        true
    }
}

const FILES: [&str; 4] = [
    "src/render/draw.rs",
    "rush-draw/src/lib.rs",
    "my docs/plan a.md",
    "README.md",
];

/// A session of a kind harness, rush hosting it, working in a folder
/// with FILES, and another agent, "drafts", to tag.
fn session(kind: &str) -> (Model, Arc<Fake>, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    for f in FILES {
        let p = d.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    let fake = Fake::new();
    fake.host(KEY);
    let mut a = agent("plain", State::Working);
    a.job.cwd = d.path().to_string_lossy().into_owned();
    let mut m = open(a, &fake, 140, 40);
    m.reg.register(Reads);
    m.agents[0].kind = Kind::from(kind);
    m.agents.push(Agent {
        key: "default/k2".into(),
        display_name: "drafts".into(),
        ..Agent::default()
    });
    m.order = vec![0, 1];
    live::sync(&mut m);
    (m, fake, d)
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn picks(m: &mut Model, typed: &str) -> Vec<Pick> {
    let dir = m.agents[0].job.cwd.clone();
    mention_picks(m, &dir, &chars(typed), 0)
}

fn rows(m: &mut Model, typed: &str, sel: usize) -> Vec<String> {
    let p = picks(m, typed);
    pick_rows(m, &p, sel, 140)
        .iter()
        .map(|l| Line::plain(l).trim_end().to_string())
        .collect()
}

fn set_box(m: &mut Model, text: &str) {
    conn(m, KEY).input.set(text);
}

fn boxed(m: &mut Model) -> String {
    conn(m, KEY).input.string()
}

/// The agents first under their heading, then the files, a hit in a
/// file's name ahead of one in its path.
#[test]
fn agents_then_files() {
    let (mut m, _fake, _d) = session("plain");
    assert_eq!(
        rows(&mut m, "fix the hover in @dra", 0),
        [
            " agents",
            " ▸ drafts      @drafts",
            " files",
            "   src/render/draw.rs",
            "   rush-draw/src/lib.rs",
            "   ↑↓ · tab or enter tags · first, the message goes to them; later, the agent here is told how to reach them",
        ]
    );
    // A file picked says what taking it does.
    assert_eq!(
        rows(&mut m, "fix the hover in @dra", 1)[3..],
        [
            " ▸ src/render/draw.rs".to_string(),
            "   rush-draw/src/lib.rs".into(),
            "   ↑↓ · tab or enter puts the file's path in the message".into(),
        ]
    );
}

/// A / or a . in what's typed: files only, as no handle has one.
#[test]
fn files_only_after_a_slash_or_a_dot() {
    let (mut m, _fake, _d) = session("plain");
    for typed in ["@src/", "@dr."] {
        let p = picks(&mut m, typed);
        assert!(!p.is_empty(), "{typed}");
        assert!(
            p.iter().all(|p| matches!(p, Pick::File(_))),
            "{typed}: {p:?}"
        );
    }
    assert_eq!(
        rows(&mut m, "@src/", 0),
        [
            " files",
            " ▸ src/render/draw.rs",
            "   rush-draw/src/lib.rs",
            "   ↑↓ · tab or enter puts the file's path in the message",
        ]
    );
    // No folder, no files: the agents alone, as before, no heading.
    let p = mention_picks(&mut m, "", &chars("@dra"), 0);
    assert_eq!(p.len(), 1);
    assert!(
        !pick_rows(&m, &p, 0, 140)[0].plain().contains("agents"),
        "no heading with agents alone"
    );
}

/// tab on a file puts @path in the box, quoted when it has a space.
#[test]
fn taking_a_file() {
    let (mut m, _fake, _d) = session("plain");
    set_box(&mut m, "fix the hover in @dra");
    press(&mut m, "down");
    press(&mut m, "tab");
    assert_eq!(boxed(&mut m), "fix the hover in @src/render/draw.rs ");
    set_box(&mut m, "read @plan");
    press(&mut m, "tab");
    assert_eq!(boxed(&mut m), "read @\"my docs/plan a.md\" ");
    // An agent still completes to its handle.
    set_box(&mut m, "ask @dra");
    press(&mut m, "tab");
    assert_eq!(boxed(&mut m), "ask @drafts ");
}

fn sent(f: &Fake) -> String {
    f.ops()
        .into_iter()
        .filter_map(|o| match o {
            Op::Send { text, .. } => Some(text),
            _ => None,
        })
        .next_back()
        .unwrap_or_else(|| panic!("nothing sent: {:?}", f.ops()))
}

const TYPED: &str = "look at @src/render/draw.rs and @\"my docs/plan a.md\"";

/// A harness that reads @path is sent it as typed; one that doesn't, the
/// plain path, still quoted when it has a space.
#[test]
fn what_each_harness_is_sent() {
    let (mut m, fake, _d) = session("reads");
    set_box(&mut m, TYPED);
    press(&mut m, "enter");
    assert_eq!(sent(&fake), TYPED);

    let (mut m, fake, _d) = session("plain");
    set_box(&mut m, TYPED);
    press(&mut m, "enter");
    assert_eq!(
        sent(&fake),
        "look at src/render/draw.rs and \"my docs/plan a.md\""
    );
}

/// Only tags of files there lose their @: an agent's, an email and a
/// file that isn't there are left as they are.
#[test]
fn only_files_lose_their_at() {
    let (m, _fake, d) = session("plain");
    let dir = d.path().to_string_lossy().into_owned();
    assert_eq!(
        send_files(
            &m.reg,
            "plain",
            &dir,
            "@README.md, then ask @drafts; mail me@x.io about @gone.rs"
        ),
        "README.md, then ask @drafts; mail me@x.io about @gone.rs"
    );
    assert_eq!(
        send_files(&m.reg, "reads", &dir, "@README.md please"),
        "@README.md please"
    );
}
