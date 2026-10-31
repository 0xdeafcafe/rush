//! The bars against Go's: which segment goes first short of room and the
//! ⋯ that says so (bars_test.go TestBarLineDrops), the top bar down the
//! header's right a line a row, the gauges while it's the default, a
//! plan window's meter (view.go usageMeter), where a word stands in a
//! header row (header_context.go segAt), and the permission mode in an
//! agent's header lit under the pointer and a click away from changing
//! (header_mode_test.go).

use rush_bundled::statusline::{Bars, Layout, default_agent, default_top};
use rush_core::adapter::{self, Adapter, Features, Kind, Level, Profile, Registry};
use rush_core::agent::{Agent, State as JobState};
use rush_core::time::Ms;
use rush_core::usage::Window;
use rush_draw::{Button, KeyMods, MouseKind};

use super::agent::seg_at;
use super::line::{Ctx, bar_line, least_kept};
use super::top::usage_meter;
use super::*;
use crate::frame::Mouse;
use crate::header::Drops;
use crate::screens::session_core as core;
use crate::session::{SessionView, Source};

const NOW: Ms = 1_793_190_896_789;
const KEY: &str = "default/k1";
const HOUR: Ms = 3_600_000;

fn lay(lines: &[&[&str]]) -> Layout {
    Layout {
        lines: lines
            .iter()
            .map(|l| l.iter().map(|s| s.to_string()).collect())
            .collect(),
        sep: " · ".into(),
        ..Layout::default()
    }
}

fn model(w: usize) -> Model {
    let mut m = Model::new("0.1.0");
    m.clock = Some(NOW);
    m.now = NOW;
    m.w = w;
    m.h = 30;
    m.loaded = true;
    m.launch_dir = "/src/checkout".into();
    m
}

fn with_bars(m: &mut Model, top: Layout, agent: Layout) {
    m.kept::<State>().bars = Some(Bars { top, agent });
}

/// The lowest keep goes first, the last of those (TestLeastKept).
#[test]
fn least_kept_goes_first() {
    let cases: &[(&[&str], usize)] = &[
        (&["clock", "version", "today"], 1),
        (&["version", "statushelp"], 1),
        (&["plan", "today", "agents"], 2),
        (&["cpu"], 0),
        (&["cpu", "ram", "clock"], 1),
    ];
    for (ids, want) in cases {
        assert_eq!(least_kept(ids), *want, "{ids:?}");
    }
}

/// Short of room, the segments that hold on least go, whole, and the
/// line ends ⋯; what went is kept for /statusline (barLine).
#[test]
fn a_line_too_wide_drops_what_holds_on_least() {
    let mut m = model(120);
    with_bars(
        &mut m,
        lay(&[&["today", "statushelp", "version"]]),
        default_agent(),
    );
    let mut x = Ctx::new(&m);
    let mut drops = Drops::new();
    let all = bar_line(&m, BAR_TOP, 0, &mut x, 80, &mut drops);
    assert_eq!(all.plain(), "– today · #statusline details · rush 0.1.0");
    assert_eq!(drops, vec![(0, Vec::<String>::new())]);
    let mut drops = Drops::new();
    let cut = bar_line(&m, BAR_TOP, 0, &mut x, 35, &mut drops);
    assert_eq!(cut.plain(), "– today · #statusline details ⋯");
    assert_eq!(drops, vec![(0, vec!["version".to_string()])]);
    let mut drops = Drops::new();
    let cut = bar_line(&m, BAR_TOP, 0, &mut x, 12, &mut drops);
    assert_eq!(cut.plain(), "– today ⋯");
    assert_eq!(drops[0].1, ["version", "statushelp"]);
    let mut drops = Drops::new();
    assert_eq!(bar_line(&m, BAR_TOP, 0, &mut x, 3, &mut drops).plain(), "⋯");
    assert_eq!(bar_line(&m, BAR_TOP, 1, &mut x, 80, &mut drops).plain(), "");
}

