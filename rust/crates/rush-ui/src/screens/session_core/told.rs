//! A conversation told for another model: the whole of it as plain text
//! for one that summarises it (#compact), its tool steps for one that
//! judges which are done (#compact's pruner). After Go's
//! internal/convo/compactby.go (PlainText :14, PruneSteps :24, Pruned :59,
//! clipText :76, plainText :84, CompactSystem :119, CompactedPrompt :128,
//! PrunedPrompt :135). What a hand-off to another harness carries
//! (internal/convo/handoff.go) is switch.rs's conversation.

use rush_convo::{Item, Session};

/// What a model compacting a session is told.
pub const COMPACT_SYSTEM: &str = "You compact a coding agent's conversation so it can carry on in a fresh one with only your summary.
Write the summary the agent will read as its whole memory of the work. Keep, in this order:
1. What the user asked for, in their words where it matters, and every correction or preference they gave.
2. What was done: files changed and how, commands that mattered, decisions and why.
3. What is in progress right now and exactly what was about to happen next.
4. Open questions, errors not yet fixed, things the user is still waiting on.
Be specific (paths, names, numbers). Leave out chatter and anything finished that no longer matters. Write it as plain markdown, no preamble.";

const COMPACTED_HEAD: &str = "This conversation was compacted by ";
const COMPACTED_MID: &str =
    " to make room. Here is the summary of everything before this point:\n\n";
const COMPACTED_TAIL: &str = "\n\nTake this as what you know of the work so far. Reply only \"ready\" and wait for my next message.";

/// The fresh conversation's first message.
pub fn compacted_prompt(by: &str, summary: &str) -> String {
    format!("{COMPACTED_HEAD}{by}{COMPACTED_MID}{summary}{COMPACTED_TAIL}")
}

/// The fresh conversation's first message when a decision model pruned
/// it: the conversation itself, the steps it judged done cut to their
/// calls.
pub fn pruned_prompt(by: &str, transcript: &str) -> String {
    format!(
        "This conversation was pruned by {by} to make room: tool steps judged finished are cut to their calls, the rest is word for word. Here it is:\n\n{transcript}{COMPACTED_TAIL}"
    )
}

/// The conversation as plain text for another model to summarise: your
/// messages and its words in full, each tool call on a line with the
/// start of what came back. Thinking is left out.
pub fn plain_text(s: &Session) -> String {
    plain(s, |_| (300, 400))
}

/// How many of the latest turns pruning never touches.
const PRUNE_RECENT: usize = 2;

/// What a decision model judges to prune the conversation by: each tool
/// step before the latest turns as a line, and the work it's judged
/// against: your latest ask, its latest words, your asks before.
pub fn prune_steps(s: &Session) -> (String, Vec<String>) {
    let mut asks = Vec::new();
    let mut said = String::new();
    for t in s.turns.iter().rev() {
        if asks.len() >= 3 {
            break;
        }
        if said.is_empty()
            && let Some(text) = t.items.iter().rev().find_map(|it| match it {
                Item::Text { text, .. } => Some(text),
                _ => None,
            })
        {
            said = clip_text(text, 500);
        }
        if !t.prompt.is_empty() {
            asks.push(clip_text(&t.prompt, 500));
        }
    }
    let mut g = String::new();
    for (i, a) in asks.iter().enumerate() {
        g += &format!("User:\n{a}\n\n");
        if i == 0 && !said.is_empty() {
            g += &format!("Assistant, latest:\n{said}\n\n");
        }
    }
    let mut steps = Vec::new();
    let old = s.turns.len().saturating_sub(PRUNE_RECENT);
    for t in &s.turns[..old] {
        for it in &t.items {
            if let Item::Step(id) = it {
                let st = s.step(*id);
                steps.push(format!(
                    "{} {}\n→ {}",
                    st.tool,
                    clip_text(st.input_json(), 400),
                    clip_text(&st.output, 700)
                ));
            }
        }
    }
    (g.trim().to_string(), steps)
}

/// The conversation as plain_text has it, but each step of prune_steps
/// kept (keep[i]) in full and the rest cut to the call alone, and the
/// latest turns' steps in full.
pub fn pruned(s: &Session, keep: &[bool]) -> String {
    let old = s.turns.len() as isize - PRUNE_RECENT as isize;
    let mut n = 0;
    plain(s, |turn| {
        if turn as isize >= old {
            return (2000, 4000);
        }
        let k = keep.get(n).copied().unwrap_or(false);
        n += 1;
        if k { (2000, 4000) } else { (120, 0) }
    })
}

/// t trimmed and cut to n bytes, never mid-character, with " …" when cut.
fn clip_text(t: &str, n: usize) -> String {
    let t = t.trim();
    if t.len() > n {
        let mut i = n;
        while !t.is_char_boundary(i) {
            i -= 1;
        }
        return format!("{} …", &t[..i]);
    }
    t.to_string()
}

/// The conversation with each step's input and output cut to what clip
/// says for its turn (an output of 0 is left out).
fn plain(s: &Session, mut clip: impl FnMut(usize) -> (usize, usize)) -> String {
    let mut b = String::new();
    for (ti, t) in s.turns.iter().enumerate() {
        if !t.prompt.is_empty() {
            b += &format!("\n## User\n{}\n", t.prompt);
        } else if !t.from.is_empty() {
            b += &format!("\n## {}\n", t.from);
        }
        for it in &t.items {
            match it {
                Item::Text { text, .. } => b += &format!("\n## Assistant\n{text}\n"),
                Item::Interject(i) => b += &format!("\n## User (mid-turn)\n{}\n", i.text),
                Item::Compact { summary, .. } => {
                    b += &format!("\n## Summary of the conversation before this\n{summary}\n")
                }
                Item::Step(id) => {
                    let st = s.step(*id);
                    let (inn, out_n) = clip(ti);
                    b += &format!("- {} {}", st.tool, clip_text(st.input_json(), inn));
                    let out = clip_text(&st.output, out_n);
                    if out_n > 0 && !out.is_empty() {
                        b += &format!("\n  → {}", out.replace('\n', "\n    "));
                    } else if out_n == 0 {
                        b += " (output dropped)";
                    }
                    b.push('\n');
                }
                _ => {}
            }
        }
    }
    b.trim().to_string()
}
