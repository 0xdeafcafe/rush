//! Settings › Usage's Emeritus (Rust-only, docs/emeritus.md): a session
//! near the end of its context hands its task over to a fresh one on a
//! baton it writes, and retires. Off until turned on; what it does is
//! rush_host::emeritus's, and the numbers it gives are the lab's.

use rush_state::config::{EMERITUS_DEPTH, EMERITUS_PERCENT, EMERITUS_TOKENS};

use super::form::{Section, choice, set};
use super::guard::numbers;
use super::paint::on_off;
use super::{config, config_mut};
use crate::model::Model;

/// The section: the switch, and under it, once on, where it hands over
/// and how many times in a row.
pub fn sections(m: &Model) -> Vec<Section> {
    let e = config(m).emeritus.clone();
    let mut rows = vec![choice(
        "Hand over near the end of context",
        on_off(e.on),
        "Past a few hundred thousand tokens every request re-reads the whole context: at 300k on Opus that's about $0.06 a call before any work. With this on, rush asks a session past the point below, once, at a turn's end while its prompt cache is still warm, to hand its task over: it writes a baton (the whole goal, what matters, what failed and why, the tests that pin text, the files that matter, the next step), rush adds the branch, HEAD and git status, starts a fresh agent on it in the same folder, model, mode and account, and the old session retires for good, its conversation kept to read. It costs the baton's turn, about $0.16 with the cache warm; a cold one wrote $2.33 in the lab, so it's never asked cold. In rush's lab at ~300k, baton and successor came to about a third less than carrying on ($1.94 against $2.85 for the same follow-up), starting the successor at 22 to 36k, and passed 10 of 11 hidden checks to carrying on's 11. An agent nearly done can say so and carry on. What you type to a retired session isn't passed on: its box opens its successor. Every harness rush hosts; Claude Code's and Codex's own in-process subagents are out of reach. Each handover is in the Efficiency place's events. Reaches a session that's running when its agent next starts (woken, resumed, or rush updated): its tools aren't changed while it runs, as that would write its whole prompt cache again, so until then it isn't asked.",
        &[
            (
                "off",
                "sessions carry on, or compact, however long they get.",
            ),
            (
                "on",
                "a session past the point below is asked to hand over to a fresh one.",
            ),
        ],
        set(|m, v| config_mut(m).emeritus.on = v == "on"),
    )];
    if e.on {
        rows.push(numbers(
            "  past",
            e.tokens as f64,
            "The context, in tokens, past which a session is asked to hand over. The lab's 300k is where each call's re-read had long been the larger part of what a turn cost; lower hands over sooner, for more batons.",
            &[0.0, 150_000.0, 200_000.0, 400_000.0, 500_000.0],
            &format!("{}k tokens", EMERITUS_TOKENS / 1000),
            &|x| format!("{:.0}k tokens", x / 1000.0),
            |m, v| config_mut(m).emeritus.tokens = v as i64,
        ));
        rows.push(numbers(
            "  or past",
            e.percent as f64,
            "The share of the model's window past which it's asked, when that comes first: 70% of a 200k window is 140k. A 1M window goes by the tokens above.",
            &[0.0, 50.0, 60.0, 80.0, 90.0],
            &format!("{EMERITUS_PERCENT}% of its window"),
            &|x| format!("{x:.0}% of its window"),
            |m, v| config_mut(m).emeritus.percent = v as i64,
        ));
        rows.push(numbers(
            "  handovers in a row",
            e.depth as f64,
            "The most handovers one task goes through. Each baton keeps most of what the agent knew, not all (the lab's successors came within one hidden check of the original), so a long chain drifts. A session that many handovers down isn't asked again: its header says so, and it carries on as any session does.",
            &[0.0, 1.0, 2.0, 5.0, 10.0],
            &format!("{EMERITUS_DEPTH}"),
            &|x| format!("{x:.0}"),
            |m, v| config_mut(m).emeritus.depth = v as i64,
        ));
    }
    vec![Section {
        title: "Emeritus".into(),
        note: "off until you turn it on; reaches a running agent at its next start".into(),
        rows,
        ..Section::default()
    }]
}
