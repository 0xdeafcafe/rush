//! The list extras' tests, after Go's internal/ui progstatus_test.go,
//! away_test.go, usagebar_test.go, agentviews_test.go (TestShareBar),
//! wall_test.go (grid, layout, step, fills the screen) and
//! listfilter_test.go, and the Agents box and its keys at Go's widths.
//! No sleeps: a model with no loop runs handed-off work at once.

use std::collections::HashMap;

use rush_core::agent::{Agent, State};
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_core::usage::{Quota, Window};

use super::*;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::worker::Fleet;

pub(super) const NOW: Ms = 1_793_190_896_789;
pub(super) const MIN: Ms = 60_000;
pub(super) const HOUR: Ms = 60 * MIN;

pub(super) fn agent(id: &str, name: &str, state: State, pid: i32) -> Agent {
    let mut a = Agent {
        key: format!("default/{id}"),
        display_name: name.into(),
        repo: "/src/checkout".into(),
        branch: "main".into(),
        pid,
        interactive: true,
        ..Agent::default()
    };
    a.job.id = id.into();
    a.job.state = state;
    a.job.cwd = "/src/checkout".into();
    a.job.created_at = NOW - 20 * MIN;
    a.job.updated_at = NOW - MIN;
    a
}

pub(super) fn model_of(agents: Vec<Agent>, w: usize, h: usize) -> Model {
    let mut m = Model::new("0.1.0");
    m.clock = Some(NOW);
    m.now = NOW;
    m.w = w;
    m.h = h;
    m.launch_dir = "/src/checkout".into();
    let roots = agents.iter().map(|a| a.repo.clone()).collect();
    m.set_fleet(Fleet {
        agents,
        roots,
        previews: HashMap::new(),
        priced: true,
    });
    m
}

fn fleet() -> Model {
    let mut ask = agent("a1", "fix the flaky tax rounding test", State::Blocked, 101);
    ask.job.needs = "Round half up, or half to even?".into();
    let mut pay = agent("b2", "add Apple Pay to checkout", State::Working, 102);
    pay.job.detail = "running pnpm test".into();
    let idle = agent("d4", "set up caching for CI", State::Idle, 104);
    model_of(vec![ask, pay, idle], 120, 40)
}

