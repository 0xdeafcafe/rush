//! The words of a conversation with guests: a guest's brief and each of
//! its turns (what's new in the chat, as rush_room::seen tells it, then
//! who asks), the note atop the lead's next turn, and a guest's reply as
//! its card in the lead's conversation. Pure: the callers read and write
//! the files. Rust-only.

use rush_room::room::{Entry, SAY, TOOL, USER};
use rush_room::seen;

/// What a guest is told before its first turn.
#[derive(Clone, Debug, Default)]
pub struct Brief<'a> {
    /// The guest: "Kimi".
    pub me: &'a str,
    /// The conversation's own agent: "Claude".
    pub lead: &'a str,
    /// Everyone else in the chat besides the lead.
    pub others: &'a [String],
    pub cwd: &'a str,
    /// The end of the conversation before it joined, as plain text; ""
    /// when not known.
    pub before: &'a str,
}

/// The rule every participant is told about changing files.
pub const WRITE_RULE: &str = "Only one participant changes files at a time: rush makes a file change of yours wait while another participant's turn is changing files, and lets it through once theirs ends. Read and run read-only commands freely.";

/// The first part of a guest's first turn: who it is, where, who's
/// there, how to address them, and the rule for changing files.
pub fn brief(b: &Brief) -> String {
    let mut s = format!(
        "You are {me}, a guest in the user's conversation with {lead} (the conversation's own agent), run by rush. The user and the other agents can see everything you write; keep it short and to the point.\n\n",
        me = b.me,
        lead = b.lead,
    );
    s += &format!("In the chat: the user, {}", b.lead);
    for o in b.others {
        if o != b.me {
            s += &format!(", {o}");
        }
    }
    s += &format!(", and you.\nWorking folder: {}\n\n", b.cwd);
    s += &format!(
        "To ask anyone in the chat something (the lead, another guest, or an agent not here yet, who joins), use rush's ask_guest tool with their name; the user tags you with @{}. Answer whoever asked you.\n{WRITE_RULE}\n",
        crate::agentmatch::clean_name(b.me)
    );
    if !b.before.trim().is_empty() {
        s += &format!(
            "\nThe conversation before you joined, its latest part:\n\n{}\n",
            b.before.trim()
        );
    }
    s
}

/// A guest's turn: its brief the first time, what's new in the chat
/// since its last turn (asks to it included), and who it answers. asker
/// is rush_room's USER for you, else the agent's name.
pub fn turn(brief: Option<&str>, since: &seen::Since, first: bool, asker: &str) -> String {
    let mut b = brief.map(str::to_string).unwrap_or_default();
    if !since.text.is_empty() {
        b += if first {
            "\nSo far in the chat:\n"
        } else {
            "\nNew in the chat since your last turn:\n"
        };
        b += &since.text;
    }
    b += "\n";
    if asker == USER {
        b += "The user asked you, by name, the last thing above: answer the user.\n";
    } else {
        b += &format!("{asker} asked you, by name, the last thing above: answer {asker}.\n");
    }
    b
}

/// What the lead is told atop its next turn of what others said since its
/// last: guests' words and tools, your asks to guests. "" when nothing.
pub fn lead_note(es: &[Entry], lead: &str, budget: usize) -> String {
    let others: Vec<&Entry> = es
        .iter()
        .filter(|e| e.from != lead && !(e.from == USER && e.to.is_empty()))
        .filter(|e| e.kind == SAY || e.kind == TOOL)
        .collect();
    if others.is_empty() {
        return String::new();
    }
    let s = seen::since(others, lead, budget);
    format!(
        "Your guests (other agents in this conversation, see docs/guests.md) said this since your last turn; the user saw it as it came:\n{}",
        s.text
    )
}

