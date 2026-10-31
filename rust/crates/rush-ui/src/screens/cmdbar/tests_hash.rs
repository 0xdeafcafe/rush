//! rush's # commands: every one the picker lists runs, under its old
//! names too; the Agents box's picker offers them as they're typed, tab
//! completes and enter runs; in a Session they act on its own agent
//! whatever the list has picked; #away, #loop, #back and #perm on a
//! session rush hosts. After Go's fleetslash_test.go, away_test.go and
//! permissions_test.go. No sleeps: a model with no loop runs handed-off
//! work at once.

use std::sync::Arc;

use rush_core::adapter::{self, Adapter, Features, Kind, Level, Profile, Registry};
use rush_core::agent::{Agent, State};
use rush_core::text::Line;
use rush_state::Store;

use super::fleetslash::{FLEET_ALIASES, FLEET_COMMANDS};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::accounts::startsheet::StartSheet;
use crate::screens::accounts::state::{cfg, set_store};
use crate::screens::list_extras::away::{Away, Held};
use crate::screens::session_core::host::{Fake, Op, set_hosts};
use crate::screens::session_core::{conn, live};
use crate::session::{SessionView, Source};

/// A harness with nothing of its own: enough for a Session to open.
struct Claw;

impl Adapter for Claw {
    fn kind(&self) -> Kind {
        Kind::from("claude")
    }
    fn name(&self) -> String {
        "Claw".into()
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

const NOW: i64 = 1_793_190_896_789;
const HOUR: i64 = 3_600_000;

fn agent(id: &str, name: &str) -> Agent {
    let mut a = Agent {
        key: format!("work/{id}"),
        display_name: name.into(),
        kind: Kind::from("claude"),
        rush: true,
        acct: Profile {
            kind: Kind::from("claude"),
            name: "work".into(),
            dir: "/x/.claude".into(),
        },
        ..Agent::default()
    };
    a.job.id = id.into();
    a.job.state = State::Idle;
    a.job.cwd = "/src/app".into();
    a.job.updated_at = NOW;
    a.pid = 4242;
    a
}

/// The Agents list with two agents, the first picked, nothing open.
fn list() -> Model {
    let mut m = Model::new("0.1.0");
    m.w = 120;
    m.h = 40;
    m.now = NOW;
    m.clock = Some(NOW);
    let reg = Registry::new();
    reg.register(Claw);
    m.reg = reg;
    set_store(&mut m, Store::default());
    let agents = vec![agent("c1", "tidy the parser"), agent("c2", "fix the build")];
    let roots = agents.iter().map(|a| a.repo.clone()).collect();
    m.set_fleet(crate::worker::Fleet {
        agents,
        roots,
        previews: Default::default(),
        priced: true,
    });
    m.sel = "work/c1".into();
    m
}

/// The list with c2's Session open, hosted by a fake host, while the
/// list has c1 picked.
fn open() -> (Model, Arc<Fake>) {
    let mut m = list();
    let src = Source::History {
        adapter: m.reg.get("claude").expect("registered"),
        session: Box::new(adapter::Session::default()),
    };
    let mut v = SessionView::new("work/c2".into(), src, 1);
    v.read = true;
    m.session = Some(v);
    let fake = Fake::new();
    fake.host("work/c2");
    set_hosts(&mut m, Arc::new(fake.clone()));
    m.sel = "work/c2".into();
    live::sync(&mut m);
    assert!(conn(&mut m, "work/c2").hosted());
    conn(&mut m, "work/c2").info.proto = 11;
    m.sel = "work/c1".into();
    (m, fake)
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s:?}")))
}

fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        let k = if c == ' ' {
            "space".to_string()
        } else {
            c.to_string()
        };
        press(m, &k);
    }
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

fn aways(f: &Fake) -> Vec<Option<Away>> {
    f.ops()
        .into_iter()
        .filter_map(|o| match o {
            Op::Away(a) => Some(a.map(|a| *a)),
            _ => None,
        })
        .collect()
}

