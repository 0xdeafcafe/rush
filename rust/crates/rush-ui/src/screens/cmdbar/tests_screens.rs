//! Claude Code's screens typed in a session's box, and attach's first
//! steps: rush's own take on a screen, else Claude Code's; the commands
//! rush leaves out and the ones the agent can't do; claude: ones asked
//! about first; one neither knows asked about, opened or sent as typed;
//! and attach marking the agent seen and letting the preview's live
//! mirror go. After Go's sessionviews.go:847-866, :1001-1078,
//! infosheet.go:572-668 and model.go:898, :2663-2687. A test has no
//! terminal: what's handed it runs at Model::run_outside.

use std::process::Command as Program;
use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    self, Adapter, Command, Commander, Feature, Features, Kind, Level, Profile, Registry, Screener,
    Support,
};
use rush_core::agent::{Agent, State};
use rush_core::text::Line;

use crate::frame::sheet::Sheet;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::session_cards::claudescreen::{
    ClaudeSheet, UnknownSheet, claude_cloud_name, off_command,
};
use crate::screens::session_core::host::{Fake, Op, set_hosts};
use crate::screens::session_core::{conn, keys, live};
use crate::session::{SessionView, Source};

const KEY: &str = "work/c1";

/// An agent with screens of its own, and the commands on disk it lists.
#[derive(Clone, Default)]
struct Claw {
    /// The screens opened, in order.
    screens: Arc<Mutex<Vec<String>>>,
    /// The commands it lists on disk.
    listed: Arc<Mutex<Vec<String>>>,
}

