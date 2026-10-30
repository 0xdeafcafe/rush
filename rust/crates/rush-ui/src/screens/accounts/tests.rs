//! Accounts' tests, ported from Go's internal/ui onboard_test.go,
//! startsheet_test.go (TestSetupName, TestPickSetup, TestRoutes),
//! choicesheet_test.go (argNow), infosheet_test.go, harness_border_test.go,
//! logins_roomy_test.go (loginName), providers_test.go,
//! startwith_test.go, signinsheet_test.go, profiles_test.go
//! (TestStartOnDefault, TestProfilePicker) and fleet/logins_test.go
//! (NextLogin), with the frames at Go's widths. Every test has a
//! rush folder of its own (a temp dir, per thread), so nothing reads or
//! writes the real one; no sleeps: a model with no loop runs handed-off
//! work at once.

use std::cell::RefCell;

use rush_core::adapter::{Choice, Registry};
use rush_core::agent::{Agent, State};
use rush_core::text::Line;
use rush_state::Store;

use super::askrush::{GUIDE, HostedStart, ask_rush, ask_start, on_config};
use super::choices::{arg_now, choice_rows};
use super::info::*;
use super::look::{agent_name, harness_border, harness_name, look_of, provider_name};
use super::onboard::{STEPS, active_steps, did_step, note_progress, show_card, started_lines};
use super::rows::{current_login_name, login_name};
use super::setup::{
    Setup, StartOver, old_setup_name, pick_setup, routes, setup_name, start_with_words,
};
use super::state::{LoginView, accounts, cfg, cfg_mut, set_store};
use crate::frame::Out;
use crate::model::Model;

const NOW: i64 = 1_793_190_896_789;

/// A rush folder of this test's own, for this thread.
fn home() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    rush_state::set_thread_dir(Some(d.path().to_path_buf()));
    d
}

/// A registry with Claude Code in it, as the binary registers it.
fn reg() -> Registry {
    let r = Registry::new();
    r.register(rush_claude::Claude::new());
    r
}

/// A model w×h on a fresh config in a rush folder of its own.
fn model(w: usize, h: usize) -> (Model, tempfile::TempDir) {
    let d = home();
    let mut m = Model::new("0.1.0");
    m.reg = reg();
    m.clock = Some(NOW);
    m.w = w;
    m.h = h;
    set_store(&mut m, Store::default());
    (m, d)
}

