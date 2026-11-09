//! Emeritus's words in a conversation (docs/emeritus.md): what rush sends
//! a session as it nudges it to hand over, tells it of a handover that
//! didn't happen, and starts its successor on, read back so the view
//! draws them as rush's own rather than as messages of yours. The host
//! (rush_host::emeritus) writes them with these very prefixes.
//! Rust-only: Go rush has no Emeritus.

/// What every one of rush's Emeritus notes to an agent begins with.
pub const NOTE_START: &str = "rush (Emeritus): ";

/// What every nudge begins with.
pub const NUDGE_START: &str = "rush (Emeritus): your context is at ";

/// What the note of a handover that didn't happen begins with.
pub const ROLLED_BACK_START: &str = "rush (Emeritus): your handover didn't happen: your successor ";

/// What the note of a retire waiting on background tasks begins with.
pub const WAITS_START: &str = "rush (Emeritus): your retire waits for your background tasks: ";

/// The note put ahead of what you sent a session whose retire it called
/// off.
pub const CALLED_OFF: &str =
    "rush (Emeritus): your retire was called off because the user wrote to you; carry on.";

/// What a successor's first message begins with.
pub const TAKE_OVER_START: &str = "You take over this task from ";

/// The rush tool a session calls to retire after its turn.
pub const RETIRE: &str = "retire";

/// One of rush's Emeritus notes as a line for you: "context at 312k, past
/// 300k · asked to hand over". None for any other message, and for the
/// note ahead of a message of yours (see [`called_off`]).
pub fn said(prompt: &str) -> Option<String> {
    let p = prompt.trim_start();
    if let Some(rest) = p.strip_prefix(NUDGE_START) {
        // "312k tokens, past the 300k where …"
        let ctx = rest.split(' ').next().unwrap_or("");
        let past = rest
            .split_once(", past the ")
            .and_then(|(_, r)| r.split(' ').next())
            .unwrap_or("");
        return Some(if ctx.is_empty() || past.is_empty() {
            "asked to hand over (Emeritus)".into()
        } else {
            format!("context at {ctx}, past {past} · asked to hand over (Emeritus)")
        });
    }
    if let Some(rest) = p.strip_prefix(ROLLED_BACK_START) {
        let why = rest
            .split_once(". You aren't retired")
            .map_or(rest, |(w, _)| w);
        return Some(format!(
            "handover rolled back: its successor {} · carrying on",
            crate::text::first_line(why)
        ));
    }
    if let Some(rest) = p.strip_prefix(WAITS_START) {
        let n = rest.split(' ').next().unwrap_or("");
        return Some(format!(
            "retire waits for its background tasks ({n} running)"
        ));
    }
    None
}

/// What you sent a session whose retire it called off, without rush's
/// note ahead of it; None when the note isn't there.
pub fn called_off(prompt: &str) -> Option<&str> {
    prompt
        .trim_start()
        .strip_prefix(CALLED_OFF)
        .map(str::trim_start)
}

/// A successor's first message, read back: who it took over from, as
/// the message names it ("\"fix the flaky test\" (rush session a1b2)"),
/// and the rest of it, the baton and what came for its predecessor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TookOver {
    pub from: String,
    pub baton: String,
}

/// prompt as a successor's first message; None for any other.
pub fn took_over(prompt: &str) -> Option<TookOver> {
    let rest = prompt.trim_start().strip_prefix(TAKE_OVER_START)?;
    let (from, _) = rest.split_once(", an agent that handed it over")?;
    let baton = rest.split_once("\n\n").map_or("", |(_, b)| b);
    Some(TookOver {
        from: from.trim().to_string(),
        baton: baton.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_as_lines() {
        let cases = [
            (
                "nudge",
                "rush (Emeritus): your context is at 312k tokens, past the 300k where each call's re-read costs more.",
                Some("context at 312k, past 300k · asked to hand over (Emeritus)"),
            ),
            (
                "rolled back",
                "rush (Emeritus): your handover didn't happen: your successor hit a usage limit. You aren't retired: carry on.",
                Some("handover rolled back: its successor hit a usage limit · carrying on"),
            ),
            (
                "waits",
                "rush (Emeritus): your retire waits for your background tasks: 2 still running. Wait for them.",
                Some("retire waits for its background tasks (2 running)"),
            ),
            ("yours", "fix the flaky test", None),
            (
                "called off: yours, with a note",
                "rush (Emeritus): your retire was called off because the user wrote to you; carry on.\n\nstop",
                None,
            ),
        ];
        for (name, p, want) in cases {
            assert_eq!(said(p).as_deref(), want, "{name}");
        }
        assert_eq!(
            called_off(&format!("{CALLED_OFF}\n\nwait, one more thing")),
            Some("wait, one more thing")
        );
        assert_eq!(called_off("wait"), None);
    }

    #[test]
    fn a_successor_read_back() {
        let p = "You take over this task from \"fix it\" (rush session a1b2), an agent that handed it over near the end of its context and has retired. Its baton follows.\n\n## Destination\nGreen CI.\n\n## State (recorded by rush)\n- Branch: main";
        assert_eq!(
            took_over(p),
            Some(TookOver {
                from: "\"fix it\" (rush session a1b2)".into(),
                baton: "## Destination\nGreen CI.\n\n## State (recorded by rush)\n- Branch: main"
                    .into(),
            })
        );
        assert_eq!(took_over("fix it"), None);
    }
}
