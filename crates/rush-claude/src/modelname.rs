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
pub fn model_name(s: &str) -> String {
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
            ("opus", "Opus"),
            ("opus[1m]", "Opus"),
            ("Opus 4.1 (1M context)", "Opus 4.1"),
            ("gpt-5-codex", "gpt-5-codex"),
            ("claude-haiku-4-5-20251001", "Haiku 4.5"),
        ] {
            assert_eq!(model_name(input), want, "{input}");
        }
    }
}