fn press(m: &mut Model, s: &str) -> Out {
    let p = Press::of(s).unwrap_or_else(|| panic!("no key {s:?}"));
    m.press_key(&p)
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

// ---- the Agents box and its keys (view.go promptLines) ----

/// The box sits over the row of keys at the foot, as wide as the screen,
/// its edge saying where a new agent starts; the keys say what you can
/// do now, at Go's widths.
#[test]
fn prompt_box_and_keys() {
    for (w, keys) in [
        (
            120,
            "  enter open  ·  ctrl+o reply  ·  ctrl+n 1 needs you  ·  ctrl+k go anywhere  ·  ctrl+r rename  ·  ? more",
        ),
        (
            70,
            "  enter open  ·  ctrl+o reply  ·  ctrl+n 1 needs you  ·  ? more",
        ),
    ] {
        let mut m = fleet();
        m.w = w;
        let f = screen(&mut m);
        let n = f.len();
        assert_eq!(f[n - 1].trim_end(), keys, "w={w}");
        assert!(
            f[n - 4].starts_with("╭─ new agent in checkout ─"),
            "{:?}",
            f[n - 4]
        );
        assert!(
            f[n - 3].starts_with("│ ❯ describe a task"),
            "{:?}",
            f[n - 3]
        );
        assert!(f[n - 2].starts_with("╰─") && f[n - 2].ends_with('╯'));
        assert_eq!(rush_core::text::width(&f[n - 4]), w);
    }
}

/// Typing changes the keys: a task offers start, a # command run it.
#[test]
fn prompt_keys_follow_the_text() {
    let mut m = fleet();
    typed(&mut m, "fix it");
    assert_eq!(m.input.string(), "fix it");
    let f = screen(&mut m);
    let last = f.last().unwrap();
    assert!(
        last.starts_with("  enter start  ·  shift+tab model"),
        "{last:?}"
    );
    assert!(last.contains("esc clear  ·  ? more"), "{last:?}");
    assert!(f.iter().any(|l| l.contains("│ ❯ fix it")), "{f:?}");

    press(&mut m, "ctrl+c");
    typed(&mut m, "#new");
    let f = screen(&mut m);
    assert_eq!(
        f.last().unwrap().trim_end(),
        "  enter run it  ·  esc clear  ·  ? guide"
    );
    assert!(
        f.iter()
            .any(|l| l.starts_with("╭─ rush command · enter runs it ─")),
        "{f:?}"
    );
}

#[test]
fn prompt_words() {
    for (s, hash, cmd) in [
        ("#", true, false),
        ("#new x", true, true),
        ("# Plan", false, false),
        ("#123", false, false),
        ("hello", false, false),
    ] {
        assert_eq!(prompt::typing_hash(s), hash, "{s}");
        assert_eq!(prompt::is_hash_cmd(s), cmd, "{s}");
    }
    assert_eq!(prompt::needs_label(1), "1 needs you");
    assert_eq!(prompt::needs_label(3), "3 need you");
    assert_eq!(prompt::short_dir("~/src/rush", 48), "~/src/rush");
    assert_eq!(
        prompt::short_dir("~/src/github.com/langwatch/.worktrees/go-ports", 30),
        "~/…/.worktrees/go-ports"
    );
}

// ---- listfilter_test.go: TestListFilterByName ----

#[test]
fn list_filter_by_name() {
    let names: Vec<Agent> = (0..30)
        .map(|i| {
            agent(
                &format!("id{i}"),
                &format!("agent number {i} doing things"),
                State::Working,
                100 + i,
            )
        })
        .collect();
    let mut m = model_of(names, 160, 40);
    let shown = |m: &Model| m.order.len();
    assert_eq!(shown(&m), 30);

    press(&mut m, "alt+f");
    assert!(filter::query(&m).is_none());
    assert!(screen(&mut m).iter().any(|l| l.contains("⌕ filter")));
    typed(&mut m, "number 22");
    assert_eq!(filter::query(&m).as_deref(), Some("number 22"));
    press(&mut m, "alt+backspace");
    assert_eq!(filter::query(&m).as_deref(), Some("number "));
    typed(&mut m, "22");
    assert_eq!(shown(&m), 1);
    assert_eq!(
        m.agents[m.order[0]].display_name,
        "agent number 22 doing things"
    );
    let f = screen(&mut m);
    assert!(f.iter().any(|l| l.contains("⌕ filter: number 22")), "{f:?}");
    assert!(f.iter().any(|l| l.ends_with("1 agent ")), "{f:?}");
    assert!(!f.iter().any(|l| l.contains("agent number 3 doing")));

    press(&mut m, "esc");
    assert!(filter::query(&m).is_none());
    assert_eq!(shown(&m), 30, "clearing shows every agent again");
    assert!(screen(&mut m).iter().any(|l| l.contains("⌕ filter")));
    press(&mut m, "esc");
    assert!(!screen(&mut m).iter().any(|l| l.contains("⌕ filter")));

    press(&mut m, "alt+f");
    typed(&mut m, "zzz");
    let f = screen(&mut m);
    assert!(
        f.iter().any(|l| l.contains("No agents match \"zzz\".")),
        "{f:?}"
    );
    for _ in 0..3 {
        press(&mut m, "backspace");
    }
    assert!(screen(&mut m).iter().any(|l| l.contains("⌕ filter")));
    press(&mut m, "backspace");
    assert!(!screen(&mut m).iter().any(|l| l.contains("⌕ filter")));
    // Typed letters went to the filter, not the Agents box.
    assert!(m.input.is_empty());
}

#[test]
fn fuzzy_matches_in_order() {
    assert!(fuzzy::matches("n22", "agent number 22").is_some());
    assert!(fuzzy::matches("22n", "agent number 22").is_none());
}

// ---- neighbour (model.go:2744) ----

#[test]
fn neighbour_in_its_section() {
    let mut a = agent("x1", "one", State::Working, 1);
    a.job.created_at = NOW - 3 * MIN;
    let mut b = agent("x2", "two", State::Working, 2);
    b.job.created_at = NOW - 2 * MIN;
    let ask = agent("x3", "asks", State::Blocked, 3);
    let m = model_of(vec![a, b, ask], 120, 40);
    let ks: Vec<String> = m
        .lines
        .iter()
        .filter_map(|l| match l {
            crate::list::ListLine::Agent(i) => Some(m.agents[*i].key.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ks.len(), 3);
    // The one alone in Needs you has none; the last of two has the first.
    assert_eq!(neighbour(&m, "default/x3"), "");
    let w: Vec<&String> = ks.iter().filter(|k| *k != "default/x3").collect();
    assert_eq!(neighbour(&m, w[0]), *w[1]);
    assert_eq!(neighbour(&m, w[1]), *w[0]);
}

// ---- wall_test.go ----

#[test]
fn wall_grid() {
    for (n, w, h, cols) in [
        (1, 200, 40, 1),
        (2, 200, 40, 2),
        (4, 200, 40, 2),
        (9, 200, 40, 3),
    ] {
        let (c, r, th) = wall::grid(&vec![1; n], w, h);
        assert!(
            c == cols && th >= wall::MIN_H && c * r >= n,
            "{n}: {c}×{r} {th}"
        );
    }
    let (_, r, th) = wall::grid(&[1; 40], 120, 30);
    assert!(
        th >= wall::MIN_H && r * wall::MIN_H <= 30,
        "40 tiles: {r} rows {th} tall"
    );
}

#[test]
fn wall_layout() {
    let shape = |rows: Vec<wall::Row>| {
        rows.iter()
            .map(|r| {
                let cells: Vec<String> = r
                    .cells
                    .iter()
                    .map(|c| format!("{}:c{}/{}/{}", c.item, c.col, c.k, c.n))
                    .collect();
                format!("{}{:?}", cells.join(" "), r.joined)
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };
    assert_eq!(
        shape(wall::layout(&[1, 3, 1], 3, 2)),
        "0:c0/0/1[false, false, false] | 1:c0/0/1 2:c1/0/1 3:c2/0/1[true, true, false] | 4:c0/0/1[false, false, false]"
    );
    assert_eq!(
        shape(wall::layout(&[6], 3, 3)),
        "0:c0/0/1 1:c1/0/3 2:c1/1/3 3:c1/2/3 4:c2/0/2 5:c2/1/2[true, true, false]"
    );
    assert_eq!(
        shape(wall::layout(&[6], 3, 1)),
        "0:c0/0/1 1:c1/0/1 2:c2/0/1[true, true, false] | 3:c1/0/1 4:c2/0/1[false, true, false] | 5:c1/0/1[false, false, false]"
    );
}

#[test]
fn wall_step() {
    let geo = wall::geos(&wall::layout(&[4, 1], 2, 3), 5);
    assert_eq!(wall::step(&geo, 2, 0, -1), Some(1));
    assert_eq!(wall::step(&geo, 2, -1, 0), Some(0));
    assert_eq!(wall::step(&geo, 0, 1, 0), Some(1));
    assert_eq!(wall::step(&geo, 3, 0, 1), Some(4));
}

fn wall_fleet(n: usize, w: usize, h: usize) -> Model {
    let states = [
        State::Working,
        State::Blocked,
        State::Working,
        State::Done,
        State::Working,
        State::Idle,
    ];
    let agents = (0..n)
        .map(|i| {
            let mut a = agent(
                &format!("id{i}"),
                &format!("agent number {i} fixing the auth flow"),
                states[i % states.len()],
                100 + i as i32,
            );
            a.job.created_at = NOW - i as Ms * MIN;
            if a.job.state == State::Blocked {
                a.job.needs = "Which database should the migration target?".into();
            }
            a
        })
        .collect();
    model_of(agents, w, h)
}

/// The Wall fills the screen at every size, nothing past its edge, and
/// what needs you is the first tile.
#[test]
fn wall_fills_the_screen() {
    for (n, w, h) in [
        (1, 120, 40),
        (3, 200, 50),
        (7, 160, 45),
        (14, 180, 40),
        (30, 100, 30),
        (2, 50, 20),
    ] {
        let mut m = wall_fleet(n, w, h);
        press(&mut m, "]");
        assert_eq!(m.agents_page, 1);
        let f = screen(&mut m);
        assert_eq!(f.len(), h, "{n} {w}x{h}");
        for l in &f {
            assert!(rush_core::text::width(l) <= w, "{n} {w}x{h}: {l:?}");
        }
        assert!(
            f.iter()
                .any(|l| l.contains("agent number") && (l.contains("╭─") || l.contains("┏━"))),
            "{f:#?}"
        );
    }
    let m = wall_fleet(6, 160, 45);
    let order = wall::wall_agents(&m, false);
    assert_eq!(m.agents[order[0]].job.state, State::Blocked);
}

#[test]
fn wall_lanes_line() {
    let mut a = agent("l", "l", State::Done, 1);
    assert_eq!(wall::lanes_line(&a), "background work running");
    a.job.background = vec!["agent\0x".into(), "agent\0y".into(), "pnpm test".into()];
    assert_eq!(wall::lanes_line(&a), "2 subagents running, 1 task running");
}

// ---- progstatus_test.go ----

#[test]
fn program_status() {
    let mut asks = agent("a", "fixer", State::Blocked, 1);
    asks.key = "a".into();
    asks.job.needs = "permission to run make".into();
    let mut works = agent("b", "builder", State::Working, 2);
    works.key = "b".into();
    works.job.detail = "running go test\n".into();
    let mut idle = agent("c", "", State::Done, 3);
    idle.key = "c".into();
    let mut agents = vec![asks, works, idle];

    let mut sent = progstatus::Sent::default();
    let out = sent.step(&agents, NOW);
    for want in [
        format!(
            "\x1b]7501;state=blocked:kind=question:msg={}:app=rush:",
            progstatus::text("1 agent waiting on you", 2048)
        ),
        format!(
            "state=blocked:id={}:kind=permission:title={}",
            progstatus::id("a"),
            progstatus::text("fixer", 192)
        ),
        format!(
            "state=working:id={}:title={}:msg={}",
            progstatus::id("b"),
            progstatus::text("builder", 192),
            progstatus::text("running go test", 2048)
        ),
    ] {
        assert!(out.contains(&want), "missing {want:?} in {out:?}");
    }
    assert!(!out.contains(&progstatus::id("c")), "{out:?}");
    assert_eq!(
        sent.step(&agents, NOW),
        "",
        "nothing changed, yet it sent again"
    );

    agents[0].job.state = State::Done;
    agents[1].job.state = State::Done;
    let out = sent.step(&agents, NOW);
    for want in [
        format!("\x1b]7501;state=clear:id={}\x1b\\", progstatus::id("a")),
        format!("\x1b]7501;state=clear:id={}\x1b\\", progstatus::id("b")),
        "\x1b]7501;state=idle:app=rush:".to_string(),
    ] {
        assert!(out.contains(&want), "missing {want:?} in {out:?}");
    }
}

#[test]
fn status_text_drops_controls() {
    assert_eq!(
        progstatus::text("a\x1b]0;x\x07b", 192),
        progstatus::text("a]0;xb", 192)
    );
    assert!(progstatus::text(&"é".repeat(200), 192).len() <= 256);
    for (s, b64) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("rush", "cnVzaA=="),
    ] {
        assert_eq!(progstatus::base64(s.as_bytes()), b64);
    }
    // SHA-1("abc") a9993e36…: an id is its first six bytes.
    assert_eq!(progstatus::id("abc"), "a9993e364706");
    assert_eq!(progstatus::kind("Allow Bash?"), "permission");
    assert_eq!(progstatus::kind("Which DB?"), "question");
}

// ---- away_test.go ----

#[test]
fn parse_away() {
    let now = NOW;
    let a = away::parse_away("3h every 15m", false, now).unwrap();
    assert!(a.until == now + 3 * HOUR && a.every == 15 * MIN && !a.looping);
    let a = away::parse_away("90m 10m", false, now).unwrap();
    assert_eq!(a.every, 10 * MIN);
    let a = away::parse_away("", true, now).unwrap();
    assert!(
        a.until == 0 && a.every == 30 * MIN && a.looping,
        "a loop needs no time"
    );
    let a = away::parse_away("every 15m", true, now).unwrap();
    assert!(a.until == 0 && a.every == 15 * MIN);
    for bad in ["", "soon", "3h 10s", "-1h", "every 15m", "3h every"] {
        assert!(away::parse_away(bad, false, now).is_err(), "{bad}");
    }
    assert_eq!(
        away::parse_away("", false, now).unwrap_err(),
        "#away how long: #away 3h, #away 3h every 15m, #away off"
    );
    for (s, ms) in [
        ("3h", Some(3 * HOUR)),
        ("1h30m", Some(90 * MIN)),
        ("1.5h", Some(90 * MIN)),
        ("0", Some(0)),
        ("x", None),
        ("3", None),
    ] {
        assert_eq!(away::parse_duration(s), ms, "{s}");
    }
}

#[test]
fn away_chip_and_back() {
    let now = NOW;
    let mut a = away::Away {
        from: now,
        until: now + HOUR,
        next: now + 10 * MIN,
        held: vec![away::Held::default()],
        ..Default::default()
    };
    assert_eq!(away::away_chip(&a, now, false).plain(), "✈ away 1h00m");
    let wide = away::away_chip(&a, now, true).plain();
    assert!(
        wide.contains("next check-in 10m · 0 sent · 1 saved"),
        "{wide}"
    );
    assert!(wide.contains("━━━━━━━━"), "{wide}");
    let l = away::Away {
        looping: true,
        next: now,
        ..Default::default()
    };
    assert_eq!(away::away_chip(&l, now, false).plain(), "⟳ loop");
    a.ended = now;
    assert!(
        away::away_chip(&a, now, false)
            .plain()
            .starts_with("✈ back · 1 saved")
    );

    let gone = away::Away {
        from: now - 3 * HOUR,
        until: now,
        ended: now,
        nudges: 5,
        held: vec![
            away::Held {
                kind: "question".into(),
                text: "Which DB?".into(),
                at: now,
            },
            away::Held {
                kind: "permission".into(),
                text: "Bash rm -rf build".into(),
                at: now,
            },
        ],
        ..Default::default()
    };
    let body: Vec<String> = away::back_lines(Some(&gone), "**All done:** shipped", now, 100, 30)
        .iter()
        .map(Line::plain)
        .collect();
    let body = body.join("\n");
    for want in [
        "away 3h00m · 5 check-ins",
        "Which DB?",
        "All done: shipped",
        "esc later",
    ] {
        assert!(body.contains(want), "{want:?} in {body}");
    }
    let r = away::held_reply(&gone.held);
    assert!(
        r.contains("- Which DB? → ") && r.contains("- you wanted Bash: rm -rf build → "),
        "{r}"
    );
    assert_eq!(away::meter(HOUR, 2 * HOUR, 8).plain(), "━━━━────");
}

// ---- usagebar_test.go ----

fn used_row(glyph: &str, label: &str, p: f64, at: Ms) -> meters::MeterRow {
    meters::MeterRow {
        glyph: glyph.into(),
        colour: None,
        label: label.into(),
        q: Quota {
            fetched_at: at,
            windows: vec![Window {
                label: "5h".into(),
                percent: p,
                ..Window::default()
            }],
            ..Quota::default()
        },
    }
}

#[test]
fn meter_row() {
    let mut gem = used_row("✦", "gemini", 30.0, NOW);
    gem.q.plan = "free".into();
    let rows = vec![
        used_row("◎", "codex · alex", 20.0, NOW),
        gem,
        used_row("◈", "copilot · gh", 90.0, NOW),
        meters::MeterRow {
            glyph: "v".into(),
            ..Default::default()
        },
    ];
    assert_eq!(
        meters::meter_row(&rows, 200).plain(),
        "  ◈ ━━━━━ 90%  ✦ free ━━─── 30%  ◎ ━──── 20%"
    );
    assert_eq!(meters::meter_row(&rows, 30).plain(), "  ◈ ━━━━━ 90%");
}

#[test]
fn switch_note() {
    let cur = used_row("", "claude · home", 92.0, NOW).q;
    let others = vec![
        used_row("", "claude · work", 85.0, NOW),
        used_row("", "OpenAI (Codex) · alex", 30.0, NOW),
        used_row("", "copilot · gh", 10.0, NOW - 2 * HOUR),
    ];
    assert_eq!(
        meters::switch_note(&cur, &others, NOW),
        "low: 92% of 5h used · OpenAI (Codex) · alex has 70% left"
    );
    assert_eq!(
        meters::switch_note(&cur, &others[..1], NOW),
        "low: 92% of 5h used"
    );
    assert_eq!(
        meters::switch_note(&used_row("", "", 60.0, NOW).q, &others, NOW),
        ""
    );
    assert_eq!(
        meters::switch_note(&used_row("", "", 92.0, NOW - HOUR).q, &others, NOW),
        ""
    );
}

#[test]
fn limit_words() {
    use crate::style::{GREEN, RED, YELLOW};
    for (p, c) in [(10.0, GREEN), (60.0, YELLOW), (84.9, YELLOW), (85.0, RED)] {
        assert_eq!(meters::usage_color(p), c, "{p}");
    }
    assert_eq!(meters::pct(41.6), "42%");
    assert_eq!(meters::limit_countdown(0, NOW), "Reset time unknown");
    assert_eq!(
        meters::limit_countdown(NOW - 1, NOW),
        "Reset time passed · refresh to check usage"
    );
    let s = meters::limit_countdown(NOW + HOUR + 2 * MIN + 3_000, NOW);
    assert!(s.starts_with("Reset in 01:02:03 · "), "{s}");
    let w = Window {
        id: "five_hour".into(),
        ..Window::default()
    };
    assert_eq!(meters::window_name(&w), "five_hour");
    assert_eq!(meters::window_name(&Window::default()), "Provider quota");
}

// ---- agentviews_test.go: TestShareBar, and its pieces ----

#[test]
fn share_bar() {
    let parts = [("3", "good"), ("1", "bad"), ("2", "quiet")];
    let b = views::share_bar(&parts, 20).expect("parts of a whole drew no bar");
    assert_eq!(b.width(), 20);
    assert_eq!(b.plain(), "━".repeat(20));
    assert!(views::share_bar(&[("41", ""), ("2", "warn")], 20).is_none());
    assert_eq!(
        views::progress_bar(0.5, 10, crate::style::GREEN).plain(),
        "━━━━━─────"
    );
    assert_eq!(
        views::spark(&[1.0, 2.0, 3.0], 2, crate::style::GREEN).plain(),
        "▁█"
    );
    assert_eq!(
        views::spark(&[5.0, 5.0], 8, crate::style::GREEN).plain(),
        "▅▅"
    );
    assert_eq!(views::views_width(200), 64);
    assert_eq!(views::views_width(100), 0);
    assert_eq!(views::tone_mark("warn").0, "▲");
}

// ---- rowmemo.go, rowindex.go ----

#[test]
fn row_memo_keeps_last_frame() {
    let mut r: rowmemo::RowMemo<u32, String> = rowmemo::RowMemo::default();
    let made = std::cell::Cell::new(0);
    let mk = |r: &mut rowmemo::RowMemo<u32, String>, k: u32| {
        r.get(k, || {
            made.set(made.get() + 1);
            format!("row {k}")
        })
    };
    mk(&mut r, 1);
    mk(&mut r, 1);
    r.turn();
    mk(&mut r, 1);
    mk(&mut r, 1);
    r.turn();
    mk(&mut r, 1); // kept from the frame before
    r.turn();
    r.turn();
    mk(&mut r, 1); // two frames unused: made again
    assert_eq!(made.get(), 4);
    assert_eq!(rowmemo::row_of(&["a", "b", "c", "b"], 10, "b", 2), Some(13));
    assert_eq!(rowmemo::row_of(&["a", "b", "c"], 10, "b", 2), Some(11));
    assert_eq!(rowmemo::row_of(&["a"], 0, "z", 0), None);
}

// ---- workstreams.go ----

#[test]
fn pick_row_follows_its_id() {
    let rows: Vec<(String, bool)> = [("a", false), ("b", true), ("c", true), ("d", false)]
        .iter()
        .map(|(s, p)| (s.to_string(), *p))
        .collect();
    let (mut pos, mut sel) = (0, "c".to_string());
    assert_eq!(work::pick_row(&rows, &mut pos, &mut sel), Some(2));
    assert_eq!(pos, 2);
    let (mut pos, mut sel) = (3, "gone".to_string());
    assert_eq!(work::pick_row(&rows, &mut pos, &mut sel), Some(2));
    assert_eq!(sel, "c");
    assert_eq!(work::pick_row(&[], &mut pos, &mut sel), None);
}

#[test]
fn work_session_lines() {
    let mut m = fleet();
    let w = 100;
    let l = |m: &Model, i: usize| work::work_session(m, &m.agents[i], w).plain();
    let ask = l(&m, 0);
    assert!(
        ask.starts_with(" ? fix the flaky tax roundin… asks: Round half up"),
        "{ask:?}"
    );
    assert!(ask.ends_with("   1m"), "{ask:?}");
    assert_eq!(rush_core::text::width(&ask), w);
    let pay = l(&m, 1);
    assert!(pay.contains("add Apple Pay to checkout"), "{pay:?}");
    assert!(pay.contains("running pnpm test"), "{pay:?}");
    m.agents[2].pid = 0;
    m.agents[2].job.state = State::Stopped;
    assert!(l(&m, 2).starts_with(" ◦ set up caching for CI"));
    assert_eq!(work::work_agents(&m).len(), 3);
    m.agents[2].job.updated_at = NOW - 25 * HOUR;
    assert_eq!(work::work_agents(&m).len(), 2);
}