fn screen(m: &mut Model) -> String {
    m.frame()
        .iter()
        .map(Line::plain)
        .collect::<Vec<_>>()
        .join("\n")
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

/// What rush said last, and of what kind: news never takes the line
/// under the box.
fn said(m: &Model) -> (crate::frame::messages::SayKind, String) {
    let l = m.said.last().expect("rush said something");
    (l.kind, l.text.clone())
}

fn needing(key: &str) -> Agent {
    let mut a = Agent {
        key: key.into(),
        display_name: key.into(),
        pid: 1,
        ..Agent::default()
    };
    a.job.state = State::Blocked;
    a.job.id = key.into();
    a
}

// ---- onboard_test.go ----

#[test]
fn getting_started_lines_at_go_widths() {
    let steps = active_steps(false);
    assert_eq!(steps.len(), 8, "the ⌘ keys only in Terminal.app");
    assert_eq!(active_steps(true).len(), STEPS.len());
    let done = vec!["start".to_string()];
    let l: Vec<String> = started_lines(&done, &steps, 150)
        .iter()
        .map(Line::plain)
        .collect();
    assert_eq!(l.len(), 2 + steps.len() + 2);
    assert_eq!(l[0], "");
    assert_eq!(l[1], " ✦ Getting started  1/8");
    assert_eq!(l[2], " ✓ Start an agent");
    // The keys end where the box's top-right label does: 4 in, less the
    // row's own space.
    assert!(l[3].starts_with(" ○ Open its Session "));
    assert!(l[3].ends_with(" { }"));
    assert_eq!(rush_core::text::width(&l[3]), 150 - 3);
    assert_eq!(
        l[11],
        " # for commands: pin, done, group, sort… · #tips off hides this"
    );
    // Narrow: the short hint, and no key where there's no room.
    let l: Vec<String> = started_lines(&[], &steps, 40)
        .iter()
        .map(Line::plain)
        .collect();
    assert_eq!(l.last().unwrap(), " # for commands · #tips off hides this");
    let l: Vec<String> = started_lines(&[], &steps, 20)
        .iter()
        .map(Line::plain)
        .collect();
    assert_eq!(l[2], " ○ Start an agent");
}

#[test]
fn getting_started_under_the_list() {
    let (mut m, _d) = model(150, 40);
    assert!(show_card(&m));
    let f = screen(&mut m);
    assert!(f.contains("✦ Getting started  0/"), "{f}");
    assert!(f.contains("See all the keys"), "{f}");
    assert!(f.contains("#tips off hides this"), "{f}");
    // The list keeps 8 rows, else the card goes.
    m.h = 16;
    assert!(!screen(&mut m).contains("See all the keys"));
    m.h = 40;
    for s in STEPS.iter() {
        did_step(&mut m, s.id);
        did_step(&mut m, s.id); // twice counts once
    }
    assert_eq!(cfg(&m).onboarding.steps.len(), STEPS.len());
    assert!(!screen(&mut m).contains("See all the keys"));
    assert!(flash(&m).contains("all set"), "{}", flash(&m));
    // Saved in this test's folder.
    let saved = std::fs::read_to_string(rush_state::dir().join("config.json")).unwrap();
    assert!(saved.contains("\"rename\""), "{saved}");
}

#[test]
fn tips_show_once() {
    let (mut m, _d) = model(150, 40);
    m.agents = vec![needing("a"), needing("b")];
    m.loaded = true;
    note_progress(&mut m);
    let first = flash(&m);
    assert_eq!(first, "● needs you · ctrl+n to go there");
    m.flash = None;
    note_progress(&mut m);
    assert!(m.flash.is_none(), "shown once: {}", flash(&m));
    m.input.insert_str("#pin");
    note_progress(&mut m);
    assert_eq!(flash(&m), "enter to run it on the picked agent");
    // A tip waits while something else is being said.
    m.input.clear();
    note_progress(&mut m);
    assert_eq!(flash(&m), "enter to run it on the picked agent");
    assert_eq!(cfg(&m).onboarding.tips, ["needs", "hash"]);
}

#[test]
fn tips_command_puts_it_away_and_back() {
    let (mut m, _d) = model(150, 40);
    did_step(&mut m, "start");
    m.command("#tips off");
    assert!(cfg(&m).onboarding.hidden);
    assert_eq!(flash(&m), "Getting started put away · #tips brings it back");
    assert!(!screen(&mut m).contains("See all the keys"));
    m.command("#tips");
    let o = &cfg(&m).onboarding;
    assert!(!o.hidden && o.steps.is_empty() && o.tips.is_empty());
    assert_eq!(flash(&m), "Getting started and tips from the top");
    assert!(screen(&mut m).contains("See all the keys"));
}

/// The keys that do a step tick it off as they go by.
#[test]
fn keys_tick_steps() {
    let (mut m, _d) = model(150, 40);
    let steps = |m: &Model| cfg(m).onboarding.steps.clone();
    m.input.insert_str("fix the flaky test");
    m.press("enter");
    assert_eq!(steps(&m), ["start"]);
    m.input.clear();
    m.input.insert_str("#sort name");
    m.press("enter");
    assert_eq!(steps(&m), ["start", "hash"]);
    // ctrl+n only when something needs you.
    m.press("ctrl+n");
    assert!(!steps(&m).contains(&"next".to_string()));
    m.input.clear();
    m.press("?");
    m.press("esc");
    assert!(steps(&m).contains(&"keys".to_string()), "{:?}", steps(&m));
    // A view another app hosts has none of it.
    super::onboard::set_hosted(&mut m, true);
    did_step(&mut m, "pin");
    assert!(!steps(&m).contains(&"pin".to_string()));
    assert!(!show_card(&m));
}

// ---- askrush.go ----

thread_local! {
    static STARTED: RefCell<Vec<HostedStart>> = const { RefCell::new(Vec::new()) };
}

#[test]
fn ask_starts_an_agent_in_rushs_folder() {
    let (mut m, _d) = model(120, 40);
    let s = ask_start("", "/home/me/.config/rush");
    assert_eq!(s.name, "asking rush");
    assert!(!s.own_worktree);
    assert!(
        s.system_prompt
            .contains("You work in rush's own folder, /home/me/.config/rush.")
    );
    assert!(s.system_prompt.contains("<rush-guide>\n"));
    assert!(s.system_prompt.ends_with("\n</rush-guide>"));
    assert!(GUIDE.len() > 1000 && s.system_prompt.contains(GUIDE));
    assert_eq!(ask_start("how do I pin?", "/x").name, "");

    // Not until the screens that host sessions say how.
    assert_eq!(ask_rush(&mut m, "hi"), Out::Draw);
    assert_eq!(flash(&m), "rush can't start sessions here yet");
    accounts(&mut m).hosted_starter = Some(|_, s| {
        STARTED.with(|v| v.borrow_mut().push(s));
        Out::Draw
    });
    m.command("#ask  how do I pin? ");
    let got = STARTED.with(|v| v.borrow().clone());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].task, "how do I pin?");
    assert_eq!(got[0].dir, rush_state::dir().to_string_lossy());
}

