//! After internal/statusline/statusline_test.go and bars_test.go.

use std::sync::{LazyLock, Once};

use regex::Regex;
use rush_core::adapter::{Adapter, Features, Kind, Level, ModelNamer, Profile};

use super::*;
use crate::testutil::Home;

/// The Claude adapter's model names (adapters/claude/claude/modelname.go),
/// standing in for the adapter, which registers itself in rush.
struct Claude;

static MODEL_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?(\[1m\])?$").unwrap()
});
static MODEL_PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(.*\)$").unwrap());

impl ModelNamer for Claude {
    fn model_name(&self, s: &str) -> String {
        if let Some(m) = MODEL_ID.captures(s) {
            let mut v = m[2].to_string();
            if let Some(minor) = m.get(3) {
                v = format!("{v}.{}", minor.as_str());
            }
            let f = &m[1];
            return format!("{}{} {v}", f[..1].to_uppercase(), &f[1..]);
        }
        let mut s = MODEL_PAREN.replace(s, "").into_owned();
        if let Some(a) = s.strip_suffix("[1m]")
            && !a.is_empty()
            && !a.contains('-')
        {
            s = a.to_string();
        }
        if !s.is_empty() && s.to_lowercase() == s && !s.contains('-') {
            return format!("{}{}", s[..1].to_uppercase(), &s[1..]);
        }
        s
    }
}

impl Adapter for Claude {
    fn kind(&self) -> Kind {
        Kind::new("claude")
    }
    fn name(&self) -> String {
        "Claude Code".into()
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
    fn model_namer(&self) -> Option<&dyn ModelNamer> {
        Some(self)
    }
}

fn claude() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| rush_core::adapter::global().register(Claude));
}

fn input() -> Input {
    let b = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/statusline/testdata/input.json"
    ))
    .unwrap();
    rush_state::json::from_slice(&b).unwrap()
}

fn l(lines: &[&[&str]], sep: &str, plain: bool) -> Layout {
    Layout {
        plain,
        ..layout(lines, sep)
    }
}

// statusline_test.go TestRenderRealInput
#[test]
fn render_real_input() {
    claude();
    let mut i = input();
    i.cost.usd = 1.5;
    i.context.input = 250_000;
    let got = render(
        &i,
        &l(
            &[&[
                "model", "effort", "folder", "repo", "context", "cost", "nope",
            ]],
            " | ",
            true,
        ),
        "/nowhere",
        Time::now(),
    );
    assert_eq!(
        got,
        "Opus 5.5 | medium | rush | 0xdeafcafe/rush | ctx 25% | $1.50"
    );
    // Segments with nothing to say drop out, separators and all.
    i.cost.usd = 0.0;
    assert_eq!(
        render(&i, &l(&[&["cost", "model"]], "", true), "", Time::now()),
        "Opus 5.5",
        "empty cost should vanish"
    );
    // Coloured, it's the same text.
    let col = render(&i, &l(&[&["model", "folder"]], "", false), "", Time::now());
    assert_eq!(strip_sgr(&col), "Opus 5.5 · rush", "coloured: {col:?}");
    assert!(col.starts_with(ORANGE));
}

// statusline_test.go TestRunAndOurs
#[test]
fn run_and_ours() {
    let _h = Home::new();
    let mut out = Vec::new();
    run(
        &br#"{"model":{"display_name":"Sonnet 5"},"cwd":"/x/y"}"#[..],
        &mut out,
    )
    .unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("Sonnet 5") && s.contains('y'), "run: {s:?}");
    for c in [
        "rush statusline",
        "/Users/me/go/bin/rush statusline",
        "'/a b/rush' statusline",
    ] {
        assert!(ours(c), "{c} is rush's");
    }
    assert!(
        !ours("~/.claude/statusline.sh"),
        "a script of your own isn't rush's"
    );
    assert!(command().ends_with(" statusline"));
    assert_eq!(shell_quote("/a b/rush"), "'/a b/rush'");
    assert_eq!(shell_quote("/usr/bin/rush"), "/usr/bin/rush");
}