/// Every command the picker lists has something that runs it, and so
/// does every name it answers to: none says it's unknown (S36, S54,
/// S20).
#[test]
fn every_listed_command_and_alias_runs() {
    let m = list();
    let names: Vec<&str> = m.ext.commands.iter().map(|c| c.name).collect();
    for (name, _, _) in FLEET_COMMANDS {
        assert!(names.contains(name), "#{name} has nothing to run it");
    }
    for (alias, to) in FLEET_ALIASES {
        assert!(
            names.contains(to),
            "#{alias} stands for #{to}, which nothing runs"
        );
        let mut m = list();
        if *to == "quit" {
            assert_eq!(m.command(&format!("#{alias}")), Out::Quit);
            continue;
        }
        m.command(&format!("#{alias}"));
        assert!(
            !flash(&m).starts_with("unknown command"),
            "#{alias}: {}",
            flash(&m)
        );
    }
}

/// Typing # in the Agents box lists rush's commands over it, #ti narrows
/// them to #tips, tab completes it and ↓ moves the pick round
/// (fleetSlashLines, fleetSlashKey; S17, S53).
#[test]
fn the_agents_box_offers_commands() {
    let mut m = list();
    typed(&mut m, "#");
    let f = screen(&mut m);
    // Rows found by what they say: cards above the box (Getting started)
    // move them.
    let boxed = row(&f, |l| l.starts_with("╭─ rush command · enter runs it"));
    let hint = boxed - 1;
    assert!(
        f[hint].contains("1 of ") && f[hint].contains("↑↓ · tab completes · enter runs"),
        "{:?}",
        f[hint]
    );
    let first = row(&f, |l| l.starts_with(" ▸ #expand "));
    assert!(first < hint, "the menu is over its hint");
    assert!(
        f[first + 1].starts_with("   #collapse "),
        "{:?}",
        f[first + 1]
    );
    assert!(
        f.iter().any(|l| l.starts_with("   #done ")),
        "an agent's picked"
    );
    press(&mut m, "down");
    let f = screen(&mut m);
    assert!(
        f[first + 1].starts_with(" ▸ #collapse "),
        "{:?}",
        f[first + 1]
    );
    press(&mut m, "up");
    press(&mut m, "up");
    let f = screen(&mut m);
    assert!(
        f.iter().any(|l| l.starts_with(" ▸ #quit ")),
        "↑ on the first goes round to the last"
    );
    m.input.clear();
    typed(&mut m, "#ti");
    let f = screen(&mut m);
    let boxed = row(&f, |l| l.starts_with("╭─ rush command"));
    let tips = row(&f, |l| l.starts_with(" ▸ #tips   [off] Getting started"));
    assert_eq!(tips + 1, boxed - 1, "#tips is the one pick, over its hint");
    assert_eq!(
        f[boxed - 1].trim_end(),
        "   ↑↓ · tab completes · enter runs"
    );
    press(&mut m, "tab");
    assert_eq!(m.input.string(), "#tips ");
}

/// The last row of frame f that want picks: rows are looked for by what
/// they say, never at a fixed distance from the bottom.
fn row(f: &[String], want: impl Fn(&str) -> bool) -> usize {
    f.iter()
        .rposition(|l| want(l))
        .unwrap_or_else(|| panic!("no such row in\n{}", f.join("\n")))
}

/// enter on a pick that needs nothing more runs it; one that needs a
/// value is completed with a space to type it after.
#[test]
fn enter_runs_the_pick() {
    let mut m = list();
    typed(&mut m, "#worktr");
    press(&mut m, "enter");
    assert!(m.input.is_empty());
    assert!(
        flash(&m).starts_with("new sessions in app get a worktree and branch of their own"),
        "{}",
        flash(&m)
    );
    typed(&mut m, "#rememb");
    press(&mut m, "enter");
    assert_eq!(m.input.string(), "#remember ");
}

/// #twotter, #history and #optimise run #feed, #stash and #slim; the
/// old names of #use say what it's called now (keys.go:884-889).
#[test]
fn aliases_run_their_command() {
    let mut m = list();
    m.command("#history");
    assert!(flash(&m).contains("isn't running · Settings, Plugins has it, as drafts"));
    let mut m = list();
    m.command("#optimise");
    assert!(!flash(&m).starts_with("unknown"), "{}", flash(&m));
    let mut m = list();
    m.command("#profile");
    assert!(
        m.sheet.is_some() || flash(&m) == "#profile is now called #use",
        "{}",
        flash(&m)
    );
}