#[test]
fn config_changed_elsewhere_is_taken_in() {
    let (mut m, _d) = model(120, 40);
    on_config(&mut m, br#"{"onboarding":{"hidden":true}}"#);
    use crate::frame::messages::SayKind;
    assert_eq!(
        said(&m),
        (
            SayKind::News,
            "settings reloaded: config.json changed outside rush, and rush is using them".into()
        )
    );
    assert!(cfg(&m).onboarding.hidden);
    on_config(&mut m, b"{nope");
    let (kind, text) = said(&m);
    assert_eq!(kind, SayKind::News);
    assert!(
        text.starts_with("rush can't read config.json: it changed outside rush, "),
        "{text}"
    );
    assert!(
        cfg(&m).onboarding.hidden,
        "a config it can't read leaves it be"
    );
}

// ---- choicesheet.go ----

#[test]
fn arg_now_finds_the_one_running() {
    let opts: Vec<Choice> = ["opus", "opus[1m]", "sonnet", "haiku"]
        .iter()
        .map(|id| Choice {
            id: id.to_string(),
            ..Choice::default()
        })
        .collect();
    for (picked, running, want) in [
        (None, "claude-opus-5-5", ("opus", true)),
        (None, "claude-opus-5-5[1m]", ("opus[1m]", true)),
        (None, "sonnet", ("sonnet", true)),
        (None, "gpt-5.5", ("", false)),
        (None, "", ("", false)),
        (Some("haiku"), "claude-opus-5-5", ("haiku", true)),
        (Some(""), "claude-opus-5-5", ("", true)),
    ] {
        let (now, known) = arg_now(picked, running, &opts);
        assert_eq!((now.as_str(), known), want, "{picked:?} {running}");
    }
    // The agent's own default first; one the list hasn't still shows.
    let (rows, now) = choice_rows(&opts, "gpt-5.5", None);
    assert_eq!(rows[0].id, "");
    assert_eq!(rows[0].note, "the agent's own");
    assert_eq!(rows.last().unwrap().note, "running now");
    assert_eq!(now, Some(rows.len() - 1));
    let (rows, now) = choice_rows(&opts, "claude-opus-5-5", None);
    assert_eq!(now.map(|i| rows[i].id.as_str()), Some("opus"));
    let (rows, now) = choice_rows(&opts, "", Some("custom"));
    assert_eq!(rows.last().unwrap().note, "selected for this session");
    assert_eq!(now, Some(rows.len() - 1));
}

// ---- infosheet.go, infotabs.go ----

#[test]
fn info_words() {
    for (id, want) in [
        ("claude-opus-5-5[1m]", "opus 5.5 · 1M"),
        ("claude-sonnet-4-5-20250929", "sonnet 4.5"),
        ("claude-haiku-4-5", "haiku 4.5"),
        ("gpt-5.5", "gpt 5.5"),
        ("", "?"),
    ] {
        assert_eq!(pretty_model(id), want, "{id}");
    }
    for (n, want) in [
        (950, "950"),
        (1234, "1.2k"),
        (12_600, "13k"),
        (1_300_000, "1.3M"),
    ] {
        assert_eq!(short(n), want);
    }
    assert_eq!(big_tokens(2_500_000_000), "2.5B");
    assert_eq!(big_tokens(5_000_000), "5M");
    assert!(hour_spark(&[0; 24]).is_none());
    let mut h = [0; 24];
    h[9] = 4;
    h[10] = 1;
    let s = hour_spark(&h).unwrap().plain();
    assert_eq!(s.chars().count(), 24);
    assert_eq!(&s.chars().skip(8).take(3).collect::<String>(), "·▇▂");
    assert_eq!(usage_color(90.0), crate::style::RED);
    assert_eq!(usage_color(60.0), crate::style::YELLOW);
    assert_eq!(usage_color(10.0), crate::style::GREEN);
}

#[test]
fn big_meter_marks_the_window_gone() {
    let hour = std::time::Duration::from_secs(3600);
    assert_eq!(big_meter(50.0, 0, NOW, hour, 10).plain(), "━━━━━─────  50%");
    // Half the window gone: the tick in the middle, over the fill.
    let l = big_meter(70.0, NOW + 1_800_000, NOW, hour, 10).plain();
    assert!(l.starts_with("━━━━━╋━─── "), "{l}");
    assert!(l.contains(" 70% · resets in 30m, "), "{l}");
    assert!(
        big_meter(5.0, NOW - 1, NOW, hour, 10)
            .plain()
            .ends_with(" · has reset since")
    );
    // A little is never nothing.
    assert!(big_meter(1.0, 0, NOW, hour, 10).plain().starts_with("━─"));
}

#[test]
fn settings_counts_read_the_files() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("settings.json");
    std::fs::write(
        &p,
        r#"{"model":"opus","env":{"A":"1"},
            "permissions":{"allow":["Bash(ls)","Read"],"ask":["Bash(rm)"],"defaultMode":"plan"},
            "hooks":{"PreToolUse":[{"matcher":"x"},{"matcher":"y"}],"Stop":[{}]},
            "enabledPlugins":{"a@b":true,"c@d":false},
            "statusLine":{"command":"rush statusline"}}"#,
    )
    .unwrap();
    let f = rush_state::settingsfile::File::load(&p).unwrap();
    let mut n = SettingsCounts::default();
    n.add(&f, true);
    assert_eq!(
        (n.allow, n.ask, n.deny, n.hooks, n.plugins, n.env),
        (2, 1, 0, 3, 1, 1)
    );
    assert_eq!((n.model.as_str(), n.mode.as_str()), ("opus", "plan"));
    let mut both = n.clone();
    both.add(&f, false);
    assert_eq!(
        (both.allow, both.env),
        (4, 1),
        "the env shown is the first's"
    );
    let reg = reg();
    let links = settings_links(&reg, "claude", &n, &[]);
    // Only what Claude Code's adapter says it can open here.
    assert_eq!(links[0].name, "Claude Code");
    assert_eq!(links[0].value, "opus · 1 env var");
    for l in &links {
        assert!(can_screen(&reg, "claude", l.screen), "{}", l.name);
    }
}

#[test]
fn info_needs_an_open_session() {
    let (mut m, _d) = model(120, 40);
    assert_eq!(open_info(&mut m, USAGE), Out::Draw);
    assert!(m.sheet.is_none());
    assert_eq!(flash(&m), "open an agent's Session first");
}

/// A Claude Code agent with a transcript of one turn, opened.
fn opened(m: &mut Model, dir: &std::path::Path) {
    let t = dir.join("s1.jsonl");
    std::fs::write(
        &t,
        concat!(
            r#"{"type":"user","uuid":"u1","sessionId":"s1","timestamp":"2026-10-29T10:00:00Z","cwd":"/src/app","message":{"role":"user","content":"fix the tax test"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","sessionId":"s1","timestamp":"2026-10-29T10:00:05Z","message":{"id":"m1","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":1200,"output_tokens":340,"cache_read_input_tokens":30000}}}"#,
            "\n"
        ),
    )
    .unwrap();
    let mut a = Agent {
        key: "claude:default:s1".into(),
        display_name: "Fix the tax test".into(),
        branch: "main".into(),
        kind: rush_core::adapter::Kind::from("claude"),
        ..Agent::default()
    };
    a.job.id = "s1".into();
    a.job.session_id = "s1".into();
    a.job.cwd = "/src/app".into();
    a.job.state = State::Done;
    a.job.transcript_path = t.to_string_lossy().into_owned();
    m.agents = vec![a];
    m.roots = vec!["/src/app".into()];
    m.order = vec![0];
    m.sel = "claude:default:s1".into();
    m.open(0);
    assert!(m.session.is_some(), "{}", flash(m));
}