/// A guest's reply as its card says it: who asked when it wasn't you,
/// its words, and a line of what it did.
pub fn reply(said: &str, did: &[String], asked_by: &str) -> String {
    let mut s = String::new();
    if !asked_by.is_empty() && asked_by != USER {
        s += &format!("(asked by {asked_by}) ");
    }
    s += said.trim();
    if !did.is_empty() {
        s += &format!("\n\ndid: {}", did.join(" · "));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(from: &str, kind: &str, text: &str, to: &str) -> Entry {
        Entry {
            from: from.into(),
            kind: kind.into(),
            text: text.into(),
            to: to.into(),
            ..Entry::default()
        }
    }

    #[test]
    fn a_brief_names_the_chat_and_the_rule() {
        let others = vec!["Kimi".to_string(), "Sol".to_string()];
        let b = brief(&Brief {
            me: "Kimi",
            lead: "Claude",
            others: &others,
            cwd: "/src/app",
            before: "you: fix the migration",
        });
        for want in [
            "You are Kimi, a guest in the user's conversation with Claude",
            "In the chat: the user, Claude, Sol, and you.",
            "Working folder: /src/app",
            "ask_guest",
            "@kimi",
            WRITE_RULE,
            "before you joined, its latest part:\n\nyou: fix the migration",
        ] {
            assert!(b.contains(want), "{want:?} in {b}");
        }
        let b = brief(&Brief {
            me: "Kimi",
            lead: "Claude",
            ..Brief::default()
        });
        assert!(!b.contains("before you joined"), "{b}");
    }

    #[test]
    fn what_a_guest_turn_says() {
        let since = seen::Since {
            text: "\nThe user (to Kimi):\nlook\n".into(),
            to_you: true,
            from_user: true,
        };
        let cases = [
            (
                Some("BRIEF\n"),
                true,
                USER,
                "BRIEF\n\nSo far in the chat:\n\nThe user (to Kimi):\nlook\n\nThe user asked you, by name, the last thing above: answer the user.\n",
            ),
            (
                None,
                false,
                "Claude",
                "\nNew in the chat since your last turn:\n\nThe user (to Kimi):\nlook\n\nClaude asked you, by name, the last thing above: answer Claude.\n",
            ),
        ];
        for (brief, first, asker, want) in cases {
            assert_eq!(turn(brief, &since, first, asker), want, "{asker}");
        }
    }

    #[test]
    fn the_lead_hears_others_only() {
        let es = vec![
            e(USER, SAY, "to the lead", ""),
            e("Claude", SAY, "the lead's own", ""),
            e(USER, SAY, "is this safe?", "Kimi"),
            e("Kimi", TOOL, "reading migrate.rs", ""),
            e("Kimi", SAY, "batch it", "you"),
            e("Kimi", rush_room::room::INFO, "noise", ""),
        ];
        let n = lead_note(&es, "Claude", 10_000);
        assert!(n.starts_with("Your guests"), "{n}");
        for want in [
            "The user (to Kimi):\nis this safe?",
            "(Kimi ran a tool: reading migrate.rs)",
            "Kimi (to you):\nbatch it",
        ] {
            assert!(n.contains(want), "{want:?} in {n}");
        }
        for not in ["to the lead", "the lead's own", "noise"] {
            assert!(!n.contains(not), "{not:?} in {n}");
        }
        assert_eq!(lead_note(&es[..2], "Claude", 10_000), "");
    }

    #[test]
    fn a_reply_card() {
        let did = [
            "editing migrate.rs".to_string(),
            "running cargo test".to_string(),
        ];
        for (said, did, by, want) in [
            ("batch it", &did[..0], USER, "batch it"),
            (
                " ok ",
                &did[..],
                USER,
                "ok\n\ndid: editing migrate.rs · running cargo test",
            ),
            (
                "214 passed",
                &did[1..],
                "Claude",
                "(asked by Claude) 214 passed\n\ndid: running cargo test",
            ),
        ] {
            assert_eq!(reply(said, did, by), want);
        }
    }
}
