//! A session told for a hand-off: what another agent needs to carry it on.
//! After Go's internal/agent/handoff.go.
//!
//! Go words each step with tool.Doing; that lives beside the Claude
//! adapter in rush-claude, so the caller passes it in as `doing`.

use super::{Input, Kind};
use crate::agent::{PreviewLine, Todo};
use crate::tool;

/// A session told for a hand-off.
#[derive(Clone, Debug, Default)]
pub struct Conversation {
    /// The agent it ran on.
    pub from: Kind,
    /// What it's called.
    pub name: String,
    pub cwd: String,
    /// The message that started it.
    pub first: String,
    /// The most recent calls it made, oldest first.
    pub steps: Vec<Step>,
    /// Older calls left out of steps.
    pub earlier_steps: usize,
    /// The files it changed.
    pub changed: Vec<String>,
    /// Its last turns: your messages (user) and its answers (assistant).
    pub recent: Vec<PreviewLine>,
    /// Every turn, as for recent, with its steps as → lines among its words.
    pub history: Vec<PreviewLine>,
    /// Its todo list as it stands.
    pub todos: Vec<Todo>,
}

/// One call a conversation made, and whether it failed.
#[derive(Clone, Debug, Default)]
pub struct Step {
    pub call: tool::Call,
    pub failed: bool,
}

// How much of a conversation a hand-off carries: enough to go on with,
// short enough to leave the new session room.
const HANDOFF_DONE: usize = 30;
const HANDOFF_CHANGED: usize = 40;
const HANDOFF_TURNS: usize = 6;
const HANDOFF_TEXT: usize = 2000;
const HANDOFF_FIRST: usize = 4000;

/// The first message of a new session, on another agent, that carries c
/// on: where it ran, how it started, what it did, where it left off and
/// what's still to do. `from` is the agent's name (Adapter::name), or its
/// kind when it isn't registered.
pub fn handoff(c: &Conversation, from: &str, doing: impl Fn(&tool::Call) -> String) -> Input {
    let mut b = String::new();
    let mut title = String::new();
    // A name that's only the first message's opening words says nothing the
    // message doesn't, cut mid-word.
    if !c.name.is_empty()
        && !c
            .first
            .trim()
            .to_lowercase()
            .starts_with(&c.name.strip_suffix('…').unwrap_or(&c.name).to_lowercase())
    {
        title = format!(" ({})", c.name);
    }
    b += &format!(
        "You're taking over a conversation{title} that ran in {from} and can't go on there. Here is where it stands; carry it on from here.\n"
    );
    if !c.cwd.is_empty() {
        b += &format!("\nIt worked in {}.\n", c.cwd);
    }
    let first = clip(&c.first, HANDOFF_FIRST);
    if !first.is_empty() {
        b += "\nIt started with this message:\n";
        quote(&mut b, &first);
    }
    let mut done = done_words(&c.steps, &doing);
    if !done.is_empty() || c.earlier_steps > 0 {
        b += "\nWhat it did:\n";
        if c.earlier_steps > 0 {
            b += &format!("- ({} earlier tool calls omitted)\n", c.earlier_steps);
        }
        if done.len() > HANDOFF_DONE {
            let n = done.len() - HANDOFF_DONE;
            b += &format!("- ({n} earlier steps)\n");
            done.drain(..n);
        }
        for d in &done {
            b += &format!("- {}\n", one_line(d));
        }
    }
    if !c.changed.is_empty() {
        let mut changed = &c.changed[..];
        let mut more = String::new();
        if changed.len() > HANDOFF_CHANGED {
            more = format!(" and {} more", changed.len() - HANDOFF_CHANGED);
            changed = &changed[..HANDOFF_CHANGED];
        }
        b += &format!("\nFiles it changed: {}{more}.\n", changed.join(", "));
    }
    if !c.recent.is_empty() {
        b += "\nWhere it left off:\n";
        let recent = &c.recent[c.recent.len().saturating_sub(HANDOFF_TURNS)..];
        for l in recent {
            let who = if l.role == "user" { "The user" } else { from };
            b += &format!("\n{who}:\n");
            quote(&mut b, &clip(&l.text, HANDOFF_TEXT));
        }
    }
    let open: Vec<&Todo> = c.todos.iter().filter(|t| !t.done).collect();
    if !open.is_empty() {
        b += "\nStill to do:\n";
        for t in open {
            let mark = if t.started { "[in progress]" } else { "[ ]" };
            b += &format!("- {mark} {}\n", one_line(&t.label));
        }
    }
    b += "\nThe files are as it left them: look at them before changing anything again. Carry on with what the user asked.";
    Input {
        text: b,
        images: Vec::new(),
    }
}

