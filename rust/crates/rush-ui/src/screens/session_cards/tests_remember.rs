//! #remember, alt+k and the memory view's + r f (Go's remember.go and
//! memoryWideKey), with a fake keeper and drafter: no model is asked and
//! nothing is written outside a temp folder.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rush_core::adapter::{Memory, Profile, Registry};
use rush_core::agent::Agent;
use rush_core::event::{Event, Message, Part, PartKind};
use rush_memory::{Keeper, MemoryNote, Opens};

use super::remember::Remembering;
use super::tests::*;
use crate::model::Model;
use crate::screens::projects_machine::memreview::set_keeper;

/// What the drafter was handed: (what, around) each time.
type Asked = Arc<Mutex<Vec<(String, String)>>>;

/// A keeper that keeps what it's asked in memory.
#[derive(Default)]
struct FakeKeeper {
    kept: Mutex<Vec<(String, MemoryNote)>>,
    tidied: Mutex<Vec<String>>,
}

impl Keeper for FakeKeeper {
    fn remember(&self, _: &Profile, cwd: &str, n: &MemoryNote) -> rush_memory::Result<String> {
        self.kept.lock().unwrap().push((cwd.to_string(), n.clone()));
        Ok(format!("{cwd}/memory/{}.md", n.name))
    }
    fn tidy_memory(&self, dir: &str) -> rush_memory::Result<Vec<String>> {
        self.tidied.lock().unwrap().push(dir.to_string());
        Ok(vec![])
    }
    fn merge_notes(&self, _: &str, _: &str) -> rush_memory::Result<()> {
        Ok(())
    }
    fn make_global(&self, _: &Profile, _: &str) -> rush_memory::Result<()> {
        Ok(())
    }
    fn read_note(&self, _: &str) -> rush_memory::Result<MemoryNote> {
        Err("none".into())
    }
    fn count_opens(&self, _: &Profile, _: &Memory) -> HashMap<String, Opens> {
        HashMap::new()
    }
}

/// A hosted model whose agent is Claude Code in a temp config, with a
/// fake keeper, and the drafts the drafter was handed.
fn model(tmp: &std::path::Path) -> (Model, Arc<FakeKeeper>, Asked) {
    let mut m = hosted(120, 40);
    let reg = Registry::new();
    reg.register(rush_claude::Claude::at(rush_claude::Account::at(
        "t",
        tmp.join("cfg"),
    )));
    m.reg = reg;
    let mut a = Agent {
        key: "k".into(),
        kind: "claude".into(),
        ..Agent::default()
    };
    a.job.name = "fixer".into();
    a.job.cwd = tmp.join("work").to_string_lossy().into_owned();
    a.acct = Profile {
        kind: "claude".into(),
        name: "t".into(),
        dir: tmp.join("cfg").to_string_lossy().into_owned(),
    };
    m.agents = vec![a];
    m.sel = "k".into();
    let k = Arc::new(FakeKeeper::default());
    set_keeper(&mut m, "claude", k.clone());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    m.kept::<Remembering>().drafter = Some(Arc::new(move |what: &str, around: &str| {
        seen.lock()
            .unwrap()
            .push((what.to_string(), around.to_string()));
        Ok(MemoryNote {
            name: "tabs".into(),
            title: String::new(),
            description: "indents with tabs".into(),
            kind: "user".into(),
            body: "Indent with tabs.".into(),
        })
    }));
    (m, k, asked)
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

/// #remember drafts, asks, and y keeps it; e keeps it and opens the
/// memory view on it; with nothing to remember it says how.
#[test]
fn remember_asks_then_keeps() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut m, k, asked) = model(tmp.path());
    let cwd = tmp.path().join("work").to_string_lossy().into_owned();
    super::remember::remember_command(&mut m, "");
    assert_eq!(
        flash(&m),
        "#remember <what to remember>, or ⌥k on a message"
    );
    super::remember::remember_command(&mut m, "I like tabs");
    assert_eq!(asked.lock().unwrap()[0], ("I like tabs".into(), "".into()));
    let c = m.confirm.as_ref().expect("asked");
    assert_eq!(
        c.question, "Remember this? tabs",
        "title falls back to name"
    );
    assert_eq!(c.detail, "user · indents with tabs\n\nIndent with tabs.");
    assert_eq!(
        c.keys().plain(),
        "y remember   e remember and edit   n cancel"
    );
    press(&mut m, "y");
    assert_eq!(k.kept.lock().unwrap()[0].0, cwd);
    assert_eq!(flash(&m), "remembered: tabs · in the memory view");
    // e: kept, and picked in the memory view.
    super::remember::remember_command(&mut m, "I like tabs");
    press(&mut m, "e");
    assert_eq!(k.kept.lock().unwrap().len(), 2);
    assert_eq!(
        m.session.as_ref().unwrap().sel,
        format!("mem:{cwd}/memory/tabs.md")
    );
}