/// In a Session its box's commands act on its own agent, though the
/// list has another picked (command's a; S38).
#[test]
fn a_sessions_commands_act_on_it() {
    let (mut m, _) = open();
    m.command("#remember");
    assert!(
        !flash(&m).contains("open tidy the parser's session"),
        "{}",
        flash(&m)
    );
    assert_eq!(m.sel, "work/c2");
}

/// #worktree on|off is kept for the project new sessions start in;
/// alone it says which; anything else says how.
#[test]
fn worktree_on_and_off() {
    let mut m = list();
    m.command("#worktree off");
    assert_eq!(
        flash(&m),
        "new sessions in app work in the checkout itself · #worktree on"
    );
    assert!(!cfg(&m).own_worktree("/src/app"));
    m.command("#worktree");
    assert_eq!(
        flash(&m),
        "new sessions in app work in the checkout itself · #worktree on"
    );
    m.command("#worktree on");
    assert!(cfg(&m).own_worktree("/src/app"));
    m.command("#worktree maybe");
    assert_eq!(flash(&m), "#worktree on or #worktree off");
}

/// #away 3h tells the host to keep it going for three hours, every 30
/// minutes; #loop every 15m keeps it going; #loop again ends it,
/// counting its check-ins (awayCommand).
#[test]
fn away_and_loop() {
    let (mut m, fake) = open();
    m.command("#away 3h");
    let a = aways(&fake).pop().flatten().expect("an away set");
    assert_eq!(a.until, NOW + 3 * HOUR);
    assert_eq!(a.every, HOUR / 2);
    assert!(!a.looping);
    assert!(
        flash(&m).ends_with(
            "told to keep going every 30m, what it asks saved for you · rush can close; leave the lid open"
        ),
        "{}",
        flash(&m)
    );
    m.command("#loop every 15m");
    let a = aways(&fake).pop().flatten().expect("a loop set");
    assert!(a.looping && a.until == 0 && a.every == HOUR / 4);
    assert_eq!(
        flash(&m),
        "looping · told to keep going every 15m it's idle, till it's all done"
    );
    conn(&mut m, "work/c2").info.away = Some(Box::new(Away {
        looping: true,
        nudges: 2,
        ..a
    }));
    m.command("#loop");
    assert_eq!(aways(&fake).pop(), Some(None));
    assert_eq!(flash(&m), "loop off · 2 check-ins sent");
    m.command("#back");
    assert_eq!(flash(&m), "looping, not away · #loop off ends it");
    m.command("#away soon");
    assert_eq!(flash(&m), "soon: a time like 3h or 90m");
}

/// #back on an away opens what happened; enter ends it, putting what it
/// saved in the box as one reply; l ends it and loops (backSheet).
#[test]
fn back_from_away() {
    let (mut m, fake) = open();
    conn(&mut m, "work/c2").info.away = Some(Box::new(Away {
        from: NOW - HOUR,
        until: NOW + HOUR,
        every: HOUR / 2,
        nudges: 1,
        held: vec![Held {
            kind: "question".into(),
            text: "which port?".into(),
            at: NOW - HOUR / 2,
        }],
        ..Away::default()
    }));
    m.command("#back");
    assert!(m.sheet.is_some(), "the back sheet opens");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("away 1h00m · 1 check-in"), "{f}");
    assert!(f.contains("which port?"), "{f}");
    press(&mut m, "l");
    assert!(m.sheet.is_none());
    let a = aways(&fake).pop().flatten().expect("a loop");
    assert!(a.looping && a.every == HOUR / 2);
    assert_eq!(
        flash(&m),
        "welcome back · 1 check-in sent · looping every 30m"
    );
    assert!(
        conn(&mut m, "work/c2")
            .input
            .string()
            .starts_with("On what you saved while I was away:\n- which port? → ")
    );
}

/// #away needs a session rush hosts, and one whose host keeps aways.
#[test]
fn away_needs_a_hosted_session() {
    let mut m = list();
    m.command("#away 3h");
    assert_eq!(flash(&m), "#away works in an open rush session");
    let (mut m, fake) = open();
    conn(&mut m, "work/c2").info.proto = 10;
    m.command("#loop");
    assert_eq!(
        flash(&m),
        "this session's host is older than this rush · /restart it for #loop"
    );
    assert!(aways(&fake).is_empty());
}

