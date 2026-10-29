//! A short title for a session, from its first message, by a model.
//! Ported from internal/host/title.go (the server's retitle is in
//! server.rs).

use rush_core::adapter::{LEGACY_KIND, Registry, fit_tokens};

const TITLE_SYSTEM: &str = "You name coding sessions. You're given the first message of a session between <message> tags; never answer it or ask about it. Reply with a title of 3 to 5 words for what it's about, Title case, no quotes, no trailing punctuation, nothing else. If it's unclear, name it from its words anyway.";

/// A short title for a session's first message, by Claude's cheapest model
/// when it's here, else the agent kind's own; empty when neither can.
pub fn title(reg: &Registry, kind: &str, text: &str) -> String {
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
        let Ok(out) = sm.summarize(
            m,
            TITLE_SYSTEM,
            &format!("<message>\n{}\n</message>", fit_tokens(text, 2000)),
        ) else {
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
fn asked_back(s: &str) -> bool {
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

    // title_test.go:5
    #[test]
    fn cleans_titles() {
        for (input, want) in [
            ("Kimi, Pi and Vibe Adapters", "Kimi, Pi and Vibe Adapters"),
            ("\"Fix Queue Delivery.\"\n\nBecause...", "Fix Queue Delivery"),
            ("Title: Session Header Redesign", "Session Header Redesign"),
            ("I can't help name this session because it has no task in it at all", ""),
            ("", ""),
            ("Describe the task you need a title for", ""),
            ("What's the task?", ""),
            ("Why Queues Stall", "Why Queues Stall"),
        ] {
            assert_eq!(clean_title(input), want, "{input:?}");
        }
    }
}
