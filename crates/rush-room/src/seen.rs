//! What one agent is told of a group chat since its last turn: the
//! others' messages, the tools they ran a line each, and anything you
//! said, the newest kept whole and the oldest left out first past a
//! budget set by the smallest context window in the chat. A room's
//! members are told it each turn.
//! Ported from internal/room/run.go (since, budget).

use crate::room::{Entry, SAY, TOOL, USER};

/// How many bytes of the chat a turn is told at most.
pub const PROMPT_BUDGET: usize = 60_000;

/// What the chat says to one agent since it last spoke.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Since {
    /// What was said, as it reads it: "" when nothing was.
    pub text: String,
    /// You spoke to it by name.
    pub to_you: bool,
    /// You spoke at all.
    pub from_user: bool,
}

/// What entries (the chat since agent me's last turn, its own and those
/// it was handed already left out by the caller or by name) say to me,
/// at most budget bytes of them: see the module's doc.
pub fn since<'a>(entries: impl IntoIterator<Item = &'a Entry>, me: &str, budget: usize) -> Since {
    let mut parts: Vec<String> = Vec::new();
    let (mut to_you, mut from_user) = (false, false);
    for e in entries {
        if e.from == me {
            continue;
        }
        if e.from == USER {
            from_user = true;
            let mut who = "The user".to_string();
            if !e.to.is_empty() {
                who += &format!(" (to {})", e.to);
                to_you = to_you || e.to.to_lowercase() == me.to_lowercase();
            }
            parts.push(format!("\n{who}:\n{}\n", e.text));
        } else if e.kind == TOOL {
            parts.push(format!(
                "  ({} ran a tool: {}{})\n",
                e.from,
                e.text,
                if_set(" — ", &e.detail)
            ));
        } else if e.kind == SAY {
            let mut who = e.from.clone();
            if !e.to.is_empty() {
                who += &format!(" (to {})", e.to);
                to_you = to_you || e.to.to_lowercase() == me.to_lowercase();
            }
            parts.push(format!(
                "\n{who}:\n{}\n{}",
                e.text,
                if_set("READY: ", &e.ready)
            ));
        }
    }
    // The newest is kept whole; the oldest goes first when it's long.
    let mut total = 0;
    let mut from = parts.len();
    while from > 0 && total + parts[from - 1].len() <= budget {
        from -= 1;
        total += parts[from].len();
    }
    if from == parts.len() && from > 0 {
        // even the newest is too long: its tail
        let last = &parts[from - 1];
        let mut cut = last.len().saturating_sub(budget);
        while !last.is_char_boundary(cut) {
            cut += 1; // a character cut in two is left out
        }
        parts[from - 1] = format!("[shortened] {}", &last[cut..]);
        from -= 1;
    }
    let mut text = parts[from..].concat();
    if from > 0 {
        text = format!("(earlier turns left out for length)\n{text}");
    }
    Since {
        text,
        to_you,
        from_user,
    }
}

/// PROMPT_BUDGET, or less where a context window (in tokens; 0 when not
/// known) is small: about a byte and a half per token of it, less 4 KB
/// for the rules, and never under 2 KB.
pub fn budget(contexts: impl IntoIterator<Item = i64>) -> usize {
    let mut b = PROMPT_BUDGET as i64;
    for c in contexts {
        if c > 0 {
            // Saturating: a context read from a file can be anything.
            b = b.min((c.saturating_mul(3) / 2).saturating_sub(4096).max(2048));
        }
    }
    b as usize
}

fn if_set(lead: &str, s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    format!("{lead}{s}")
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
    fn what_one_agent_reads() {
        struct Case {
            name: &'static str,
            log: Vec<Entry>,
            me: &'static str,
            budget: usize,
            want: Since,
        }
        let cases = vec![
            Case {
                name: "nothing said",
                log: vec![],
                me: "Kimi",
                budget: 100,
                want: Since::default(),
            },
            Case {
                name: "its own words left out",
                log: vec![e("Kimi", SAY, "mine", "")],
                me: "Kimi",
                budget: 100,
                want: Since::default(),
            },
            Case {
                name: "you, to it by name",
                log: vec![e(USER, SAY, "look at this", "kimi")],
                me: "Kimi",
                budget: 100,
                want: Since {
                    text: "\nThe user (to kimi):\nlook at this\n".into(),
                    to_you: true,
                    from_user: true,
                },
            },
            Case {
                name: "another agent's words and its tool, a line",
                log: vec![
                    e("Claude", TOOL, "editing migrate.rs", ""),
                    e("Claude", SAY, "done", "Kimi"),
                ],
                me: "Kimi",
                budget: 100,
                want: Since {
                    text: "  (Claude ran a tool: editing migrate.rs)\n\nClaude (to Kimi):\ndone\n"
                        .into(),
                    to_you: true,
                    from_user: false,
                },
            },
            Case {
                name: "the oldest left out first",
                log: vec![e("Sol", SAY, "old old old", ""), e("Sol", SAY, "new", "")],
                me: "Kimi",
                budget: 12,
                want: Since {
                    text: "(earlier turns left out for length)\n\nSol:\nnew\n".into(),
                    to_you: false,
                    from_user: false,
                },
            },
            Case {
                name: "the newest alone too long: its tail",
                log: vec![e("Sol", SAY, "0123456789", "")],
                me: "Kimi",
                budget: 4,
                want: Since {
                    text: "[shortened] 789\n".into(),
                    to_you: false,
                    from_user: false,
                },
            },
        ];
        for c in cases {
            assert_eq!(since(&c.log, c.me, c.budget), c.want, "{}", c.name);
        }
    }

    #[test]
    fn budget_by_the_smallest_window() {
        for (contexts, want) in [
            (vec![], PROMPT_BUDGET),
            (vec![0, 0], PROMPT_BUDGET),
            (vec![1_000_000], PROMPT_BUDGET),
            (vec![32_000, 200_000], 32_000 * 3 / 2 - 4096),
            (vec![1_000], 2048),
            (vec![i64::MAX], PROMPT_BUDGET),
        ] {
            assert_eq!(budget(contexts.clone()), want, "{contexts:?}");
        }
    }
}
