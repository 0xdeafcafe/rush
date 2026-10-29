//! Settings › General's first half: how long agents stay where, what
//! happens to finished work, notifications, the feed, cleanups and
//! stalls. After Go's internal/ui/settings_general.go (generalSections
//! :13, offPick :206, offValue :218, onOffWord :231, cleanupValue :238,
//! countValue :249, activeValue :257, attributionSetting :269).

use std::rc::Rc;

use super::form::{Section, Setting, choice, set};
use super::paint::on_off;
use super::{config, config_mut};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// The number v starts with, as Sscanf's %d reads it; None when it
/// doesn't start with one.
pub fn scan_int(v: &str) -> Option<i64> {
    let t = v.trim_start();
    let end = t
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    t[..end].parse().ok()
}

/// A setting's number as its choice: "" for the default (0), "off" for
/// -1, else n with its unit (offPick).
pub fn off_pick(n: i64, unit: &str) -> String {
    match n {
        n if n < 0 => "off".into(),
        0 => String::new(),
        n => format!("{n}{unit}"),
    }
}

/// off_pick's choice back as the number (offValue).
pub fn off_value(v: &str) -> i64 {
    match v {
        "off" => -1,
        "" => 0,
        v => scan_int(v).unwrap_or(0),
    }
}

/// CleanupHours as its choice says it (cleanupValue).
pub fn cleanup_value(h: i64) -> String {
    match h {
        h if h < 0 => "off".into(),
        h if h > 0 => format!("{h}h"),
        _ => String::new(),
    }
}

/// A count as its choice says it: "" for the default (countValue).
pub fn count_value(n: i64) -> String {
    if n <= 0 { String::new() } else { n.to_string() }
}

/// ActiveMinutes as its choice says it (activeValue).
pub fn active_value(n: i64) -> String {
    off_pick(n, "m")
}

/// minutes from a choice: "off" -1, "" 0, else its number.
fn minutes(v: &str) -> i64 {
    off_value(v)
}