#[test]
fn info_sheet_tabs_at_go_width() {
    let (mut m, d) = model(120, 40);
    opened(&mut m, d.path());
    open_info(&mut m, HISTORY);
    let k = m.sheet_as::<InfoSheet>().expect("the info sheet");
    // History needs the agent's record; tabs it hasn't fall back to Status.
    assert_eq!(k.shown[0], STATUS);
    assert!(!k.shown.contains(&WHY), "rush logs only what it hosts");
    let tab = k.tab;
    assert!(k.shown.contains(&tab));
    let f = screen(&mut m);
    assert!(
        f.contains("Fix the tax test  the agent, its account, and Claude Code"),
        "{f}"
    );
    assert!(f.contains("Status"), "{f}");
    m.press("[");
    m.press("]");
    let k = m.sheet_as::<InfoSheet>().unwrap();
    k.tab = STATUS;
    let f = screen(&mut m);
    for want in [
        "  Session",
        "  name           Fix the tax test",
        "  folder         /src/app on main",
        "  connection     Claude Code · its daemon, read from the transcript",
        "  model          opus 5.5",
        "  Account",
        "  version        not known from a transcript",
        "esc close",
    ] {
        assert!(f.contains(want), "no {want:?} in\n{f}");
    }
    if m.sheet_as::<InfoSheet>().unwrap().shown.contains(&USAGE) {
        m.sheet_as::<InfoSheet>().unwrap().tab = USAGE;
        let f = screen(&mut m);
        assert!(f.contains("  This agent"), "{f}");
        assert!(f.contains("  tokens out     340"), "{f}");
    }
    assert_eq!(m.press("q"), Out::Close);
    assert!(m.sheet.is_none());
}

#[test]
fn context_tab_says_why_theres_no_breakdown() {
    let (mut m, d) = model(120, 40);
    opened(&mut m, d.path());
    open_info(&mut m, CONTEXT);
    let has = m.sheet_as::<InfoSheet>().unwrap().shown.contains(&CONTEXT);
    let f = screen(&mut m);
    if has {
        assert!(
            f.contains("a breakdown needs a rush-mode session")
                || f.contains("reports context usage without a detailed breakdown"),
            "{f}"
        );
    }
}

// ---- harness_border_test.go, providers_test.go, logins_roomy_test.go ----

#[test]
fn harness_border_is_quiet_and_distinct() {
    let reg = reg();
    let claude = harness_border(&reg, "claude");
    let codex = harness_border(&reg, "codex");
    assert_ne!(claude, codex, "Claude and Codex borders have the same tint");
    assert_ne!(claude, look_of(&reg, "claude").colour());
    assert_ne!(codex, look_of(&reg, "codex").colour());
}

#[test]
fn provider_name_is_not_harness() {
    let reg = reg();
    assert_eq!(harness_name(&reg, "claude"), "Claude Code");
    assert_eq!(agent_name(&reg, "claude"), "Anthropic (Claude Code)");
    assert_eq!(provider_name(&reg, &reg.provider_of("claude")), "Anthropic");
}

#[test]
fn logins_are_named_by_yours() {
    let lv = |id: &str, name: &str, email: &str, current: bool| LoginView {
        login: rush_state::Login {
            id: id.into(),
            name: name.into(),
            email: email.into(),
            ..rush_state::Login::default()
        },
        current,
        ..LoginView::default()
    };
    let logins = vec![
        lv("a", "personal", "me@x", true),
        lv("b", "alex", "alex@y", false),
    ];
    assert_eq!(login_name(&logins[1]), "alex");
    assert_eq!(current_login_name(&logins), "personal");
    assert_eq!(login_name(&lv("", "", "e@x", false)), "e@x");
    assert_eq!(login_name(&lv("0123456789", "", "", false)), "01234567");
    assert_eq!(current_login_name(&logins[1..]), "another");
}

// ---- startsheet_test.go ----

#[test]
fn setup_names() {
    let reg = reg();
    for (k, billing, account, want, old) in [
        (
            "claude",
            "",
            "Alex Work",
            "anthropic-claudecode:alex-work",
            "claudecode:alex-work",
        ),
        (
            "claude",
            "key",
            "",
            "anthropic-claudecode:key",
            "claudecode:key",
        ),
    ] {
        assert_eq!(setup_name(&reg, k, billing, account), want);
        assert_eq!(old_setup_name(&reg, k, billing, account), old);
    }
}

#[test]
fn pick_setup_takes_names_and_efforts() {
    let o = |kind: &str, account: &str, effort: &str| StartOver {
        kind: kind.into(),
        account: account.into(),
        effort: effort.into(),
        ..StartOver::default()
    };
    let ss = vec![
        Setup {
            name: "codex".into(),
            alias: vec!["codex".into()],
            o: o("codex", "", ""),
        },
        Setup {
            name: "codex:alex".into(),
            alias: vec!["codex:alex".into()],
            o: o("codex", "alex", ""),
        },
        Setup {
            name: "claudecode:alex".into(),
            alias: vec!["claude:alex".into()],
            o: o("claude", "alex", ""),
        },
    ];
    let efforts = |_: &str| {
        ["low", "high"]
            .iter()
            .map(|e| Choice {
                id: e.to_string(),
                ..Choice::default()
            })
            .collect()
    };
    for (arg, want) in [
        ("codex:alex", o("codex", "alex", "")),
        ("Claude:Alex", o("claude", "alex", "")),
        ("codex:alex:high", o("codex", "alex", "high")),
        ("codex:low", o("codex", "", "low")),
    ] {
        assert_eq!(pick_setup(&ss, arg, &efforts), Ok(want), "{arg}");
    }
    for arg in ["nope", "codex:alex:ultra", "pi", ":high"] {
        assert!(pick_setup(&ss, arg, &efforts).is_err(), "{arg}");
    }
}

