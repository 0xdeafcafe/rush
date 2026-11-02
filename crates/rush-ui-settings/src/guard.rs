//! Settings › Usage's Runaway guard (Rust-only): the pace guard, the
//! cap on agents working at once and what's stricter while you're away,
//! each off until you turn it on. What they do is the accounts module's
//! (screens/accounts/guard.rs) and rush_core::guard's.

use std::collections::HashMap;

use super::form::{Section, Setting, choice, set};
use super::paint::on_off;
use super::{config, config_mut};
use crate::model::Model;

/// A number as a choice: "" for 0, the default.
fn pick(n: f64) -> String {
    if n <= 0.0 {
        String::new()
    } else if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

/// A row of numbers to choose from, each named by name; "" is unset.
pub(crate) fn numbers(
    label: &str,
    value: f64,
    what: &str,
    xs: &[f64],
    unset: &str,
    name: &dyn Fn(f64) -> String,
    set_to: impl Fn(&mut Model, f64) + 'static,
) -> Setting {
    let mut names = HashMap::new();
    let mut means = HashMap::new();
    let mut choices = Vec::new();
    for x in xs {
        let k = pick(*x);
        if *x > 0.0 {
            names.insert(k.clone(), name(*x));
        }
        means.insert(k.clone(), String::new());
        choices.push(k);
    }
    Setting {
        label: label.into(),
        value: pick(value),
        choices,
        names,
        means,
        unset: unset.into(),
        what: what.into(),
        set: set(move |m, v| set_to(m, v.parse::<f64>().unwrap_or(0.0))),
        ..Setting::default()
    }
}

/// The section.
pub fn sections(m: &Model) -> Vec<Section> {
    let g = config(m).guard.clone();
    let mut rows = vec![choice(
        "Pace guard",
        on_off(g.pace),
        "Keeps each account's week from going in a few hours. An account's fair share is its weekly room left spread over the hours you work before its weekly reset, plus what the agent's other accounts leave unused (work can move there); its pace is how fast it used its week over the last 2 hours, from its plan's own readings, so every machine and session on it counts. Past the warning rush says so; past the hold, new turns go to the account with the most room against its pace, or wait when every account is past its own. A running turn always finishes. A session whose prompt cache is warm waits rather than move, as moving writes its whole context again; a cold one moves, at most once in the hours set. A provider that reports no limits has no pace: only the cap and away cover it.",
        &[
            ("off", "rush spends each account as it would."),
            (
                "on",
                "an account past its pace has new turns move or wait; running turns finish.",
            ),
        ],
        set(|m, v| config_mut(m).guard.pace = v == "on"),
    )];
    if g.pace {
        let times = |x: f64| format!("{x}× its fair share");
        rows.push(numbers(
            "  warn at",
            g.pace_warn,
            "How fast against its fair share an account goes before rush says so. Work moves back to an account only once it's 0.3× under this.",
            &[0.0, 1.25, 2.0],
            "1.5× its fair share",
            &times,
            |m, v| config_mut(m).guard.pace_warn = v,
        ));
        rows.push(numbers(
            "  hold at",
            g.pace_trip,
            "How fast against its fair share an account goes before its new turns move or wait.",
            &[0.0, 1.5, 2.5, 3.0],
            "2× its fair share",
            &times,
            |m, v| config_mut(m).guard.pace_trip = v,
        ));
        rows.push(numbers(
            "  you work",
            g.pace_hours as f64,
            "How many hours a day you use the accounts. A week's room is spread over those hours to the reset, not round the clock, so a working day may spend more of it.",
            &[0.0, 8.0, 12.0, 15.0, 18.0, 24.0],
            "15 hours a day",
            &|x| format!("{x:.0} hours a day"),
            |m, v| config_mut(m).guard.pace_hours = v as i64,
        ));
        rows.push(numbers(
            "  moves",
            g.move_hours as f64,
            "How often the guard may move one session to another account. Each move writes its context again on the new account; it never moves one back and forth.",
            &[0.0, 3.0, 12.0, 24.0],
            "a session at most once in 6h",
            &|x| format!("a session at most once in {x:.0}h"),
            |m, v| config_mut(m).guard.move_hours = v as i64,
        ));
    }
    rows.push(numbers(
        "Agents at once",
        g.cap as f64,
        "At most this many agents working at once. A session's turn counts 1, and each subagent and workflow run it has going 1 more; background shells don't. Agents rush doesn't run, in a terminal, count towards it but are never held. Past it a session's next turn waits, shown as held, and goes in the order held as others finish; new subagents and workflows are refused with the reason. alt+h in a session lets a held turn through.",
        &[0.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0],
        "no cap",
        &|x| format!("{x:.0}"),
        |m, v| config_mut(m).guard.cap = v as i64,
    ));
    rows.push(choice(
        "When you're away",
        on_off(g.away),
        "Stricter while you're away: no key in rush or anywhere on the Mac for the time set, or its screen locked. Each session's running turn finishes, then holds; new subagents and workflows are refused; the cap is the one for away. A session you've set to work on while you're away (#away) carries on. Your first key back lets everything go.",
        &[
            ("off", "rush works the same whether you're there or not."),
            ("on", "sessions hold after their turn while you're away."),
        ],
        set(|m, v| config_mut(m).guard.away = v == "on"),
    ));
    if g.away {
        rows.push(numbers(
            "  away after",
            g.away_minutes as f64,
            "How long with no key before you count as away.",
            &[0.0, 10.0, 30.0, 60.0],
            "20 minutes",
            &|x| format!("{x:.0} minutes"),
            |m, v| config_mut(m).guard.away_minutes = v as i64,
        ));
        rows.push(numbers(
            "  agents at once",
            g.away_cap as f64,
            "The cap while you're away.",
            &[0.0, 1.0, 3.0, 4.0],
            "2",
            &|x| format!("{x:.0}"),
            |m, v| config_mut(m).guard.away_cap = v as i64,
        ));
    }
    vec![Section {
        title: "Runaway guard".into(),
        note: "each part off until you turn it on".into(),
        rows,
        ..Section::default()
    }]
}

/// A harness's own row for it, on its page, where it can.
pub fn cache_harness_section(m: &Model, kind: &str) -> Option<Section> {
    if !m.reg.supports(kind, rush_core::adapter::Feature::KeepWarm) {
        return None;
    }
    let ck = &config(m).cache_keep;
    let v = ck
        .harnesses
        .get(kind)
        .map_or("", |on| if *on { "on" } else { "off" });
    let k = kind.to_string();
    let mut row = choice(
        "Keep its cache warm",
        v,
        "Whether this harness's idle sessions, and its waiting subagents, have their prompt cache kept warm, apart from Settings › Usage › Prompt cache.",
        &[
            ("", "as Settings › Usage says."),
            ("on", "kept, whatever Usage says."),
            ("off", "never kept, nor its subagents'."),
        ],
        set(move |m, v| {
            let h = &mut config_mut(m).cache_keep.harnesses;
            match v {
                "on" => h.insert(k.clone(), true),
                "off" => h.insert(k.clone(), false),
                _ => h.remove(&k),
            };
        }),
    );
    row.unset = if ck.on {
        "as Usage: on"
    } else {
        "as Usage: off"
    }
    .into();
    Some(Section::new("Prompt cache", vec![row]))
}

/// The harnesses that can say how long their subagents' prompt cache
/// lives (Feature::SubagentCache), by kind.
pub fn subagent_cache_kinds(m: &Model) -> Vec<String> {
    super::harnesses::catalog_harnesses()
        .into_iter()
        .filter(|k| {
            m.reg
                .supports(k, rush_core::adapter::Feature::SubagentCache)
        })
        .collect()
}

/// How long harness kind's subagents' prompt cache lives, as a row
/// labelled label: on its own page, and on Usage. One stored value
/// (config.subagentCache); the host reads it as a session starts, so a
/// change reaches sessions started or moved onto a new build after it.
pub fn subagent_cache_row(m: &Model, kind: &str, label: &str) -> Setting {
    let v = config(m)
        .subagent_cache
        .get(kind)
        .map_or("", String::as_str)
        .to_string();
    let k = kind.to_string();
    let mut row = choice(
        label,
        &v,
        "How long the prompt cache lives for everything outside the main conversation: subagents, workflows, background and helper requests. A request after it lapses pays to write the whole context again (1.25× the input price against 0.1× for a read); a 1-hour cache costs more to write (2×) but lapses less. The Overview's Cache section and the Projects list show what lapses cost and what an hour would have cost or saved. Applies to sessions started, or moved onto a new build, after the change; one running keeps what it started with. Sets CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL where rush starts Claude Code (2.1.316 or later).",
        &[
            ("", "automatic: sets nothing, Claude Code's own: 5 minutes."),
            ("5m", "5 minutes: a write costs 1.25×."),
            ("1h", "1 hour: a write costs 2×, and it lapses far less."),
        ],
        set(move |m, v| {
            let h = &mut config_mut(m).subagent_cache;
            match v {
                "5m" | "1h" => h.insert(k.clone(), v.to_string()),
                _ => h.remove(&k),
            };
        }),
    );
    row.unset = "automatic".into();
    row
}

/// A harness's "Subagents' cache" row, on its page, where it can say how
/// long its subagents' prompt cache lives (Feature::SubagentCache).
pub fn subagent_cache_section(m: &Model, kind: &str) -> Option<Section> {
    if !m
        .reg
        .supports(kind, rush_core::adapter::Feature::SubagentCache)
    {
        return None;
    }
    Some(Section::new(
        "Subagents' cache",
        vec![subagent_cache_row(m, kind, "Subagents' cache")],
    ))
}