/// The top bar's lines go down the header's right, a row each: beside
/// the counts, beside the folder, beside the tabs; while the transcripts
/// are being costed the second says so (view.go:256).
#[test]
fn the_top_bar_goes_down_the_right() {
    let mut m = model(120);
    with_bars(
        &mut m,
        lay(&[&["version"], &["statushelp"], &["today"]]),
        default_agent(),
    );
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(rows[0].trim_end().ends_with("rush 0.1.0"), "{:?}", rows[0]);
    assert!(
        rows[1].trim_end().ends_with("#statusline details"),
        "{:?}",
        rows[1]
    );
    assert!(rows[2].trim_end().ends_with("– today"), "{:?}", rows[2]);
    // Each ends two cells in from the edge, as Go's do.
    assert_eq!(rush_core::text::width(rows[0].trim_end()), 118);
    // The folder alone on the second row only while usage shows.
    assert!(rows[1].contains("· /src/checkout"), "{:?}", rows[1]);
    m.loaded = false;
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(
        rows[1].contains("costing transcripts…   #statusline details"),
        "{:?}",
        rows[1]
    );
    // What had no room is kept for /statusline to point out.
    let mut m = model(64);
    with_bars(
        &mut m,
        lay(&[&["version", "statushelp"], &[], &[]]),
        default_agent(),
    );
    m.frame();
    assert!(dropped(&m, BAR_TOP).contains("statushelp"));
}

/// The default top bar is a block of gauges down the right instead:
/// today's spend and whose plan, then the machine (headerGauges).
#[test]
fn the_default_top_bar_is_gauges() {
    let mut m = model(140);
    with_bars(&mut m, default_top(), default_agent());
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(rows[0].contains("– today · "), "{:?}", rows[0]);
    assert!(rows[1].contains("0.0G RAM · 0 agents"), "{:?}", rows[1]);
    // Its own layout draws its lines instead.
    assert!(top::header_gauges(&m, 0.0, 200).is_some());
    with_bars(&mut m, lay(&[&["version"]]), default_agent());
    assert!(top::header_gauges(&m, 0.0, 200).is_none());
}

/// A plan window: what's used, a tick for how far through the window
/// we are, the percentage, and when it resets (usageMeter).
#[test]
fn a_window_meter() {
    let win = |p: f64, left: Ms| Window {
        label: "5h".into(),
        percent: p,
        span: std::time::Duration::from_secs(5 * 3600),
        resets_at: NOW + left,
        ..Window::default()
    };
    let cases = [
        (40.0, 5 * HOUR / 2, true, "5h ━━━━─┼──── 40% ↻3h"),
        (40.0, 5 * HOUR / 2, false, "5h ━━━━─┼──── 40%"),
        (90.0, HOUR, true, "5h ━━━━━━━━╋─ 90% ↻1h"),
        (1.0, 5 * HOUR, true, "5h ╋───────── 1% ↻5h"),
    ];
    for (p, left, when, want) in cases {
        let l = usage_meter(&win(p, left), 0.0, NOW, 10, when);
        assert_eq!(l.plain(), want, "{p} {left} {when}");
    }
    // Red from 80%, yellow from 50% or ahead of pace.
    let c = |p: f64, left: Ms| {
        usage_meter(&win(p, left), 0.0, NOW, 10, false)
            .spans
            .last()
            .map(|s| s.style.fg)
    };
    assert_eq!(c(85.0, HOUR), Some(crate::style::RED));
    assert_eq!(c(55.0, HOUR), Some(crate::style::YELLOW));
    assert_eq!(c(30.0, 4 * HOUR), Some(crate::style::YELLOW));
    assert_eq!(c(30.0, HOUR), Some(crate::style::GREEN));
}

/// Where a word stands on its own in a row's part (segAt).
#[test]
fn where_a_word_stands() {
    let row = "  name  ·  idle      context 4% · plan · high ";
    let cases = [
        ("context 4% · plan · high", "plan", true, Some(34)),
        ("context 4% · plan · high", "pla", true, None),
        ("context 4% · plan · high", "", true, None),
        ("context 4% · plan · high", "high", true, Some(41)),
        ("nowhere", "plan", true, None),
    ];
    for (part, word, last, want) in cases {
        assert_eq!(seg_at(row, part, word, last), want, "{part} {word}");
    }
}