/// #perm with no agent picked opens the setup sheet on its Permissions
/// row, to apply only; with one picked whose Session isn't open it says
/// to open it; in its Session it opens on that session's (S34).
#[test]
fn perm_opens_the_permissions_row() {
    let mut m = list();
    m.set_fleet(crate::worker::Fleet {
        agents: Vec::new(),
        roots: Vec::new(),
        previews: Default::default(),
        priced: true,
    });
    m.command("#permissions");
    if let Some(s) = m.sheet_as::<StartSheet>() {
        assert_eq!(s.row, 6);
        assert!(s.apply_only);
    } else {
        assert_eq!(flash(&m), "rush can't run any harness here yet");
    }
    let mut m = list();
    m.command("#perm");
    assert_eq!(
        flash(&m),
        "Open the session first to change its permissions"
    );
    m.command("#yolo");
    assert_eq!(
        flash(&m),
        "Open the session first to change its permissions"
    );
    let (mut m, _) = open();
    m.command("#yolo now");
    assert_eq!(
        flash(&m),
        "#yolo takes no arguments; use #perm to choose a permission mode"
    );
}

/// #go on an agent at work says so; #quit leaves; #pr with none says
/// so; #why wants a session rush hosts open.
#[test]
fn go_quit_pr_why() {
    let mut m = list();
    let i = m.selected().expect("c1 picked");
    m.agents[i].job.state = State::Working;
    m.agents[i].pid = 42;
    m.command("#go");
    assert_eq!(flash(&m), "tidy the parser is still working");
    m.command("#pr");
    assert_eq!(flash(&m), "no pull request found for this agent");
    m.command("#why");
    assert_eq!(
        flash(&m),
        "only a session rush hosts has a why: open it in rush first"
    );
    assert_eq!(m.command("#quit"), Out::Quit);
    assert_eq!(m.command("#exit"), Out::Quit);
}

/// #why needs only the open session's rush id, which stays once its host
/// said it, not a live connection (keys.go:1131: m.host.sess.Info.ID).
#[test]
fn why_needs_only_the_rush_id() {
    use crate::screens::accounts::info::{InfoSheet, WHY};
    let (mut m, _) = open();
    m.sel = "work/c2".into();
    m.command("#why");
    assert_eq!(
        flash(&m),
        "only a session rush hosts has a why: open it in rush first",
        "no rush id yet"
    );
    let c = conn(&mut m, "work/c2");
    c.info.began.id = "r-1".into();
    c.link = None; // its host is gone for now
    m.flash = None;
    m.command("#why");
    let said = flash(&m);
    let s = m.sheet_as::<InfoSheet>().expect("the info sheet opens");
    assert_eq!(s.tab, WHY, "{said}");
}

/// #slim is offered in a session rush runs only once it has counted what
/// fills its context (fleetslash.go:435: m.host.sess.Usage).
#[test]
fn slim_waits_for_the_context_count() {
    let (mut m, _) = open();
    let has_slim = |m: &Model| {
        super::fleetslash::available_fleet_commands(m)
            .iter()
            .any(|c| c.name == "slim")
    };
    assert!(!has_slim(&m), "not counted yet");
    conn(&mut m, "work/c2").context = Some(Box::default());
    assert!(has_slim(&m), "counted");
    conn(&mut m, "work/c2").link = None;
    assert!(!has_slim(&m), "rush doesn't run it now");
}