/// General's sections on how sessions live and what rush watches.
pub fn sections(m: &Model) -> Vec<Section> {
    let c = config(m);
    let mut secs = Vec::new();

    let hib = if c.hibernate.after_minutes > 0 {
        format!("{}m", c.hibernate.after_minutes)
    } else {
        "off".into()
    };
    let hibernate = choice(
        "Hibernate finished agents",
        &hib,
        "A finished agent's process stays in memory (often 300–800 MB) until it is stopped. Hibernating stops it; the conversation is kept and enter resumes it.",
        &[
            (
                "off",
                "finished agents stay in memory until you stop them (ctrl+x).",
            ),
            (
                "15m",
                "one that finished and sat idle 15 minutes is stopped.",
            ),
            (
                "30m",
                "one that finished and sat idle 30 minutes is stopped.",
            ),
            ("60m", "one that finished and sat idle an hour is stopped."),
        ],
        set(|m, v| config_mut(m).hibernate.after_minutes = scan_int(v).unwrap_or(0)),
    );

    let mut active = choice(
        "Keep stopped agents in Active for",
        &active_value(c.active_minutes),
        "An agent whose process has stopped (rested, hibernated or closed) stays in Active with the rest of what's in play for this long, then moves to Today.",
        &[
            ("", "it moves to Today 5 minutes after it was last active."),
            (
                "10m",
                "it moves to Today 10 minutes after it was last active.",
            ),
            (
                "30m",
                "it moves to Today half an hour after it was last active.",
            ),
            ("60m", "it moves to Today an hour after it was last active."),
            (
                "120m",
                "it moves to Today two hours after it was last active.",
            ),
            ("off", "it moves to Today as soon as its process stops."),
        ],
        set(|m, v| config_mut(m).active_minutes = minutes(v)),
    );
    active.unset = "5m".into();

    let mut just_left = choice(
        "Keep agents in Just left for",
        &active_value(c.just_left_minutes),
        "An agent that leaves Idle goes to Just left, most recent first, so one you missed is still easy to find; after this long it moves to Today.",
        &[
            ("", "it stays two hours after it was last active."),
            ("30m", "it stays half an hour after it was last active."),
            ("60m", "it stays an hour after it was last active."),
            ("480m", "it stays eight hours after it was last active."),
            (
                "off",
                "nothing goes to Just left; it moves straight to Today.",
            ),
        ],
        set(|m, v| config_mut(m).just_left_minutes = minutes(v)),
    );
    just_left.unset = "120m".into();
    let mut just_left_n = choice(
        "Just left holds at most",
        &count_value(c.just_left_count),
        "The most recent stay in Just left; older ones move on to Today.",
        &[
            ("", "the 10 most recent."),
            ("5", "the 5 most recent."),
            ("20", "the 20 most recent."),
            ("50", "the 50 most recent."),
        ],
        set(|m, v| config_mut(m).just_left_count = scan_int(v).unwrap_or(0)),
    );
    just_left_n.unset = "10".into();

    let mut cleanup = choice(
        "Clean up done work after",
        &cleanup_value(c.cleanup_hours),
        &rush_convo::render::key_word(
            "What happens to an agent's worktree and temp work once it's in Earlier (put away with alt+d, or a day without a turn) and left alone. Stopping an agent never removes anything. A worktree goes only if git says every change in it is committed and pushed; its branch stays. One that isn't is kept, and Projects says why on the worktree.",
        ),
        &[
            (
                "",
                "done work that's committed and pushed goes 3 hours after it was last touched.",
            ),
            (
                "1h",
                "done work that's committed and pushed goes an hour after it was last touched.",
            ),
            (
                "12h",
                "done work that's committed and pushed goes 12 hours after it was last touched.",
            ),
            (
                "24h",
                "done work that's committed and pushed goes a day after it was last touched.",
            ),
            (
                "off",
                "nothing goes by itself; x on a worktree in Projects removes it.",
            ),
        ],
        set(|m, v| config_mut(m).cleanup_hours = minutes(v)),
    );
    cleanup.unset = "3h".into();

    let orphans = choice(
        "Orphaned processes",
        if c.keep_orphans { "keep" } else { "end" },
        "What happens to what a session was running once the session itself has ended: a dev server, a watcher, a test run left going. Projects › System lists them either way.",
        &[
            (
                "end",
                "they're ended two minutes after their session ended, gently and then firmly.",
            ),
            (
                "keep",
                "they run until you end them, with x or X in Projects › System.",
            ),
        ],
        set(|m, v| config_mut(m).keep_orphans = v == "keep"),
    );

    let on_install = choice(
        "When rush is installed again",
        if c.restart_on_install {
            "restart"
        } else {
            "ask"
        },
        "What happens once a newer rush is installed (go install, or an update): sessions keep running on the one they started with until they're restarted.",
        &[
            (
                "ask",
                "nothing restarts by itself: #reload runs it here, Settings › Updates restarts the idle sessions.",
            ),
            (
                "restart",
                "each session restarts on it once it's between turns, with nothing queued, asked or running in the background, and the view reloads.",
            ),
        ],
        set(|m, v| config_mut(m).restart_on_install = v == "restart"),
    );

    secs.push(Section::new(
        "Idle and finished",
        vec![
            hibernate,
            active,
            just_left,
            just_left_n,
            cleanup,
            orphans,
            on_install,
        ],
    ));

    let notify = choice(
        "Notify when an agent needs you",
        on_off(!c.quiet),
        "A macOS notification when an agent starts waiting on you (a question or a permission), not when you have already seen it.",
        &[
            ("on", "you are notified once per new question."),
            ("off", "the Needs you section is the only signal."),
        ],
        set(|m, v| config_mut(m).quiet = v == "off"),
    );
    let mut menu = choice(
        "Menu bar icon",
        on_off(c.menu_bar),
        "rush in the menu bar: every account's usage, what's working, and who needs you, with a badge. A question with a few answers can be answered from its notification's buttons, a permission allowed or denied. The first time, it's built with Xcode's Swift compiler (a few seconds).",
        &[
            (
                "on",
                "it opens with rush, and its menu can open it at login; rush's own notifications give way to its.",
            ),
            ("off", "no menu bar icon."),
        ],
        None,
    );
    menu.run = Some(Rc::new(|m: &mut Model, v: &str| {
        let c = config_mut(m);
        c.menu_bar = v == "on";
        c.menu_bar_asked = true;
        // Starting and stopping the icon is the menu bar's own module's,
        // not in this rush yet: the choice is kept for the one that is.
        Out::Draw
    }));
    secs.push(Section::new("When an agent needs you", vec![notify, menu]));

    let feed = choice(
        "The feed",
        on_off(c.feed),
        "The feed your agents share: blockers, fixes and heads-ups, docked at the foot of the list. Off, agents are not told about it and rush feed refuses. #feed on|off says the same.",
        &[
            ("off", "no feed: agents are not told and rush feed refuses."),
            (
                "on",
                "agents may chirp and reply; the latest chirps dock at the foot of the list.",
            ),
        ],
        set(|m, v| config_mut(m).feed = v == "on"),
    );
    secs.push(Section::new("The feed", vec![feed]));

    let mut low_disk = choice(
        "Offer a cleanup when disk is under",
        &off_pick(c.cleanup_disk_gb, "G"),
        "rush watches your free disk itself, without AI. When it's this low it asks whether to start a cleanup agent (with the cleanup plugin on; off, it recommends turning it on): one that reads your worktrees, what's left in /tmp and what plugins like Haven and go-clean report, removes what can go without losing work, says what it did and goes away. Nothing starts until you say yes.",
        &[
            ("", "it asks once there's less than 20G free."),
            ("10G", "it asks once there's less than 10G free."),
            ("50G", "it asks once there's less than 50G free."),
            ("off", "it never asks about disk."),
        ],
        set(|m, v| config_mut(m).cleanup_disk_gb = off_value(v)),
    );
    low_disk.unset = "20G".into();
    let mut high_ram = choice(
        "Offer a cleanup when memory is full for",
        &off_pick(c.cleanup_ram_minutes, "m"),
        "rush watches memory pressure itself, without AI. When the memory has been under pressure this long it asks whether to start a cleanup agent, which can stop what's holding memory (Haven's stacks, dev servers left running; with the cleanup plugin on) as well as free disk. Nothing starts until you say yes.",
        &[
            ("", "it asks after 15 minutes under memory pressure."),
            ("5m", "it asks after 5 minutes under memory pressure."),
            ("30m", "it asks after 30 minutes under memory pressure."),
            ("off", "it never asks about memory."),
        ],
        set(|m, v| config_mut(m).cleanup_ram_minutes = off_value(v)),
    );
    high_ram.unset = "15m".into();
    secs.push(Section::new("Disk and memory", vec![low_disk, high_ram]));

    // What keeps a turn from hanging on a stream that went silent: shown,
    // as it's always on.
    let stalls = Setting {
        label: "Stalled turns".into(),
        what: "A model's stream can go silent without ending, leaving a turn waiting for good. Claude Code sessions rush runs have Claude Code's own stream watchdog on, which cuts such a stream and tries again, and every turn that has heard nothing from its agent for 2 minutes, with no step running, is marked stalled on its working line. After 5 minutes it moves to Stuck? and rush tells you once; rush never stops it for you.".into(),
        line: Some(Rc::new(|_| {
            line([
                sp(fit_str("Stalled turns", 32), TEXT),
                sp("✓ watched", GREEN),
                sp(
                    " · Claude Code's stream watchdog on · quiet 2m is marked stalled · 5m tells you",
                    FAINT,
                ),
            ])
        })),
        ..Setting::default()
    };
    secs.push(Section::new("Stalls", vec![stalls]));
    secs
}