struct Plain;

impl Adapter for Plain {
    fn kind(&self) -> Kind {
        Kind::from("plain")
    }
    fn name(&self) -> String {
        "Plain Code".into()
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

/// A model with an agent's Session open, its mode as given.
fn opened(mode: &str) -> Model {
    let mut m = model(120);
    m.h = 30;
    let r = Registry::new();
    r.register(Plain);
    m.reg = r;
    let mut a = Agent {
        key: KEY.into(),
        display_name: "tidy the parser".into(),
        repo: "/src/checkout".into(),
        branch: "main".into(),
        pid: 4242,
        kind: "plain".into(),
        ..Agent::default()
    };
    a.job.id = "k1".into();
    a.job.state = JobState::Idle;
    a.job.cwd = "/src/checkout".into();
    a.rush = true;
    let src = Source::History {
        adapter: m.reg.get("plain").expect("registered"),
        session: Box::new(adapter::Session::default()),
    };
    let mut v = SessionView::new(KEY.into(), src, 1);
    v.read = true;
    m.sel = KEY.into();
    m.agents = vec![a];
    m.session = Some(v);
    m.full = true; // the Session alone
    core::conn(&mut m, KEY).info.permission_mode = mode.into();
    with_bars(
        &mut m,
        default_top(),
        lay(&[&["mode", "effort"], &["folder", "branch"]]),
    );
    m
}

fn click(m: &mut Model, kind: MouseKind, x: usize, y: usize) -> Out {
    m.mouse_event(&Mouse {
        kind,
        x: x as u16,
        y: y as u16,
        mods: KeyMods::NONE,
    })
}

/// The agent header: its bar's first line right of the name, its second
/// under it with what it runs as and how it's reached; the permission
/// mode lit under the pointer, and a click on it asks to change it
/// (Go 42172936).
#[test]
fn the_permission_mode_is_a_click_away() {
    let mut m = opened("plan");
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    let top = m.session.as_ref().unwrap().top;
    let head = top - crate::session::HEAD_H;
    let name = &rows[head + 1];
    assert!(name.contains("tidy the parser"), "{name:?}");
    assert!(name.trim_end().ends_with("plan"), "{name:?}");
    let meta = &rows[head + 2];
    assert!(meta.contains("checkout · main"), "{meta:?}");
    assert!(meta.trim_end().ends_with("send to resume"), "{meta:?}");
    let (row, at, w) = m.kept::<State>().mode_at;
    assert_eq!((row, w), (1, 4));
    let x = 2 + at;
    assert_eq!(&name[name.rfind("plan").unwrap()..][..4], "plan");
    assert_eq!(
        rush_core::text::width(&name[..name.rfind("plan").unwrap()]),
        x
    );
    // The pointer over it lights it.
    assert_eq!(click(&mut m, MouseKind::Moved, x + 1, head + 1), Out::Draw);
    assert_eq!(m.kept::<State>().hover_mode, KEY);
    assert_eq!(click(&mut m, MouseKind::Moved, x + 9, head + 1), Out::Draw);
    assert_eq!(m.kept::<State>().hover_mode, "");
    // A click on it: the permission command, the setup sheet on its
    // Permissions row (permissions.go:105-111), or why it can't open.
    click(&mut m, MouseKind::Down(Button::Left), x, head + 1);
    let said = m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default();
    match m.sheet_as::<crate::screens::accounts::startsheet::StartSheet>() {
        Some(s) => assert!(s.row == 6 && s.apply_only),
        None => assert_eq!(said, "rush can't run any harness here yet"),
    }
    // Not beside it.
    m.sheet = None;
    m.flash = None;
    click(&mut m, MouseKind::Down(Button::Left), x + 6, head + 1);
    assert!(m.flash.is_none());
}

/// A mode that doesn't ask is orange, plan blue, the rest quiet; none
/// says nothing and leaves nothing to click.
#[test]
fn the_mode_in_its_colour() {
    for (mode, want) in [
        ("plan", Some(crate::style::BLUE)),
        ("bypassPermissions", Some(crate::style::ORANGE)),
        ("default", Some(crate::style::SUB)),
        ("", None),
    ] {
        let m = opened(mode);
        let of = agent::Of::read(&m, KEY).unwrap();
        let mut x = Ctx::new(&m);
        x.a = Some(&of);
        let seg = line::segs(BAR_AGENT)
            .iter()
            .find(|s| s.id == "mode")
            .unwrap();
        let l = (seg.draw)(&x);
        assert_eq!(l.spans.first().map(|s| s.style.fg), want, "{mode}");
    }
    let mut m = opened("");
    m.frame();
    assert_eq!(m.kept::<State>().mode_at.2, 0);
}

/// Every segment /statusline lists is one the bars draw, top and agent.
#[test]
fn every_segment_is_listed() {
    for which in [BAR_TOP, BAR_AGENT] {
        let ids: Vec<String> = line::seg_infos(which).into_iter().map(|s| s.id).collect();
        assert!(ids.len() >= 18, "{ids:?}");
        let lay = if which == BAR_TOP {
            default_top()
        } else {
            default_agent()
        };
        for l in &lay.lines {
            for id in l {
                assert!(ids.contains(id), "{id} isn't a segment of {which}");
            }
        }
    }
}

/// The header's state in Go's words from its host's info: a limit pauses
/// it, an API error retried says when, offline and a cold cache say what
/// it waits for, retries run out say why (rushmode.go:2858 paneHeader).
#[test]
fn the_state_in_gos_words() {
    use crate::screens::session_core::host::{Info, Limit, Retry};
    let m = opened("default");
    let words = |info: Info, sleeping: bool| {
        let mut o = super::agent::Of::read(&m, KEY).unwrap();
        o.info = Some(&info);
        o.sleeping = sleeping;
        super::agent::state_words(&o, NOW).plain()
    };
    let retry = |r: Retry| Info {
        retry: Some(r),
        ..Info::default()
    };
    assert_eq!(words(Info::default(), false), "idle");
    assert_eq!(words(Info::default(), true), "idle");
    assert_eq!(
        words(
            Info {
                limit: Some(Limit {
                    window: "five_hour".into(),
                    ..Limit::default()
                }),
                ..Info::default()
            },
            false
        ),
        "paused · 5h limit · waiting for you"
    );
    assert_eq!(
        words(
            retry(Retry {
                attempt: 2,
                max: 10,
                next: NOW + 30_000,
                ..Retry::default()
            }),
            false
        ),
        "retrying · 2 of 10 in 30s"
    );
    assert_eq!(
        words(
            retry(Retry {
                offline: true,
                ..Retry::default()
            }),
            false
        ),
        "offline · goes on when the network is back"
    );
    assert_eq!(
        words(
            retry(Retry {
                proof: true,
                ..Retry::default()
            }),
            false
        ),
        "cache expired · goes on once the connection holds"
    );
    assert_eq!(
        words(
            retry(Retry {
                gave_up: true,
                why: "overloaded".into(),
                ..Retry::default()
            }),
            false
        ),
        "API error · overloaded"
    );
    assert_eq!(
        words(
            Info {
                error: "it died".into(),
                ..Info::default()
            },
            false
        ),
        "stopped mid-turn"
    );
}

/// A turn that ended with a workflow still at work has the header say
/// working, with what runs, not idle (TestWorkflowRunningNotIdle's
/// paneHeader).
#[test]
fn a_running_workflow_says_working() {
    use crate::screens::session_core::host::Info;
    let mut m = opened("default");
    if let Some(a) = m.agents.iter_mut().find(|a| a.key == KEY) {
        a.job.in_flight = 1;
        a.job.background = vec!["workflow\0port main's changes".into()];
    }
    let info = Info::default();
    let mut o = super::agent::Of::read(&m, KEY).unwrap();
    o.info = Some(&info);
    assert_eq!(
        super::agent::state_words(&o, NOW).plain(),
        "working · 1 workflow running"
    );
}

/// Billing and an older rush from its host's info (bars.go:188, :210;
/// stale.go:41): a host from before it said its rush is never older.
#[test]
fn billing_and_an_older_rush() {
    use crate::screens::session_core::host::Info;
    let mut m = opened("default");
    with_bars(
        &mut m,
        default_top(),
        lay(&[&["billing", "rush"], &["folder"]]),
    );
    m.kept::<State>().installed = 2_000;
    let line = |m: &Model, info: Info| {
        let mut o = super::agent::Of::read(m, KEY).unwrap();
        o.info = Some(&info);
        let mut x = Ctx::new(m);
        x.a = Some(&o);
        let mut drops = Drops::new();
        bar_line(m, BAR_AGENT, 0, &mut x, 80, &mut drops).plain()
    };
    let info = |billing: &str, exe_mod: Ms| Info {
        billing: billing.into(),
        exe_mod,
        ..Info::default()
    };
    assert_eq!(line(&m, info("overage", 1_000)), "extra usage · older rush");
    assert_eq!(line(&m, info("metered", 2_000)), "API billed");
    assert_eq!(line(&m, info("plan", 0)), "");
}

/// The name, then its @handle, then its state (mention.go:47 nameHandle).
#[test]
fn the_handle_after_the_name() {
    let mut m = opened("default");
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    let top = m.session.as_ref().unwrap().top;
    let name = &rows[top - crate::session::HEAD_H + 1];
    let a = m.agents[0].clone();
    let h = super::agent::name_handle(&a).text;
    assert!(h.starts_with('@'), "{h:?}");
    assert!(
        name.contains(&format!("tidy the parser   {h}  ·  idle")),
        "{name:?}"
    );
}

/// Temp files sums every agent's temp work as the reading carries it
/// (tempTotal, tempwork.go:80), shown once there's much (bars.go:113).
#[test]
fn temp_files_counts_what_the_reading_says() {
    let mut m = model(140);
    let mut a = rush_core::agent::Agent {
        key: "default/a:1".into(),
        ..Default::default()
    };
    a.temp = 300 << 20;
    m.agents = vec![a];
    with_bars(&mut m, lay(&[&["tmp"]]), default_agent());
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(rows[0].contains("300M tmp"), "{:?}", rows[0]);
}

/// Temp work Projects cleaned to nothing counts as nothing, not as the
/// size the reading still carries (onTemp sets a.Temp, tempwork.go:71).
#[test]
fn temp_files_cleaned_to_nothing() {
    use crate::screens::projects_machine::data::{Data, TempWork};
    let mut m = model(140);
    let mut a = rush_core::agent::Agent {
        key: "default/a:1".into(),
        ..Default::default()
    };
    a.temp = 300 << 20;
    m.agents = vec![a];
    // Listed, not measured yet: the reading's size.
    m.kept::<Data>().temp.insert(
        "default/a:1".into(),
        TempWork {
            dirs: Vec::new(),
            bytes: 0,
            known: false,
        },
    );
    with_bars(&mut m, lay(&[&["tmp"]]), default_agent());
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(rows[0].contains("300M tmp"), "{:?}", rows[0]);
    // Deleted: nothing left.
    if let Some(t) = m.kept::<Data>().temp.get_mut("default/a:1") {
        t.known = true;
    }
    let rows: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(!rows[0].contains("tmp"), "{:?}", rows[0]);
}

/// #statusline's Agent header samples are the agent it was opened on,
/// with no Session open (statuslinesheet.go:737 sample): not "(nothing
/// right now)" for each.
#[test]
fn samples_are_the_builders_agent() {
    let mut m = opened("default");
    m.session = None;
    let folder = super::line::sample(&m, super::line::BAR_AGENT, KEY, "branch").plain();
    assert_eq!(folder, "main");
    assert_eq!(
        super::line::sample(&m, super::line::BAR_AGENT, "", "branch").plain(),
        "",
        "nothing open, no agent named"
    );
}