/// An away that ended while you were gone opens the back sheet by
/// itself once the host says so, once; esc leaves it shut after
/// (backFromAway, away.go:111; away_test.go TestBackFromAway).
#[test]
fn an_ended_away_opens_the_back_sheet() {
    use crate::screens::session_core::host::Said;
    let (mut m, _) = open();
    let mut info = conn(&mut m, "work/c2").info.clone();
    info.away = Some(Box::new(Away {
        from: NOW - 3 * HOUR,
        until: NOW,
        ended: NOW,
        every: HOUR / 2,
        nudges: 5,
        held: vec![Held {
            kind: "question".into(),
            text: "Which DB?".into(),
            at: NOW - HOUR,
        }],
        ..Away::default()
    }));
    let r#gen = conn(&mut m, "work/c2").link_gen;
    live::take(
        &mut m,
        "work/c2",
        r#gen,
        vec![Said::Info(info.clone())],
        false,
    );
    assert!(m.sheet.is_some(), "the back sheet opens by itself");
    let f = screen(&mut m).join("\n");
    assert!(f.contains("away 3h00m · 5 check-ins"), "{f}");
    assert!(f.contains("Which DB?"), "{f}");
    press(&mut m, "esc");
    assert!(m.sheet.is_none());
    live::take(&mut m, "work/c2", r#gen, vec![Said::Info(info)], false);
    assert!(m.sheet.is_none(), "kept, it opened again");
}

/// An away still on opens nothing by itself.
#[test]
fn an_away_on_opens_nothing() {
    use crate::screens::session_core::host::Said;
    let (mut m, _) = open();
    let mut info = conn(&mut m, "work/c2").info.clone();
    info.away = Some(Box::new(Away {
        from: NOW - HOUR,
        until: NOW + HOUR,
        every: HOUR / 2,
        ..Away::default()
    }));
    let r#gen = conn(&mut m, "work/c2").link_gen;
    live::take(&mut m, "work/c2", r#gen, vec![Said::Info(info)], false);
    assert!(m.sheet.is_none());
}

/// ctrl+b (and alt+g) in the list tells an agent that stopped to go on,
/// as #go does (keys.go:467, keepGoing :747).
#[test]
fn ctrl_b_keeps_going() {
    for key in ["ctrl+b", "alt+g"] {
        let mut m = list();
        let fake = Fake::new();
        set_hosts(&mut m, Arc::new(fake.clone()));
        let i = m.selected().expect("c1 picked");
        m.agents[i].pid = 0;
        assert_eq!(press(&mut m, key), Out::Draw);
        assert_eq!(
            *fake.sent_to.lock().unwrap(),
            vec![("c1".to_string(), "keep going".to_string(), false)],
            "{key}: {}",
            flash(&m)
        );
        assert_eq!(flash(&m), "sent to tidy the parser");
    }
}

/// #intervene from the list: with no chat open it says how; @agent goes
/// into that agent's chat (keys.go:941-955, discuss.go:56 interveneOn).
#[test]
fn intervene_from_the_list() {
    let mut m = list();
    m.command("#intervene");
    assert_eq!(
        flash(&m),
        "#intervene works in a chat, or tag one: #intervene @agent"
    );
    m.command("#intervene @nobody-here it loops");
    assert_eq!(flash(&m), "no agent called @nobody-here");
    let tag = rush_room::community::name("c2");
    m.command(&format!("#intervene @{tag} it loops"));
    assert_eq!(
        m.session.as_ref().map(|v| v.key.as_str()),
        Some("work/c2"),
        "the tagged agent's chat opens"
    );
}

/// #compact with an agent picked but no session open says to open one
/// (compactsheet.go:41).
#[test]
fn compact_wants_an_open_session() {
    let mut m = list();
    m.command("#compact");
    assert_eq!(
        flash(&m),
        "#compact works on a session rush runs: open one first"
    );
}

/// #view's choices mark the layout in use; #broadcast's offer every
/// agent's @name (fleetslash.go:104 fleetArgs).
#[test]
fn hash_args_from_the_model() {
    let m = list();
    let got = |t: &str| {
        let input: Vec<char> = t.chars().collect();
        super::fleetslash::hash_matches(&m, &input, 0, &[])
            .into_iter()
            .map(|c| (c.name, c.description))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        got("#view "),
        [
            ("view split".to_string(), String::new()),
            ("view agent".to_string(), String::new()),
            ("view list".to_string(), "now".to_string()),
        ]
    );
    let names: Vec<String> = got("#broadcast ").into_iter().map(|c| c.0).collect();
    assert_eq!(
        names,
        [
            "broadcast all".to_string(),
            format!("broadcast @{}", rush_room::community::name("c1")),
            format!("broadcast @{}", rush_room::community::name("c2")),
        ]
    );
}

/// A # command typed the old way, /quit, runs as #quit and says what it's
/// called now (fleetslash.go:384 legacyCommand); a / word that isn't one
/// of rush's is left to start a session.
#[test]
fn old_slash_names_run_the_hash_command() {
    let mut m = list();
    m.input.set("/quit");
    assert_eq!(m.submit(), Out::Quit);
    assert_eq!(
        flash(&m),
        "rush's commands start with # now: #quit · / starts a session with one of Claude's"
    );
    assert!(super::fleetslash::legacy_command(&mut m, "/review the diff").is_none());
}
