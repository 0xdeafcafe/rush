//! The plan's line in the header, after Go's internal/ui
//! bars_quiet_test.go (planSummary, systemAlerts, the queues) and
//! agentquota.go's nextAccount, accountChain and upcoming.

use std::time::Duration;

use rush_core::text::Line;
use rush_core::time::Ms;
use rush_core::usage::{Quota, Window};

use super::meters::MeterRow;
use super::plan::*;
use super::tests::{HOUR, MIN, NOW};
use crate::style::RED;

const DAY: Ms = 24 * HOUR;

fn win(id: &str, label: &str, span: Ms, p: f64, resets: Ms) -> Window {
    Window {
        id: id.into(),
        label: label.into(),
        span: Duration::from_millis(span as u64),
        percent: p,
        resets_at: resets,
        ..Window::default()
    }
}

/// An account's 5-hour and weekly windows, p5 and p7 used, resetting in
/// r5 and r7 (queueQuota).
fn queue_quota(now: Ms, p5: f64, p7: f64, r5: Ms, r7: Ms) -> Quota {
    Quota {
        fetched_at: now,
        windows: vec![
            win("five_hour", "5h", 5 * HOUR, p5, now + r5),
            win("seven_day", "7d", 7 * DAY, p7, now + r7),
        ],
        ..Quota::default()
    }
}

fn name(s: &str) -> Line {
    crate::style::line([crate::style::plain(s)])
}

#[test]
fn plan_summary_has_stable_windows() {
    let mut q = Quota {
        fetched_at: NOW,
        windows: vec![
            win("weekly", "7d", 7 * DAY, 35.0, NOW + 4 * DAY),
            win("short", "5h", 5 * HOUR, 58.0, NOW + 3 * HOUR),
        ],
        ..Quota::default()
    };
    assert_eq!(
        plan_summary(name("Anthropic"), &q, NOW).plain(),
        "Anthropic 5h 58% ↻3h  7d 35% ↻4d"
    );
    // The short window first, whatever's used.
    q.windows[0].percent = 99.0;
    q.windows[1].percent = 9.0;
    let got = plan_summary(name("Anthropic"), &q, NOW).plain();
    assert!(
        got.find("5h") < got.find("7d") && got.contains("7d 99%"),
        "{got}"
    );
}

#[test]
fn plan_summary_warns_before_a_window_runs_out() {
    let mut w = win("short", "5h", 5 * HOUR, 86.0, NOW + 3 * HOUR);
    w.burn = 28.0;
    let q = Quota {
        fetched_at: NOW,
        windows: vec![w],
        ..Quota::default()
    };
    assert_eq!(
        plan_summary(name("Anthropic"), &q, NOW).plain(),
        "Anthropic 5h 86% out in 20m ↻3h"
    );
}

#[test]
fn plan_summary_marks_stale_and_expired() {
    let mut q = Quota {
        fetched_at: NOW,
        windows: vec![win("", "5h", 0, 99.0, NOW - MIN)],
        ..Quota::default()
    };
    // Even a recent reading can't prove new room once its reset passed.
    let got = plan_summary(name("OpenAI"), &q, NOW).plain();
    assert!(
        got.contains("5h 99%") && got.contains("refresh needed"),
        "{got}"
    );
    q.windows[0].resets_at = NOW + HOUR;
    q.fetched_at = NOW - HOUR;
    let got = plan_summary(name("OpenAI"), &q, NOW).plain();
    assert!(got.contains("5h 99%") && got.contains("stale"), "{got}");
}

#[test]
fn system_alerts_only_when_actionable() {
    let good = system_alerts(
        Some((80.0, false)),
        Some((200 << 30, 1 << 40)),
        Some((true, true)),
    );
    assert_eq!(good.plain(), "");
    let bad = system_alerts(
        Some((10.0, false)),
        Some((3 << 30, 1 << 40)),
        Some((false, false)),
    );
    assert_eq!(bad.plain(), "network offline · disk low · battery low");
    let charging = system_alerts(Some((10.0, true)), Some((200 << 30, 1 << 40)), None);
    assert!(!charging.plain().contains("battery"));
}

fn claude(accts: Vec<InQueue>) -> Queue {
    Queue {
        kind: "claude".into(),
        glyph: "✻".into(),
        colour: None,
        accts,
    }
}

fn acct(name: &str, cur: bool, q: Quota) -> InQueue {
    InQueue {
        name: name.into(),
        cur,
        q,
    }
}

/// With room in every account, the one in use is a meter and the rest
/// are counted.
#[test]
fn queue_counts_the_ready() {
    let qu = claude(vec![
        acct(
            "personal",
            true,
            queue_quota(NOW, 2.0, 4.0, 5 * HOUR, 7 * DAY),
        ),
        acct(
            "side",
            false,
            queue_quota(NOW, 10.0, 20.0, 3 * HOUR, 6 * HOUR),
        ),
        acct(
            "work",
            false,
            queue_quota(NOW, 61.0, 30.0, 2 * HOUR, 5 * DAY),
        ),
    ]);
    let rows = plan_rows(&[qu], &[], NOW, &[200, 200, 200]);
    assert_eq!(rows[0].plain(), "✻ ▸ personal ▰▱▱▱▱▱  +2 ready");
}

