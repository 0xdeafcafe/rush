//! A Claude model's id as people say it. Port of
//! internal/adapters/claude/claude/modelname.go.

use std::sync::LazyLock;

use regex::Regex;

static MODEL_PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(.*\)$").unwrap());

static MODEL_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?(\[1m\])?$").unwrap()
});

/// How a model is shown: "Opus 5.5" for claude-opus-5-5[1m], a display
/// name without its "(1M context)".
///
/// Not Go's: an alias Claude Code takes for --model ("fable",
/// "opus[1m]") is named as the model it runs now, "Fable 5.2", so a
/// session's header, the model picker and its usage say one name for one
/// model. Go said "Fable" for the alias and "Fable 5.2" for the id.
pub fn model_name(s: &str) -> String {
    let full = crate::pricing::alias(s.strip_suffix("[1m]").unwrap_or(s));
    let s = full.unwrap_or(s);
    if let Some(m) = MODEL_ID.captures(s) {
        let mut v = m[2].to_string();
        if let Some(minor) = m.get(3) {
            v.push('.');
            v.push_str(minor.as_str());
        }
        return format!("{} {v}", upper_first(&m[1]));
    }
    let mut s = MODEL_PAREN.replace(s, "").into_owned();
    if let Some(a) = s.strip_suffix("[1m]")
        && !a.is_empty()
        && !a.contains('-')
    {
        s = a.to_string(); // opus[1m] → Opus, as claude-opus-5-5[1m] is Opus 5.5
    }
    if !s.is_empty() && s.to_lowercase() == s && !s.contains('-') {
        return upper_first(&s); // opus → Opus
    }
    s
}

/// s with its first letter (an ASCII one, as Go's [:1] takes a byte) in
/// capitals.
fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // modelname_test.go
    #[test]
    fn names() {
        for (input, want) in [
            ("claude-opus-5-5[1m]", "Opus 5.5"),
            ("claude-sonnet-5", "Sonnet 5"),
            // An alias is the model it runs now (not Go's "Opus").
            ("opus", "Opus 5.5"),
            ("opus[1m]", "Opus 5.5"),
            ("fable", "Fable 5.2"),
            ("claude-fable-5-2", "Fable 5.2"),
            ("best", "Best"),
            ("Opus 4.1 (1M context)", "Opus 4.1"),
            ("gpt-5-codex", "gpt-5-codex"),
            ("claude-haiku-4-5-20251001", "Haiku 4.5"),
        ] {
            assert_eq!(model_name(input), want, "{input}");
        }
    }
}