#[test]
fn routes_only_valid_ones() {
    let reg = reg();
    let say = |rs: Vec<super::setup::Route>| {
        rs.iter()
            .map(|r| format!("{}={}", r.id, r.kinds.join("+")))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let kinds = vec!["claude".to_string()];
    assert_eq!(say(routes(&reg, &kinds, &|_| false)), "claude=claude");
    assert_eq!(say(routes(&reg, &[], &|_| true)), "");
}

// ---- startwith_test.go ----

#[test]
fn the_box_says_what_starts() {
    let (mut m, _d) = model(200, 50);
    let c = cfg_mut(&mut m);
    c.dispatch.kind = "claude".into();
    c.dispatch.set_start_for(
        "claude",
        rush_state::Start {
            model: "claude-opus-5-5[1m]".into(),
            effort: "high".into(),
            ..rush_state::Start::default()
        },
    );
    let w = start_with_words(&m, "/src/app");
    // The model as the adapter names it (Opus 5.5 once it has a namer).
    assert!(w.starts_with("Anthropic (Claude Code) · "), "{w}");
    assert!(w.contains("high effort"), "{w}");
    cfg_mut(&mut m)
        .dispatch
        .set_start_for("claude", rush_state::Start::default());
    let w = start_with_words(&m, "/src/app");
    assert!(w.contains("default model") && !w.contains("effort"), "{w}");
    // #with alone says it.
    m.command("#with");
    assert!(
        flash(&m).starts_with("the next session starts as "),
        "{}",
        flash(&m)
    );
    assert!(flash(&m).ends_with(" · #with harness@provider picks another, once"));
}

// ---- keys on every screen ----

#[test]
fn alt_keys_open_the_pickers() {
    let (mut m, _d) = model(120, 40);
    m.press("alt+w");
    assert!(m.picker().is_some(), "⌥w picks what new sessions run");
    m.press("esc");
    assert!(m.sheet.is_none());
    m.press("alt+m");
    if m.sheet.is_none() {
        // No harness installed here.
        assert_eq!(
            flash(&m),
            "rush can't run any harness here yet · Settings › Config"
        );
        return;
    }
    let f = screen(&mut m);
    assert!(f.contains("New session"), "{f}");
    m.press("esc");
    assert!(m.sheet.is_none());
}

// ---- every sheet at every width ----

/// Claude Code as rush will have it once its adapter is whole: every
/// feature, its program in bin, and models, efforts and modes to pick.
struct Whole(rush_claude::Claude);

impl rush_core::adapter::Adapter for Whole {
    fn kind(&self) -> rush_core::adapter::Kind {
        "claude".into()
    }
    fn name(&self) -> String {
        "Claude Code".into()
    }
    fn features(&self) -> rush_core::adapter::Features {
        use rush_core::adapter::{Feature, Support};
        [
            "run",
            "model",
            "effort",
            "modes",
            "switch",
            "signin",
            "quota",
            "settings",
            "stats",
            "context",
            "images",
            "resume",
            "rewind",
            "subagents",
        ]
        .iter()
        .filter_map(|w| Feature::from_word(w))
        .map(|f| (f, Support::YES))
        .collect()
    }
    fn level(&self) -> rush_core::adapter::Level {
        rush_core::adapter::Level::Full
    }
    fn profiles(&self) -> Vec<rush_core::adapter::Profile> {
        Vec::new()
    }
    fn claude_transcripts(&self) -> bool {
        true
    }
    fn native(&self) -> Option<&dyn rush_core::adapter::Native> {
        rush_core::adapter::Adapter::native(&self.0)
    }
    rush_core::has!(programmer: Programmer, chooser: Chooser);
}

impl rush_core::adapter::Programmer for Whole {
    fn program(&self) -> (String, Vec<String>) {
        ("claude".into(), Vec::new())
    }
}

impl rush_core::adapter::Chooser for Whole {
    fn choices(&self) -> rush_core::adapter::Choices {
        let ch = |ids: &[&str]| {
            ids.iter()
                .map(|id| Choice {
                    id: id.to_string(),
                    note: "a note long enough to be cut at a narrow width".into(),
                    context: 0,
                })
                .collect()
        };
        rush_core::adapter::Choices {
            models: ch(&["opus", "opus[1m]", "sonnet", "haiku", "語言模型🙂"]),
            efforts: ch(&["low", "medium", "high"]),
            modes: ch(&["default", "plan", "auto"]),
        }
    }
}

/// A model on Whole, its program in a folder of its own, with logins and
/// sign-ins whose names are wide.
fn whole_model() -> (Model, tempfile::TempDir, tempfile::TempDir) {
    let d = home();
    let bin = tempfile::tempdir().unwrap();
    let p = bin.path().join("claude");
    std::fs::write(&p, "#!/bin/sh\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let reg = Registry::with_env(&bin.path().to_string_lossy(), &d.path().to_string_lossy());
    reg.register(Whole(rush_claude::Claude::new()));
    // State's list of agents is told this registry alone, not whatever
    // another test's told it (claude found here, missing there).
    rush_state::agents::use_thread_list();
    let mut m = Model::new("0.1.0");
    m.reg = reg;
    m.clock = Some(NOW);
    let mut s = Store::default();
    for (id, name) in [("id-1", "工作 account 🙂"), ("id-2", "home")] {
        s.config.logins.push(rush_state::Login {
            id: id.into(),
            name: name.into(),
            email: format!("{name}@example.com"),
            ..Default::default()
        });
    }
    set_store(&mut m, s);
    (m, d, bin)
}

#[test]
fn sheets_draw_at_every_width() {
    let (mut m, d, _bin) = whole_model();
    opened(&mut m, d.path());
    m.session = None;
    let sizes = [
        (1, 1),
        (2, 3),
        (8, 4),
        (20, 6),
        (40, 12),
        (64, 20),
        (80, 30),
        (120, 40),
        (200, 50),
    ];
    type Open = (&'static str, fn(&mut Model));
    let opens: [Open; 6] = [
        ("new", |m| {
            m.press("alt+m");
        }),
        ("new list", |m| {
            m.press("alt+m");
            m.press("right");
            m.press("x");
        }),
        ("start", |m| {
            let o = super::setup::next_start(m, "/work/rush");
            super::startsheet::open_switch_sheet(m, "k", o);
        }),
        ("profiles", |m| {
            m.press("alt+w");
        }),
        ("signed out", |m| {
            let o = super::setup::next_start(m, "/work/rush");
            let mut c = super::signedout::Card::default();
            for w in [0, 1, 5, 30, 120] {
                super::signedout::signed_out_rows(
                    m,
                    "工作 account 🙂",
                    "Not logged in",
                    &o,
                    &mut c,
                    NOW,
                    w,
                );
                c.in_modal = !c.in_modal;
                super::signedout::signed_out_rows(m, "", "", &o, &mut c, NOW, w);
            }
        }),
        ("sign in", |m| {
            let start: super::signin::Start = std::sync::Arc::new(|| Err("no program here".into()));
            let mut s = super::signin::SignInSheet::new("Claude Code", start);
            s.url = "https://example.test/oauth?code=true&state=s1&名=🙂".into();
            s.asks = true;
            m.open_sheet(Box::new(s));
        }),
    ];
    for (w, h) in sizes {
        m.w = w;
        m.h = h;
        for (name, open) in &opens {
            m.sheet = None;
            open(&mut m);
            let f = screen(&mut m);
            let want = match *name {
                "new" | "new list" => "New session",
                "start" => "Switch to",
                "profiles" => "New sessions run",
                "sign in" => "Sign in",
                _ => "",
            };
            if w == 120 {
                assert!(f.contains(want), "{name}:\n{f}");
            }
            // Narrower than a sheet's least width the terminal cuts it, as
            // Go's is.
            for (i, l) in f.lines().enumerate().filter(|_| w >= 80) {
                assert!(
                    rush_core::text::width(l) <= w,
                    "{name} at {w}x{h}: line {i} is too wide: {l}"
                );
            }
            m.press("esc");
            m.press("esc");
        }
    }
    // The info sheet's tabs, on the conversation.
    for (w, h) in sizes {
        m.w = w;
        m.h = h;
        m.sheet = None;
        m.open(0);
        for tab in [STATUS, CONTEXT, USAGE, HISTORY, SETTINGS] {
            open_info(&mut m, tab);
            screen(&mut m);
        }
        m.sheet = None;
        m.command("#model");
        screen(&mut m);
        m.sheet = None;
        m.session = None;
    }
    // Getting started at the list's foot, and its lines.
    for w in [0, 1, 3, 10, 40] {
        let steps = active_steps(true);
        started_lines(&[], &steps, w);
    }
}

// ---- fleet/logins_test.go: the login to move to ----

fn login_at(id: &str, current: bool, five_hour: f64, seven_day: f64) -> LoginView {
    use rush_core::usage::{Quota, Window};
    let win = |id: &str, hours: u64, percent: f64| Window {
        id: id.into(),
        span: std::time::Duration::from_secs(hours * 3600),
        percent,
        ..Window::default()
    };
    LoginView {
        login: rush_state::Login {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        },
        current,
        quota: Quota {
            account: id.into(),
            fetched_at: NOW,
            windows: vec![
                win("five_hour", 5, five_hour),
                win("seven_day", 7 * 24, seven_day),
            ],
            ..Quota::default()
        },
        ..LoginView::default()
    }
}

fn next_id(logins: &[LoginView], stopped: bool) -> String {
    super::autoswitch::next_login(logins, stopped, NOW)
        .map(|l| l.id.clone())
        .unwrap_or_default()
}

#[test]
fn next_login() {
    let cases: &[(&str, Vec<LoginView>, bool, &str)] = &[
        (
            "room left",
            vec![
                login_at("a", true, 60.0, 40.0),
                login_at("b", false, 0.0, 0.0),
            ],
            false,
            "",
        ),
        (
            "idle at 96% runs on",
            vec![
                login_at("a", true, 96.0, 40.0),
                login_at("b", false, 10.0, 20.0),
            ],
            false,
            "",
        ),
        (
            "5h nearly out",
            vec![
                login_at("a", true, 99.0, 40.0),
                login_at("b", false, 10.0, 20.0),
            ],
            false,
            "b",
        ),
        (
            "7d nearly out",
            vec![
                login_at("a", true, 10.0, 99.0),
                login_at("b", false, 10.0, 20.0),
            ],
            false,
            "b",
        ),
        (
            "most week to lose wins",
            vec![
                login_at("a", true, 99.0, 40.0),
                login_at("b", false, 50.0, 20.0),
                login_at("c", false, 5.0, 30.0),
            ],
            false,
            "b",
        ),
        (
            "others nearly out too",
            vec![
                login_at("a", true, 99.0, 40.0),
                login_at("b", false, 95.0, 20.0),
            ],
            false,
            "",
        ),
        (
            "out, the others nearly",
            vec![
                login_at("a", true, 100.0, 27.0),
                login_at("b", false, 98.0, 12.0),
                login_at("c", false, 0.0, 97.0),
            ],
            false,
            "c",
        ),
        (
            "out, the others too",
            vec![
                login_at("a", true, 100.0, 27.0),
                login_at("b", false, 100.0, 12.0),
            ],
            false,
            "",
        ),
        (
            "stopped, the others nearly out",
            vec![
                login_at("a", true, 90.0, 40.0),
                login_at("b", false, 97.0, 20.0),
            ],
            true,
            "b",
        ),
        (
            "stopped switches below the threshold",
            vec![
                login_at("a", true, 80.0, 40.0),
                login_at("b", false, 90.0, 20.0),
            ],
            true,
            "b",
        ),
        ("only one", vec![login_at("a", true, 99.0, 99.0)], true, ""),
    ];
    for (name, logins, stopped, want) in cases {
        assert_eq!(next_id(logins, *stopped), *want, "{name}");
    }
}

/// Room left in a week that resets in hours is lost unless it's spent
/// now; one that resets in days can wait.
#[test]
fn next_login_spends_what_resets_soonest() {
    const H: i64 = 3_600_000;
    let week = |mut l: LoginView, in_ms: i64| {
        for w in l.quota.windows.iter_mut().filter(|w| w.id == "seven_day") {
            w.resets_at = NOW + in_ms;
        }
        l
    };
    let cases: Vec<(&str, Vec<LoginView>, &str)> = vec![
        (
            "hours beats days",
            vec![
                week(login_at("a", true, 12.0, 2.0), 72 * H),
                week(login_at("b", false, 0.0, 77.0), 4 * H),
            ],
            "b",
        ),
        (
            "days stays on hours",
            vec![
                week(login_at("a", true, 12.0, 77.0), 4 * H),
                week(login_at("b", false, 0.0, 2.0), 72 * H),
            ],
            "",
        ),
        (
            "too little to move for",
            vec![
                week(login_at("a", true, 12.0, 2.0), 72 * H),
                week(login_at("b", false, 0.0, 95.0), H),
            ],
            "",
        ),
        (
            "nearly out takes the most urgent",
            vec![
                week(login_at("a", true, 99.0, 2.0), 72 * H),
                week(login_at("b", false, 0.0, 0.0), 100 * H),
                week(login_at("c", false, 0.0, 60.0), 10 * H),
            ],
            "c",
        ),
        (
            "out of 5h for now",
            vec![
                week(login_at("a", true, 12.0, 2.0), 72 * H),
                week(login_at("b", false, 100.0, 77.0), 4 * H),
            ],
            "",
        ),
    ];
    for (name, logins, want) in cases {
        assert_eq!(next_id(&logins, false), want, "{name}");
    }
}

#[test]
fn next_login_skips_logins_without_reading() {
    let unread = LoginView {
        login: rush_state::Login {
            id: "b".into(),
            ..Default::default()
        },
        ..LoginView::default()
    };
    let mut stale = login_at("c", false, 0.0, 0.0);
    stale.quota.fetched_at = NOW - 2 * 3_600_000;
    let mut older = login_at("e", false, 0.0, 0.0);
    older.quota.fetched_at = NOW - 40 * 60_000;
    assert_eq!(
        next_id(&[login_at("a", true, 100.0, 55.0), older], false),
        "e",
        "a login not in use only empties"
    );
    assert_eq!(
        next_id(
            &[
                login_at("a", true, 100.0, 55.0),
                unread,
                stale,
                login_at("d", false, 1.0, 0.0)
            ],
            true
        ),
        "d",
        "the only one with a reading"
    );
}

#[test]
fn next_login_ignores_stale_reading() {
    let mut cur = login_at("a", true, 99.0, 40.0);
    cur.quota.fetched_at = NOW - 3_600_000;
    assert_eq!(
        next_id(&[cur, login_at("b", false, 0.0, 0.0)], false),
        "",
        "an hour-old reading"
    );
}

/// Nearly out of the login in use, rush moves new sessions to the next
/// through the vault, once; with the default profile waiting, it doesn't.
#[test]
fn auto_switch_moves_to_the_next_login() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SWITCHED: AtomicUsize = AtomicUsize::new(0);
    let (mut m, _d) = model(120, 40);
    accounts(&mut m).logins = vec![
        login_at("a", true, 99.0, 40.0),
        login_at("b", false, 10.0, 20.0),
    ];
    accounts(&mut m).login_ops = Some(super::keys::LoginOps {
        switch: |m, to, why, mv| {
            assert_eq!(to.id, "b");
            assert!(mv, "nearly out: running sessions move too");
            assert_eq!(why, "a was at 99%");
            SWITCHED.fetch_add(1, Ordering::Relaxed);
            accounts(m).login_switched_at = m.clock.unwrap();
            Out::Draw
        },
        renew: |_, _| Out::Idle,
        add: |_, _| Out::Idle,
        forget: |_, _| {},
    });
    super::autoswitch::auto_switch(&mut m);
    assert_eq!(SWITCHED.load(Ordering::Relaxed), 1);
    // Not again until the readings catch up.
    super::autoswitch::auto_switch(&mut m);
    assert_eq!(SWITCHED.load(Ordering::Relaxed), 1);
    accounts(&mut m).login_switched_at = 0;
    cfg_mut(&mut m).at_limit = rush_state::setup::AT_LIMIT_WAIT.into();
    super::autoswitch::auto_switch(&mut m);
    assert_eq!(
        SWITCHED.load(Ordering::Relaxed),
        1,
        "the default profile waits"
    );
}

// ---- signinsheet_test.go, profiles_test.go ----

/// The link is read whole though the program's output comes in pieces
/// that cut it, and a character, in two; then whether it asks for a code,
/// and what it said last (TestSignInSheet's reading of a fake login).
#[test]
fn sign_in_sheet_reads_the_link_and_the_ask() {
    let start: super::signin::Start = std::sync::Arc::new(|| Err("not run".into()));
    let mut s = super::signin::SignInSheet::new("Test", start);
    s.took("Opening browser to sign in…\nIf the browser did not open, visit: \x1b]8;;https://example.test/oauth?co");
    s.took("de=true&state=s1\x07https://example.test/oauth?code=true&state=s1\x1b]8;;\x07\n");
    assert_eq!(s.url, "https://example.test/oauth?code=true&state=s1");
    assert!(!s.asks);
    s.took("Paste code here if prompted > ");
    assert!(s.asks);
    s.sent = true;
    s.took("Invalid code. Please make sure the full code was copied.\n");
    assert!(!s.sent, "it answered the code");
    assert!(s.said().contains("Invalid code"), "{}", s.said());
    assert_eq!(super::signin::first_url("no link"), "");
}

/// ctrl+t hands the terminal to a fresh sign-in: the sheet closes, the
/// program runs once the key's been handled, and finish runs once it has
/// ended well; one that fails says why (toTerminal, inTerminal).
#[test]
fn sign_in_goes_to_the_terminal() {
    let start = |prog: &'static str| -> super::signin::Start {
        std::sync::Arc::new(move || {
            let finish: super::signin::Finish = Box::new(|| {
                Box::new(|m: &mut Model| {
                    m.flash("signed in");
                    Out::Draw
                })
            });
            Ok((std::process::Command::new(prog), finish))
        })
    };
    let mut m = Model::new("t");
    m.open_sheet(Box::new(super::signin::SignInSheet::new(
        "Test",
        start("/usr/bin/true"),
    )));
    m.press("ctrl+t");
    assert!(m.sheet.is_none(), "the sheet closes");
    assert_eq!(m.run_outside(), 1);
    assert_eq!(m.said.last().unwrap().text, "signed in");
    m.open_sheet(Box::new(super::signin::SignInSheet::new(
        "Test",
        start("/usr/bin/false"),
    )));
    m.press("ctrl+t");
    m.run_outside();
    assert_eq!(m.said.last().unwrap().text, "exit status 1");
}

/// An expired login's row says r refreshes it (TestExpiredLoginRow).
#[test]
fn expired_login_row() {
    let (m, _d) = model(120, 40);
    let r = super::rows::AcctRow {
        login: Some(LoginView {
            login: rush_state::Login {
                id: "id-1".into(),
                ..Default::default()
            },
            ..LoginView::default()
        }),
        q: rush_core::usage::Quota {
            problem: format!(
                "couldn't tell whose sign-in it is: {}",
                rush_core::usage::EXPIRED
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        super::rows::problem(&m, &r),
        "sign-in expired · r refreshes it"
    );
}

/// A session's provider is the default one, wherever it starts
/// (TestStartOnDefault); ⌥w opens on it and enter keeps it the default
/// (TestProfilePicker).
#[test]
fn start_on_default_and_the_picker() {
    let (mut m, _d, _bin) = whole_model();
    assert_eq!(super::rows::start_kind_in(&m, "/elsewhere"), "claude");
    assert_eq!(super::rows::start_kind_in(&m, "/work/repo"), "claude");
    m.w = 120;
    m.h = 40;
    m.press("alt+w");
    let p = m.picker().expect("the picker");
    let at = p.rows[p.cursor].label.plain();
    assert!(at.starts_with("★ ✻ "), "opened on the default: {at}");
    assert!(at.ends_with(" only"), "{at}");
    m.press("enter");
    assert!(m.sheet.is_none());
    assert!(
        flash(&m).contains("is the default profile · "),
        "{}",
        flash(&m)
    );
}

// agentquota.go:166-195: with the first provider's accounts back to
// having room, new sessions run it again, said as news when rush notices
// on its own; only the rush that leads does (agentquota.go:95), and an
// answer when asked.
#[test]
fn spill_comes_back_said_by_the_lead() {
    use crate::frame::messages::SayKind;
    let (mut m, _d, _bin) = whole_model();
    m.follows = true;
    accounts(&mut m).spill = "codex".into();
    super::profiles::auto_spill(&mut m);
    assert_eq!(
        super::state::peek(&m).spill,
        "codex",
        "a rush that follows moves nothing"
    );
    m.follows = false;
    super::profiles::auto_spill(&mut m);
    assert_eq!(super::state::peek(&m).spill, "");
    assert_eq!(
        said(&m),
        (
            SayKind::News,
            "new sessions run Claude Code again: its accounts have room".into()
        )
    );
    accounts(&mut m).spill = "codex".into();
    super::profiles::spill_to(&mut m, true);
    assert_eq!(flash(&m), "new sessions run Claude Code again");
    // Unchanged, nothing's said.
    let n = m.said.len();
    super::profiles::spill_to(&mut m, false);
    assert_eq!(m.said.len(), n);
}

// logins.go:437-466: a switch rush made on its own is news, with why and
// what follows; one you asked for an answer; one that failed on its own
// is news too.
#[test]
fn login_switches_say_what_they_did() {
    use super::keys::{LoginSwitched, Restored, login_switched, logins_found};
    use crate::frame::messages::SayKind;
    let (mut m, _d, _bin) = whole_model();
    login_switched(
        &mut m,
        LoginSwitched {
            to: "home".into(),
            why: "work was at 99%".into(),
            resumed: 1,
            waiting: 2,
            stay: true,
            err: None,
        },
    );
    assert_eq!(
        said(&m),
        (
            SayKind::News,
            "switched to home: work was at 99% · new sessions run on home · running ones follow once their cache cools · 1 session the limit stopped carry on · 2 busy sessions will switch after the current turn".into()
        )
    );
    login_switched(
        &mut m,
        LoginSwitched {
            to: "home".into(),
            ..LoginSwitched::default()
        },
    );
    assert_eq!(flash(&m), "new sessions run on home");
    login_switched(
        &mut m,
        LoginSwitched {
            to: "home".into(),
            why: "a session hit a usage limit".into(),
            err: Some("no keychain".into()),
            ..LoginSwitched::default()
        },
    );
    assert_eq!(
        said(&m).1,
        "rush couldn't switch to home: a session hit a usage limit · no keychain"
    );
    // logins.go:102-118: a failing look is said once; a login put back.
    logins_found(&mut m, Some("locked".into()), None);
    logins_found(&mut m, Some("locked".into()), None);
    let n = m
        .said
        .iter()
        .filter(|l| l.text == "rush can't switch account: locked")
        .count();
    assert_eq!(n, 1);
    logins_found(
        &mut m,
        None,
        Some(Restored {
            was: "id-2".into(),
            now: "id-9".into(),
            err: None,
        }),
    );
    assert_eq!(
        said(&m).1,
        "rush switched back to another account: a Claude Code started on home had signed ~/.claude back in as it"
    );
}
