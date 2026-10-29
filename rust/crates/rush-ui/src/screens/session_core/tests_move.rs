//! Moving agents: to rush mode (/rush, #rush, a message to a past one),
//! to another folder (#cd, ctrl+l's picker), and where new sessions
//! start (ctrl+l). After Go's internal/ui/rushmode.go moveToRushWith,
//! movePending and model.go's movedToRushMsg, and picker.go (moveTo,
//! moveNote, diskDirs, the folder pickers), against a Fake host.

use std::sync::Arc;

use rush_core::adapter::{Adapter, Feature, Features, Kind, Level, Profile, Registry, Support};
use rush_core::agent::{Agent, Job, State};
use rush_core::text::Line;

use super::host::{Fake, set_hosts};
use super::moveto::{disk_dirs, move_note};
use super::{core, rushmove};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::list_extras::prompt::start_dir;

/// An agent that picks its conversations up again.
struct Movable;

impl Adapter for Movable {
    fn kind(&self) -> Kind {
        Kind::from("movable")
    }
    fn name(&self) -> String {
        "Movable Code".into()
    }
    fn features(&self) -> Features {
        [(Feature::Resume, Support::YES)].into_iter().collect()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
}

/// One that doesn't.
struct Stuck;

impl Adapter for Stuck {
    fn kind(&self) -> Kind {
        Kind::from("stuck")
    }
    fn name(&self) -> String {
        "Stuck Code".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
}

fn agent(key: &str, state: State) -> Agent {
    Agent {
        kind: Kind::from("movable"),
        key: format!("work/{key}"),
        display_name: format!("tidy {key}"),
        pid: 812,
        acct: Profile {
            kind: Kind::from("movable"),
            name: "work".into(),
            dir: "/x/.movable".into(),
        },
        job: Job {
            id: key.into(),
            session_id: format!("s-{key}"),
            state,
            cwd: "/src/app".into(),
            updated_at: 1_000,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn model(agents: Vec<Agent>) -> (Model, Arc<Fake>) {
    let mut m = crate::screens::social::feed::tests::model(120, 30);
    let reg = Registry::new();
    reg.register(Movable);
    reg.register(Stuck);
    m.reg = reg;
    let n = agents.len();
    let sel = agents.first().map(|a| a.key.clone()).unwrap_or_default();
    m.set_fleet(crate::worker::Fleet {
        agents,
        roots: vec![String::new(); n],
        ..Default::default()
    });
    m.sel = sel;
    let h = Fake::new();
    set_hosts(&mut m, Arc::new(h.clone()));
    (m, h)
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

fn said(m: &Model) -> String {
    m.said.last().map(|s| s.text.clone()).unwrap_or_default()
}

fn adopted(h: &Fake) -> Vec<(String, String)> {
    h.adopted.lock().unwrap().clone()
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s:?}")))
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

// ---- moving to rush mode ----

/// Mid-turn it waits for the turn to end; /rush again moves it now. Once
/// moved, the old row is put away, the new one picked as it's listed.
#[test]
fn a_busy_one_moves_when_its_turn_ends() {
    let (mut m, h) = model(vec![agent("a1", State::Working)]);
    m.command("#rush");
    assert_eq!(
        flash(&m),
        "tidy a1 moves to rush mode when this turn ends · /rush again moves it now"
    );
    assert!(adopted(&h).is_empty());
    rushmove::move_pending(&mut m);
    assert!(adopted(&h).is_empty(), "still working");
    m.agents[0].job.state = State::Idle;
    rushmove::move_pending(&mut m);
    assert_eq!(adopted(&h), [("work/a1".to_string(), String::new())]);
    assert!(m.agents[0].done, "the old row is put away");
    assert_eq!(flash(&m), "started tidy a1");
    assert_eq!(core(&mut m).select_on_load, "work/a:adopted-1");
    rushmove::move_pending(&mut m);
    assert_eq!(adopted(&h).len(), 1, "it waits no more");
}

/// /rush a second time moves a busy one at once.
#[test]
fn rush_again_moves_now() {
    let (mut m, h) = model(vec![agent("a1", State::Working)]);
    m.command("#rush");
    m.command("#rush");
    assert_eq!(adopted(&h).len(), 1);
}

/// Messages still waiting for the old row go to the new session.
#[test]
fn its_queue_goes_over() {
    let (mut m, h) = model(vec![agent("a1", State::Idle)]);
    core(&mut m)
        .local_q
        .entry("work/a1".into())
        .or_default()
        .items = vec!["first".into(), "second".into()];
    m.command("#rush");
    let sent = h.sent_to.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "adopted-1");
    assert!(sent[0].1.contains("first") && sent[0].1.contains("second"));
    assert!(!sent[0].2, "queued, not mid-turn");
    assert!(!core(&mut m).local_q.contains_key("work/a1"));
}

/// One open in a terminal is copied, and says so; the copy carries on.
#[test]
fn a_terminal_one_is_copied() {
    let mut a = agent("a1", State::Working);
    a.interactive = true;
    let (mut m, h) = model(vec![a]);
    m.command("#rush");
    assert_eq!(adopted(&h).len(), 1, "not waited on: its terminal keeps it");
    assert_eq!(
        flash(&m),
        "tidy a1 carries on in rush mode · its terminal copy is still open there, now under Done"
    );
}

/// What can't move says why; nothing moves.
#[test]
fn what_cant_move_says_why() {
    let mut a = agent("a1", State::Idle);
    a.rush = true;
    let (mut m, h) = model(vec![a]);
    m.command("#rush");
    assert_eq!(flash(&m), "tidy a1 already runs in rush mode");

    let mut a = agent("a2", State::Idle);
    a.kind = Kind::from("stuck");
    let (mut m, _) = model(vec![a]);
    m.command("#rush");
    assert_eq!(flash(&m), "Stuck Code can't pick a conversation up again");

    let mut a = agent("a3", State::Idle);
    a.job.session_id.clear();
    let (mut m, _) = model(vec![a]);
    m.command("#rush");
    assert_eq!(said(&m), "can't find tidy a3's conversation to resume");

    let mut a = agent("a4", State::Idle);
    a.headless = true;
    let (mut m, _) = model(vec![a]);
    m.command("#rush");
    assert_eq!(
        flash(&m),
        "tidy a4 is driven by another program; rush can't take it over"
    );
    assert!(adopted(&h).is_empty());
}

/// A host that couldn't start says why, and the row stays.
#[test]
fn a_failed_move_says_why() {
    let (mut m, h) = model(vec![agent("a1", State::Idle)]);
    *h.fail.lock().unwrap() = Some("no room".into());
    m.command("#rush");
    assert_eq!(said(&m), "no room");
    assert!(!m.agents[0].done);
}

// ---- moving to another folder ----

/// Go's moveNote, word for word; a path with a space is quoted.
#[test]
fn the_note_is_gos() {
    assert_eq!(
        move_note("/src/new", "/src/old"),
        "Please move over to /src/new and work there from now on (you were in /src/old). \
         Start by running `cd /src/new`. Your shell may go back to the old folder between \
         commands, so begin each Bash command with `cd /src/new && ` and use absolute paths \
         under /src/new. Paths from earlier in this conversation point at the old folder."
    );
    assert!(move_note("/src/it's here", "/a").contains("`cd '/src/it'\\''s here'`"));
}

/// #cd with a path tells a rush session mid-turn, then has its host list
/// it there and name it afresh; one outside rush is told as a message.
#[test]
fn cd_tells_it() {
    let mut a = agent("a1", State::Idle);
    a.rush = true;
    let (mut m, h) = model(vec![a]);
    h.host("work/a1");
    m.command("#cd /src/new");
    let sent = h.sent_to.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "a1");
    assert_eq!(sent[0].1, move_note("/src/new", "/src/app"));
    assert!(sent[0].2, "mid-turn, not queued");
    assert_eq!(
        *h.cds.lock().unwrap(),
        [("a1".to_string(), "/src/new".to_string(), true)]
    );
    assert_eq!(flash(&m), "sent to tidy a1");

    let (mut m, h) = model(vec![agent("a2", State::Idle)]);
    m.command("#cd /src/new");
    let r = h.replies.lock().unwrap().clone();
    assert_eq!(
        r,
        [("work/a2".to_string(), move_note("/src/new", "/src/app"))]
    );

    let mut a = agent("a3", State::Idle);
    a.interactive = true;
    let (mut m, _) = model(vec![a]);
    m.command("#cd /src/new");
    assert_eq!(
        flash(&m),
        "tidy a3 is open in another terminal · tell it there"
    );
}

/// #cd alone opens the folders to move it to; enter moves it to the one
/// picked.
#[test]
fn cd_alone_picks_a_folder() {
    let mut b = agent("a2", State::Idle);
    b.job.cwd = "/src/other".into();
    b.repo = "/src/other".into();
    let (mut m, h) = model(vec![agent("a1", State::Idle), b]);
    m.command("#cd");
    let f = screen(&mut m);
    assert!(
        f.iter()
            .any(|r| r.contains("Move tidy a1 to · now in /src/app")),
        "{f:#?}"
    );
    assert!(f.iter().any(|r| r.contains("Folder ❯")), "{f:#?}");
    assert!(
        f.iter().any(|r| r.contains("tell it to work here")),
        "{f:#?}"
    );
    for c in "other".chars() {
        press(&mut m, &c.to_string());
    }
    press(&mut m, "enter");
    let r = h.replies.lock().unwrap().clone();
    assert_eq!(
        r,
        [("work/a1".to_string(), move_note("/src/other", "/src/app"))]
    );
}

/// ctrl+l in the list picks where new sessions start; it holds while the
/// agent selected stays selected. With a # command typed it moves the
/// agent instead.
#[test]
fn ctrl_l_picks_where_new_sessions_start() {
    let mut b = agent("a2", State::Idle);
    b.job.cwd = "/src/other".into();
    let (mut m, _) = model(vec![agent("a1", State::Idle), b]);
    assert_eq!(press(&mut m, "ctrl+l"), Out::Draw);
    let f = screen(&mut m);
    assert!(
        f.iter().any(|r| r.contains("Start new sessions in")),
        "{f:#?}"
    );
    assert!(
        f.iter().any(|r| r.contains("start new sessions here")),
        "{f:#?}"
    );
    for c in "/tmp/brand-new".chars() {
        press(&mut m, &c.to_string());
    }
    press(&mut m, "enter");
    assert_eq!(flash(&m), "new sessions start in /tmp/brand-new");
    assert_eq!(start_dir(&m), "/tmp/brand-new");
    // ctrl+l closes it, as esc does.
    press(&mut m, "ctrl+l");
    assert!(m.sheet.is_some());
    press(&mut m, "ctrl+l");
    assert!(m.sheet.is_none());

    m.input.set("#cd");
    press(&mut m, "ctrl+l");
    let f = screen(&mut m);
    assert!(f.iter().any(|r| r.contains("Move tidy a1 to")), "{f:#?}");
}

/// What's on disk for a typed path: the folders in its folder that start
/// as its last part does, dotted ones left out; else folders beside the
/// projects whose names hold the text (diskDirs).
#[test]
fn disk_dirs_reads_as_gos() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_string_lossy().into_owned();
    for f in ["rush", "rust-port", "Photon", ".hidden", "ruby"] {
        std::fs::create_dir(d.path().join(f)).unwrap();
    }
    std::fs::write(d.path().join("rush.txt"), "").unwrap();
    let j = |n: &str| format!("{root}/{n}");
    assert_eq!(
        disk_dirs(&format!("{root}/rus"), &[]),
        [j("rush"), j("rust-port")]
    );
    assert_eq!(disk_dirs(&format!("{root}/"), &[]).len(), 4);
    assert_eq!(disk_dirs("pho", std::slice::from_ref(&root)), [j("Photon")]);
    assert_eq!(disk_dirs("pho", &[root.clone(), root.clone()]).len(), 1);
}
