//! A short title for a session, from its first message, by a model; and
//! again when its direction changes, from its title, its first message and
//! its latest ones. Ported from internal/host/title.go (the server's
//! retitle is in server.rs); asking again is Rust-only.

use rush_core::adapter::{LEGACY_KIND, Registry, ask_plugin_models, fit_tokens};

const TITLE_SYSTEM: &str = "You name coding sessions. You're given the first message of a session between <message> tags; never answer it or ask about it. Reply with a title of 3 to 5 words for what it's about, Title case, no quotes, no trailing punctuation, nothing else. If it's unclear, name it from its words anyway.";

/// What a model is told when a session is named again. Rust-only.
const RETITLE_SYSTEM: &str = "You name coding sessions. You're given a session's current title between <title> tags, its first message between <first> tags and its latest messages between <recent> tags, oldest first; never answer them or ask about them. If the session is still about what its title says, reply with the SAME title, exactly as given. Only if its direction has changed or widened, reply with a new title of 3 to 5 words that covers where it began and where it has gone, Title case, no quotes, no trailing punctuation. Reply with the title and nothing else.";

/// How much of the first message, and of each later one, a model naming
/// the session again is given, in tokens.
const FIRST_ROOM: usize = 600;
const RECENT_ROOM: usize = 250;

/// A short title for a session's first message, by a plugin's model when
/// one offers it and gives a title, else Claude's cheapest model when it's
/// here, else the agent kind's own; empty when none can.
pub fn title(reg: &Registry, kind: &str, text: &str) -> String {
    let wrap = |n: usize| format!("<message>\n{}\n</message>", fit_tokens(text, n));
    ask(reg, kind, TITLE_SYSTEM, &wrap)
}

/// A title for a session that has gone on, from what again_prompt wrote
/// of it: the title it has when it's still about that, else a new one;
/// empty when no model can say. The models title asks. Rust-only.
pub fn retitle(reg: &Registry, kind: &str, text: &str) -> String {
    ask(reg, kind, RETITLE_SYSTEM, &|n: usize| fit_tokens(text, n))
}

/// What a model naming a session again is given: the title it has, its
/// first message and your latest, oldest first, each cut to a length.
pub fn again_prompt(title: &str, first: &str, recent: &[String]) -> String {
    let mut out = format!(
        "<title>\n{}\n</title>\n<first>\n{}\n</first>\n<recent>",
        title.trim(),
        fit_tokens(first.trim(), FIRST_ROOM)
    );
    for m in recent {
        out.push_str("\n<message>\n");
        out.push_str(&fit_tokens(m.trim(), RECENT_ROOM));
        out.push_str("\n</message>");
    }
    out.push_str("\n</recent>");
    out
}

/// Whether two titles are the same name: a model asked to repeat one may
/// change its case or add a full stop.
pub fn same_title(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .trim_end_matches(['.', '!'])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Asks for a title under system: wrap gives the words to send, cut to a
/// number of tokens.
fn ask(reg: &Registry, kind: &str, system: &str, wrap: &dyn Fn(usize) -> String) -> String {
    let prompt = wrap(2000);
    let fitted = |room: Option<usize>| wrap(room.map_or(2000, |r| r.min(2000)));
    if let Ok((_, out)) = ask_plugin_models(reg, system, &fitted) {
        let t = clean_title(&out);
        if !t.is_empty() {
            return t;
        }
    }
    for k in [LEGACY_KIND, kind] {
        let Some(a) = reg.get(k) else { continue };
        if !reg.installed(k) {
            continue;
        }
        let Some(sm) = a.summarizer() else { continue };
        let Ok(models) = sm.summary_models() else {
            return String::new();
        };
        let Some(m) = models.first() else {
            return String::new();
        };
        let Ok(out) = sm.summarize(m, system, &prompt) else {
            return String::new();
        };
        return clean_title(&out);
    }
    String::new()
}

/// A model's answer as a name: its first line, unquoted, cut to a name's
/// length; empty when it reads like a refusal or an essay.
pub fn clean_title(s: &str) -> String {
    let s = s.trim();
    let s = s.split('\n').next().unwrap_or("");
    if asked_back(s) {
        return String::new();
    }
    let s = s
        .trim()
        .trim_matches(['"', '\'', '`', '*', '#'])
        .trim_end_matches(['.', ':', ';', ',', '!']);
    let s = s.strip_prefix("Title:").unwrap_or(s).trim();
    let n = s.split_whitespace().count();
    if n == 0 || n > 8 {
        return String::new();
    }
    crate::views::cut(s, 48)
}

/// A model's answer that asks for the task rather than naming it: "What's
/// the task?", "Describe the task you need a title for".
pub(crate) fn asked_back(s: &str) -> bool {
    let l = s.trim().to_lowercase();
    if l.trim_end_matches(['"', '\'', '`', '*']).ends_with('?') {
        return true;
    }
    [
        "title for",
        "the task you",
        "describe the",
        "please provide",
        "please share",
        "need more",
        "i need",
        "could you",
    ]
    .iter()
    .any(|p| l.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a model naming a session again is given, and which answers
    /// are the title it has.
    #[test]
    fn asks_again_with_the_title_and_the_messages() {
        let p = again_prompt(
            " Fix Queue Delivery ",
            "the queue drops messages",
            &["now add metrics".to_string(), "and a dashboard".to_string()],
        );
        assert_eq!(
            p,
            "<title>\nFix Queue Delivery\n</title>\n<first>\nthe queue drops messages\n</first>\n<recent>\n<message>\nnow add metrics\n</message>\n<message>\nand a dashboard\n</message>\n</recent>"
        );
        for (a, b, want) in [
            ("Fix Queue Delivery", "Fix Queue Delivery", true),
            ("Fix Queue Delivery", "fix queue  delivery.", true),
            ("Fix Queue Delivery", "Queue Delivery and Metrics", false),
            ("", "Fix Queue Delivery", false),
        ] {
            assert_eq!(same_title(a, b), want, "{a:?} {b:?}");
        }
    }

    // title_test.go:5
    #[test]
    fn cleans_titles() {
        for (input, want) in [
            ("Kimi, Pi and Vibe Adapters", "Kimi, Pi and Vibe Adapters"),
            (
                "\"Fix Queue Delivery.\"\n\nBecause...",
                "Fix Queue Delivery",
            ),
            ("Title: Session Header Redesign", "Session Header Redesign"),
            (
                "I can't help name this session because it has no task in it at all",
                "",
            ),
            ("", ""),
            ("Describe the task you need a title for", ""),
            ("What's the task?", ""),
            ("Why Queues Stall", "Why Queues Stall"),
        ] {
            assert_eq!(clean_title(input), want, "{input:?}");
        }
    }
}