impl Adapter for Claw {
    fn kind(&self) -> Kind {
        Kind::from("claw")
    }
    fn name(&self) -> String {
        "Claw Code".into()
    }
    fn features(&self) -> Features {
        [
            Feature::Screen,
            Feature::Mcp,
            Feature::Hooks,
            Feature::Settings,
            Feature::Commands,
            Feature::Memory,
        ]
        .into_iter()
        .map(|f| (f, Support::YES))
        .collect()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(screener: Screener, commander: Commander);
}

impl Screener for Claw {
    fn screen(&self, p: &Profile, dir: &str, command: &str, _hint: &str) -> Program {
        self.screens
            .lock()
            .unwrap()
            .push(format!("/{command} in {dir} as {}", p.name));
        Program::new("/usr/bin/true")
    }
}

impl Commander for Claw {
    fn commands(&self, _: &Profile, _: &str) -> Vec<Command> {
        let names = self.listed.lock().unwrap().clone();
        names
            .into_iter()
            .map(|name| Command {
                name,
                ..Command::default()
            })
            .collect()
    }
}

fn agent() -> Agent {
    let mut a = Agent {
        key: KEY.into(),
        display_name: "tidy the parser".into(),
        kind: Kind::from("claw"),
        pid: 4242,
        rush: true,
        acct: Profile {
            kind: Kind::from("claw"),
            name: "work".into(),
            dir: "/x/.claw".into(),
        },
        ..Agent::default()
    };
    a.job.id = "c1".into();
    a.job.state = State::Idle;
    a.job.cwd = "/src/app".into();
    a
}

/// A model with a hosted Claw session open, its host saying the commands
/// it has.
fn open() -> (Model, Claw, Arc<Fake>) {
    let mut m = Model::new("0.1.0");
    m.w = 120;
    m.h = 40;
    let claw = Claw::default();
    let reg = Registry::new();
    reg.register(claw.clone());
    m.reg = reg;
    let a = agent();
    let src = Source::History {
        adapter: m.reg.get("claw").expect("registered"),
        session: Box::new(adapter::Session::default()),
    };
    let mut v = SessionView::new(a.key.clone(), src, 1);
    v.read = true;
    m.sel = a.key.clone();
    m.agents = vec![a];
    m.session = Some(v);
    let fake = Fake::new();
    fake.host(KEY);
    set_hosts(&mut m, Arc::new(fake.clone()));
    live::sync(&mut m);
    assert!(conn(&mut m, KEY).hosted());
    m.session.as_mut().unwrap().sess.commands = vec!["compact".into(), "review".into()];
    (m, claw, fake)
}

/// Types text in the box and sends it.
fn send(m: &mut Model, text: &str) -> Out {
    conn(m, KEY).input.set(text);
    keys::send(m, KEY, false)
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

fn sent(f: &Fake) -> Vec<String> {
    f.ops()
        .into_iter()
        .filter_map(|o| match o {
            Op::Send { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

fn text(l: &Line) -> String {
    l.spans.iter().map(|s| s.text.as_str()).collect()
}

fn body<S: Sheet>(m: &Model, s: &mut S, w: usize) -> Vec<String> {
    s.body(m, w, 40).iter().map(text).collect()
}

fn press(m: &mut Model, k: &str) -> Out {
    m.sheet_key(&Press::of(k).expect("a key"))
}

/// Both sheets read as Go's, word for word and space for space, at the
/// width they're drawn at on any screen of 80 columns or more and at a
/// narrow one (testdata/go_screen_sheets.txt: Go's unknownSheet and
/// claudeSheet bodies, ANSI stripped).
#[test]
fn the_sheets_read_as_gos() {
    let m = Model::new("0.1.0");
    let mut out = String::new();
    for w in [68, 50] {
        let mut u = UnknownSheet {
            conn: KEY.into(),
            line: "frobnicate now".into(),
            now: false,
            cur: 0,
        };
        let mut c = ClaudeSheet {
            conn: KEY.into(),
            line: "teleport abc".into(),
        };
        out += &format!("=== unknown {w}\n{}\n", body(&m, &mut u, w).join("\n"));
        out += &format!("=== claude {w}\n{}\n", body(&m, &mut c, w).join("\n"));
    }
    assert_eq!(out, include_str!("testdata/go_screen_sheets.txt"));
}

#[test]
fn names_for_the_cloud_and_the_commands_left_out() {
    for (name, want) in [
        ("login", Some("login")),
        ("claude:login", Some("login")),
        ("tp", Some("teleport")),
        ("claude:extra-usage", Some("usage-credits")),
        ("plugin", None),
    ] {
        assert_eq!(claude_cloud_name(name), want, "{name}");
    }
    for (name, want) in [
        ("theme", Some("it's for Claude Code's own terminal")),
        ("stickers", Some("rush leaves it out")),
        (
            "loops",
            Some("Claude Code has it switched off; ask Claude to /loop instead"),
        ),
        ("compact", None),
    ] {
        assert_eq!(off_command(name), want, "{name}");
    }
}

/// /mcp opens Claude Code's own; given arguments it goes to the agent.
#[test]
fn mcp_opens_claude_codes_screen() {
    let (mut m, claw, fake) = open();
    assert_eq!(send(&mut m, "/mcp"), Out::Draw);
    assert_eq!(*claw.screens.lock().unwrap(), ["/mcp in /src/app as work"]);
    assert_eq!(m.run_outside(), 1);
    assert!(conn(&mut m, KEY).input.is_empty(), "the box is cleared");
    send(&mut m, "/mcp add db");
    assert_eq!(sent(&fake), ["/mcp add db"]);
    assert_eq!(claw.screens.lock().unwrap().len(), 1);
}

/// /hooks, /permissions and /status are rush's own sheets; /memory is
/// the memory view.
#[test]
fn rush_draws_its_own() {
    let (mut m, claw, _) = open();
    send(&mut m, "/hooks");
    assert!(
        m.sheet_as::<crate::screens::settings::sheets::HookSheet>()
            .is_some()
    );
    m.sheet = None;
    send(&mut m, "/allowed-tools");
    assert!(
        m.sheet_as::<crate::screens::settings::sheets::PermSheet>()
            .is_some()
    );
    m.sheet = None;
    send(&mut m, "/version");
    assert!(
        m.sheet_as::<crate::screens::accounts::info::InfoSheet>()
            .is_some()
    );
    m.sheet = None;
    send(&mut m, "/memory");
    assert!(m.sheet.is_none());
    assert_eq!(
        crate::screens::session_core::views::view_name(&m, KEY),
        "memory"
    );
    assert!(claw.screens.lock().unwrap().is_empty());
}

/// What rush leaves out, and what the agent can't do, say so; nothing
/// goes.
#[test]
fn what_isnt_done_says_why() {
    let (mut m, claw, fake) = open();
    for (typed, said) in [
        (
            "/theme",
            "/theme isn't in rush: it's for Claude Code's own terminal",
        ),
        ("/plugins", "/plugins isn't something Claw Code can do"),
        ("/branch", "/fork isn't something Claw Code can do"),
        ("/stats", "/stats isn't something Claw Code can do"),
    ] {
        send(&mut m, typed);
        assert_eq!(flash(&m), said, "{typed}");
        assert!(conn(&mut m, KEY).input.is_empty(), "{typed}");
    }
    assert!(sent(&fake).is_empty());
    assert!(claw.screens.lock().unwrap().is_empty());
}

/// claude:login says first that it opens a real Claude Code; enter opens
/// it, its arguments with it.
#[test]
fn a_cloud_command_asks_first() {
    let (mut m, claw, _) = open();
    send(&mut m, "/tp abc");
    let mut s = ClaudeSheet {
        conn: KEY.into(),
        line: "teleport abc".into(),
    };
    assert_eq!(
        m.sheet_as::<ClaudeSheet>().map(|s| s.line.clone()),
        Some(s.line.clone())
    );
    let rows = body(&m, &mut s, 68);
    assert_eq!(
        rows[0],
        "claude:teleport  send a session to the cloud, or bring one back fro…"
    );
    assert_eq!(rows[2].trim_end(), "  ↗ This opens a real Claude Code");
    assert_eq!(
        rows[4],
        "  /teleport is Claude Code's own, for your account or Claude's"
    );
    assert!(rows.last().unwrap().contains("open Claude Code"));
    press(&mut m, "enter");
    assert!(m.sheet.is_none());
    assert_eq!(
        *claw.screens.lock().unwrap(),
        ["/teleport abc in /src/app as work"]
    );
}

/// A command neither rush nor the session knows is asked about: opened in
/// Claude Code, or sent as typed. One of the agent's own on disk goes at
/// once.
#[test]
fn an_unknown_command_is_asked_about() {
    let (mut m, claw, fake) = open();
    assert_eq!(send(&mut m, "/frobnicate now"), Out::Draw);
    let mut s = UnknownSheet {
        conn: KEY.into(),
        line: "frobnicate now".into(),
        now: false,
        cur: 0,
    };
    assert_eq!(
        m.sheet_as::<UnknownSheet>().map(|s| s.line.clone()),
        Some(s.line.clone())
    );
    assert_eq!(
        conn(&mut m, KEY).input.string(),
        "/frobnicate now",
        "kept until it's answered"
    );
    let rows = body(&m, &mut s, 68);
    assert_eq!(rows[0], "/frobnicate  isn't a command rush knows");
    assert!(
        rows[6].starts_with("▍ Open it in Claude Code"),
        "{:?}",
        rows[6]
    );
    assert_eq!(
        rows[7],
        "    in this agent's folder and account · esc, then ctrl+c twice, …"
    );
    assert_eq!(rows[8], "  Send it to Claude as a message");
    assert_eq!(rows[9], "    as you typed it");
    press(&mut m, "enter");
    assert_eq!(
        *claw.screens.lock().unwrap(),
        ["/frobnicate now in /src/app as work"]
    );
    assert!(conn(&mut m, KEY).input.is_empty());

    send(&mut m, "/frobnicate");
    press(&mut m, "down");
    press(&mut m, "enter");
    assert!(m.sheet.is_none());
    assert_eq!(sent(&fake), ["/frobnicate"]);

    // The session's own, and the agent's on disk, go as typed.
    send(&mut m, "/review");
    claw.listed.lock().unwrap().push("deploy".into());
    send(&mut m, "/deploy prod");
    assert!(m.sheet.is_none());
    assert_eq!(sent(&fake), ["/frobnicate", "/review", "/deploy prod"]);
}

/// Before the session has said which commands it has, nothing is asked.
#[test]
fn nothing_is_asked_before_the_session_says() {
    let (mut m, _, fake) = open();
    m.session.as_mut().unwrap().sess.commands.clear();
    send(&mut m, "/frobnicate");
    assert!(m.sheet.is_none());
    assert_eq!(sent(&fake), ["/frobnicate"]);
}

/// attach marks an agent waiting on you seen, and lets the preview's
/// live mirror go before the terminal is handed over; the terminal is
/// the session's until it's back.
#[test]
fn attach_marks_it_seen_and_closes_the_mirror() {
    use super::attach::{Attached, attach, attached};
    static CLOSED: Mutex<usize> = Mutex::new(0);
    let (mut m, _, _) = open();
    m.kept::<Attached>().close_live = Some(|_| *CLOSED.lock().unwrap() += 1);
    let mut a = agent();
    a.rush = false;
    a.job.state = State::Blocked;
    attach(&mut m, Some(a.clone()));
    let seen = rush_state::Store::load().overlay.seen;
    assert!(seen.contains_key(KEY), "kept as seen");
    assert_eq!(*CLOSED.lock().unwrap(), 1);
    // Claw has no screen to join: the terminal's let go at once.
    assert_eq!(attached(&m), None);
    assert_eq!(
        flash(&m),
        "couldn't open tidy the parser: its agent has no screen of its own to open"
    );

    // One open in another terminal keeps the mirror, as Go's does.
    a.interactive = true;
    attach(&mut m, Some(a));
    assert_eq!(*CLOSED.lock().unwrap(), 1);
    assert_eq!(attached(&m), Some(KEY));
}