/// An account that's out says so in red, and when it's back: in hours
/// within a day, else the day and time.
#[test]
fn queue_says_when_an_out_account_is_back() {
    let qu = claude(vec![
        acct(
            "personal",
            true,
            queue_quota(NOW, 100.0, 40.0, 2 * HOUR, 5 * DAY),
        ),
        acct(
            "acme",
            false,
            queue_quota(NOW, 100.0, 100.0, 3 * HOUR, 69 * HOUR),
        ),
    ]);
    let got = queue_line(&qu, NOW, 200);
    let until = super::meters::day_clock(NOW + 69 * HOUR);
    assert_eq!(
        got.plain(),
        format!("✻ ▸ personal ▰▰▰▰▰▰ out ↻2h  → acme ▰▰▰ out until {until}")
    );
    assert!(
        got.spans
            .iter()
            .any(|s| s.text == "out" && s.style.fg == RED),
        "out isn't red"
    );
}

/// Short of room the queue leaves out the others' meters, then their
/// resets, then the last of them, counted; never the one in use.
#[test]
fn queue_fits_by_dropping_in_order() {
    let qu = claude(vec![
        acct(
            "personal",
            true,
            queue_quota(NOW, 70.0, 4.0, 2 * HOUR, 7 * DAY),
        ),
        acct(
            "work",
            false,
            queue_quota(NOW, 90.0, 30.0, 2 * HOUR, 5 * DAY),
        ),
        acct(
            "acme",
            false,
            queue_quota(NOW, 85.0, 20.0, 3 * HOUR, 6 * DAY),
        ),
        acct(
            "side",
            false,
            queue_quota(NOW, 10.0, 20.0, 3 * HOUR, 6 * DAY),
        ),
    ]);
    for want in [
        "✻ ▸ personal ▰▰▰▰▱▱  → work ▰▰▰ 90% ↻2h  → acme ▰▰▰ 85% ↻3h  +1 ready",
        "✻ ▸ personal ▰▰▰▰▱▱  → work 90% ↻2h  → acme 85% ↻3h  +1 ready",
        "✻ ▸ personal ▰▰▰▰▱▱  → work 90%  → acme 85%  +1 ready",
        "✻ ▸ personal ▰▰▰▰▱▱  → work 90%  +2",
        "✻ ▸ personal ▰▰▰▰▱▱  +3",
    ] {
        let w = rush_core::text::width(want);
        assert_eq!(queue_line(&qu, NOW, w).plain(), want, "in {w} cells");
    }
    // Narrower than the account in use: it's cut, not left out.
    assert!(queue_line(&qu, NOW, 12).plain().starts_with("✻ ▸ person"));
    // One line keeps the other providers while there's room.
    let single = [MeterRow {
        kind: "codex".into(),
        glyph: "◎".into(),
        q: queue_quota(NOW, 100.0, 100.0, HOUR, 4 * DAY),
        ..MeterRow::default()
    }];
    let got = plan_rows(&[qu], &single, NOW, &[60]);
    let got = got[0].plain();
    assert!(
        got.starts_with("✻ ▸ personal ▰▰▰▰▱▱")
            && got.contains("◎ out")
            && rush_core::text::width(&got) <= 60,
        "{got}"
    );
}

/// Providers with one account and room in it are batched; one near its
/// limit is named, with its number.
#[test]
fn single_line_batches_the_quiet() {
    let row = |kind: &str, glyph: &str, p5: f64| MeterRow {
        kind: kind.into(),
        glyph: glyph.into(),
        q: queue_quota(NOW, p5, 10.0, HOUR, 4 * DAY),
        ..MeterRow::default()
    };
    let rows = [
        row("codex", "◎", 20.0),
        row("gemini", "✦", 30.0),
        row("copilot", "◈", 92.0),
    ];
    let got = single_line(&rows, NOW, 200).plain();
    assert!(
        got.starts_with("◎✦ ▰▱▱▱") && got.contains("◈ copilot 92%"),
        "{got}"
    );
}

/// rush moves to the account with the most room, unless that's nearly out
/// too; the header says which is next once the one in use is near.
#[test]
fn next_account_and_upcoming() {
    let row = |name: &str, cur: bool, p: f64| MeterRow {
        name: name.into(),
        current: cur,
        q: queue_quota(NOW, p, 10.0, HOUR, 4 * DAY),
        ..MeterRow::default()
    };
    let accts = [
        row("one", true, 99.0),
        row("two", false, 60.0),
        row("three", false, 20.0),
    ];
    let chain: Vec<String> = account_chain(&accts).into_iter().map(|r| r.name).collect();
    assert_eq!(chain, ["one", "three", "two"]);
    let (to, why) = next_account(&accts, NOW).expect("moves on");
    assert_eq!(
        (to.name.as_str(), why.as_str()),
        ("three", "one was at 99%")
    );
    let roomy = [row("one", true, 40.0), row("two", false, 20.0)];
    assert!(next_account(&roomy, NOW).is_none());
    let full = [row("one", true, 99.0), row("two", false, 97.0)];
    assert!(next_account(&full, NOW).is_none());
    assert_eq!(
        upcoming(&accts, 85.0, false, NOW).map(|r| r.name),
        Some("three".into())
    );
    assert!(upcoming(&accts, 50.0, false, NOW).is_none());
    assert!(upcoming(&accts, 85.0, true, NOW).is_none());
}