/// A draft that lands while you're typing waits; #remember alone shows it.
#[test]
fn a_draft_waits_while_you_type() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut m, _, _) = model(tmp.path());
    crate::screens::session_core::conn(&mut m, "k")
        .input
        .set("half a message");
    super::remember::remember_command(&mut m, "I like tabs");
    assert!(m.confirm.is_none());
    assert_eq!(flash(&m), "the note is ready · #remember alone shows it");
    crate::screens::session_core::conn(&mut m, "k")
        .input
        .clear();
    super::remember::remember_command(&mut m, "");
    assert!(m.confirm.is_some(), "shown");
}

/// An agent rush can't write memory for says so; another agent picked in
/// the list than the one open is told to open it.
#[test]
fn remember_needs_a_keeper_and_the_open_session() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut m, _, _) = model(tmp.path());
    let mut b = Agent {
        key: "b".into(),
        ..Agent::default()
    };
    b.job.name = "other".into();
    m.agents.push(b);
    m.sel = "b".into();
    super::remember::remember_command(&mut m, "x");
    assert_eq!(
        flash(&m),
        "open other's session to remember something for its project"
    );
    m.sel = "k".into();
    m.agents[0].kind = "nokeeper".into();
    super::remember::remember_command(&mut m, "x");
    assert!(
        flash(&m).ends_with(" keeps no memory rush can write"),
        "{}",
        flash(&m)
    );
}

/// alt+k on a picked turn remembers its answer; with the box typed in,
/// it's the box's.
#[test]
fn alt_k_remembers_the_picked_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut m, _, asked) = model(tmp.path());
    m.session
        .as_mut()
        .unwrap()
        .sess
        .sent("which indent?".into(), vec![], 1_000);
    apply(
        &mut m,
        Event::Message(Message {
            role: "assistant".into(),
            id: "a".into(),
            parts: vec![Part {
                kind: PartKind::Text,
                text: "Tabs, always.".into(),
                ..Part::default()
            }],
            ..Message::default()
        }),
    );
    apply(&mut m, Event::TurnEnd(Default::default()));
    m.session.as_mut().unwrap().sel = "t1".into();
    press(&mut m, "alt+k");
    assert_eq!(
        asked.lock().unwrap().first().map(|a| a.1.as_str()),
        Some("Tabs, always.")
    );
    assert!(m.confirm.is_some());
}

/// The memory view's + starts #remember in the box; f tidies the index.
#[test]
fn memory_view_plus_and_tidy() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut m, k, _) = model(tmp.path());
    let cfg = tmp.path().join("cfg");
    let cwd = tmp.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    // A note, so Claude Code's memory has a folder.
    let p = m.agents[0].acct.clone();
    rush_claude::keeper::NoteKeeper
        .remember(
            &p,
            &cwd.to_string_lossy(),
            &MemoryNote {
                name: "x".into(),
                title: "X".into(),
                description: "x".into(),
                kind: "user".into(),
                body: "x".into(),
            },
        )
        .unwrap();
    assert!(cfg.exists());
    assert!(crate::screens::session_core::views::show_view(
        &mut m, "k", "memory"
    ));
    let mem = crate::screens::session_core::memory::memory_of(&mut m, "k").expect("read");
    let note = mem
        .files
        .iter()
        .find(|f| f.name == "x")
        .unwrap_or_else(|| panic!("{mem:?}"))
        .path
        .clone();
    m.session.as_mut().unwrap().sel = format!("mem:{note}");
    press(&mut m, "f");
    assert_eq!(
        k.tidied.lock().unwrap().as_slice(),
        std::slice::from_ref(&mem.dir)
    );
    assert_eq!(
        flash(&m),
        "nothing in the index to tidy · the rest needs you: r reviews it"
    );
    m.session.as_mut().unwrap().sel = format!("mem:{note}");
    press(&mut m, "+");
    assert_eq!(
        crate::screens::session_core::conn(&mut m, "k")
            .input
            .string(),
        "#remember "
    );
    assert!(m.session.as_ref().unwrap().sel.is_empty());
}