/// Steps in words, a run of the same said once.
fn done_words(steps: &[Step], doing: &impl Fn(&tool::Call) -> String) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in steps {
        let mut d = doing(&s.call);
        if d.is_empty() {
            continue;
        }
        if s.failed {
            d += " (failed)";
        }
        if out.last() != Some(&d) {
            out.push(d);
        }
    }
    out
}

/// Writes s as a quoted block.
fn quote(b: &mut String, s: &str) {
    for l in s.split('\n') {
        b.push_str("> ");
        b.push_str(l);
        b.push('\n');
    }
}

/// s, trimmed, cut to n bytes at a line or word where it can be.
fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut cut = &s[..end];
    if let Some(i) = cut.rfind(['\n', ' '])
        && i > n / 2
    {
        cut = &cut[..i];
    }
    format!("{cut} …")
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a session started with c as its own history (carry) is told beside
/// it: where it came from, and how its steps read. `from` is as for
/// [`handoff`].
pub fn carried(from: &str) -> String {
    format!(
        "This conversation began in {from} and was moved to you, with its history as your own. \
         The steps it took there are the lines starting → in its answers, in its own tools' words, not yours. \
         The files are as it left them: look at them before changing anything again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shell calls in words as tool.Doing gives them: their description,
    /// its first letter lowered.
    fn doing(c: &tool::Call) -> String {
        let d = &c.input.description;
        let mut cs = d.chars();
        cs.next()
            .map(|f| f.to_lowercase().collect::<String>() + cs.as_str())
            .unwrap_or_default()
    }

    // A long conversation is handed on cut down: its latest steps and turns,
    // and a long message clipped.
    #[test]
    fn handoff_keeps_the_latest() {
        let mut c = Conversation {
            from: Kind::from("nosuch"),
            cwd: "/r".into(),
            first: "word ".repeat(2000),
            ..Default::default()
        };
        for i in 0..50 {
            c.steps.push(Step {
                call: tool::Call {
                    kind: tool::Kind::Shell,
                    input: tool::Input {
                        description: format!("step {i}"),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                failed: false,
            });
        }
        for i in 0..10 {
            c.recent.push(PreviewLine {
                role: "user".into(),
                text: format!("turn {i}"),
                ..Default::default()
            });
        }
        let text = handoff(&c, "nosuch", doing).text;
        for want in [
            "ran in nosuch",
            "It worked in /r.",
            "(20 earlier steps)",
            "- step 49",
            "turn 9",
            " …",
        ] {
            assert!(text.contains(want), "lacks {want:?}:\n{text}");
        }
        for not in ["- step 19\n", "turn 3\n"] {
            assert!(!text.contains(not), "keeps {not:?}");
        }
        assert!(text.len() <= 12000, "hand-off is {} bytes", text.len());
    }

    #[test]
    fn handoff_reports_omitted_calls_without_summaries() {
        let c = Conversation {
            earlier_steps: 12,
            ..Default::default()
        };
        assert!(
            handoff(&c, "", doing)
                .text
                .contains("12 earlier tool calls omitted")
        );
    }
}