// statusline_test.go TestLinesAndYourOwn
#[test]
fn lines_and_your_own() {
    claude();
    let mut i = Input::default();
    i.model.id = "claude-opus-5-5[1m]".into();
    i.cwd = "/src/rush".into();
    let mut lay = l(&[&["model", "custom"], &[], &["folder"]], "", true);
    lay.custom = "cat >/dev/null; printf 'mine\\nsecond'".into();
    assert_eq!(
        render(&i, &lay, "", Time::now()),
        "Opus 5.5 · mine\nsecond\nrush"
    );
    for (id, want) in [
        ("claude-sonnet-5", "Sonnet 5"),
        ("claude-haiku-4-5-20251001", "Haiku 4.5"),
        ("Opus 5.5 (1M context)", "Opus 5.5"),
        ("opus", "Opus"),
    ] {
        assert_eq!(model_name(id), want, "{id}");
    }
}

// statusline_test.go TestHealthSegment: a provider whose login stopped
// working shows, by name and error; once it answers again it drops out.
#[test]
fn health_segment() {
    let _h = Home::new();
    let lay = l(&[&["health"]], "", true);
    assert_eq!(
        render(&Input::default(), &lay, "", Time::now()),
        "",
        "nothing failing should show nothing"
    );
    let now = Time::now();
    let k = rush_state::health::key("claude", "a");
    rush_state::health::failed(&k, "API Error: 401 invalid_grant", now);
    assert_eq!(
        render(&Input::default(), &lay, "", Time::now()),
        "claude ✗ 401 invalid_grant"
    );
    rush_state::health::answered(&k, now.add_nanos(1_000_000_000));
    assert_eq!(
        render(&Input::default(), &lay, "", Time::now()),
        "",
        "answered"
    );
}

#[test]
fn layout_saved_and_loaded() {
    let _h = Home::new();
    assert_eq!(load(), default_layout());
    let mut lay = l(&[&["model"], &[]], " │ ", true);
    lay.custom = "x".into();
    save(&lay).unwrap();
    assert_eq!(load(), lay);
    let b = std::fs::read_to_string(path()).unwrap();
    assert_eq!(
        b,
        "{\n  \"lines\": [\n    [\n      \"model\"\n    ],\n    []\n  ],\n  \"sep\": \" │ \",\n  \"plain\": true,\n  \"custom\": \"x\"\n}\n"
    );
}

// bars_test.go TestOldDefaultBarsMigrateWithoutChangingCustomLayouts
#[test]
fn old_default_bars_migrate_without_changing_custom_layouts() {
    let _h = Home::new();
    let old = layout(
        &[
            &["today", "usage"],
            &["ram", "tokens", "cpu", "net", "disk", "battery", "tmp"],
        ],
        " · ",
    );
    save_bars(&Bars {
        top: old.clone(),
        agent: default_agent(),
    })
    .unwrap();
    assert_eq!(load_bars().top, default_top(), "old default not migrated");
    let custom = layout(&[&["cpu", "today", "usage"], &["battery", "ram"]], " | ");
    save_bars(&Bars {
        top: custom.clone(),
        agent: default_agent(),
    })
    .unwrap();
    assert_eq!(load_bars().top, custom, "custom layout changed");
    let mut old = old;
    old.sep = " / ".into();
    save_bars(&Bars {
        top: old.clone(),
        ..Default::default()
    })
    .unwrap();
    let got = load_bars();
    assert_eq!(got.top, old, "custom separator reset");
    assert_eq!(
        got.agent,
        default_agent(),
        "a zero agent layout is its default"
    );
}

// bars_test.go TestPreviousQuietDefaultBecomesOneLine
#[test]
fn previous_quiet_default_becomes_one_line() {
    let _h = Home::new();
    let previous = layout(
        &[&["today", "plan"], &["memory", "system", "statushelp"]],
        " · ",
    );
    save_bars(&Bars {
        top: previous,
        agent: default_agent(),
    })
    .unwrap();
    let got = load_bars();
    assert!(
        got.top == default_top() && got.top.lines[1].is_empty(),
        "old quiet default not condensed: {:?}",
        got.top
    );
}
