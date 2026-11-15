//! The tags a message to a conversation starts with: `@kimi @sol what do
//! you think?` asks Kimi and Sol. A tag is letters, digits, - and _; one
//! with a / or a . in it is a file (`@src/main.rs`), and one inside a word
//! an address, so neither tags anyone. Which agent a tag names is the
//! registry's to say (ask::route). Rust-only.

/// The tags text starts with, as typed and without their @, and the rest
/// of it after them; None when it starts with none.
pub fn leading(text: &str) -> Option<(Vec<String>, String)> {
    let mut rest = text.trim_start();
    let mut tags = Vec::new();
    while let Some(after) = rest.strip_prefix('@') {
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let tag = &after[..end];
        if tag.is_empty()
            || !tag
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            break;
        }
        tags.push(tag.to_string());
        rest = after[end..].trim_start();
    }
    if tags.is_empty() {
        return None;
    }
    Some((tags, rest.to_string()))
}

/// The rest's first word, and what follows it.
pub fn first_word(rest: &str) -> (&str, &str) {
    let rest = rest.trim_start();
    match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim_start()),
        None => (rest, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_at_the_start() {
        type Case<'a> = (&'a str, Option<(Vec<&'a str>, &'a str)>);
        let cases: Vec<Case> = vec![
            ("hello", None),
            ("", None),
            (
                "@kimi does this look safe?",
                Some((vec!["kimi"], "does this look safe?")),
            ),
            ("  @kimi   look", Some((vec!["kimi"], "look"))),
            (
                "@kimi @sol what do you think?",
                Some((vec!["kimi", "sol"], "what do you think?")),
            ),
            (
                "@claude haiku fix it",
                Some((vec!["claude"], "haiku fix it")),
            ),
            ("@kimi", Some((vec!["kimi"], ""))),
            ("@gpt-6_1 hi", Some((vec!["gpt-6_1"], "hi"))),
            ("@src/main.rs is broken", None),
            ("@README.md", None),
            (
                "@kimi @src/a.rs look",
                Some((vec!["kimi"], "@src/a.rs look")),
            ),
            ("mail a@b.c", None),
            ("@ kimi", None),
        ];
        for (text, want) in cases {
            let want =
                want.map(|(t, r)| (t.into_iter().map(String::from).collect(), r.to_string()));
            assert_eq!(leading(text), want, "{text:?}");
        }
    }

    #[test]
    fn a_first_word() {
        for (s, want) in [
            ("haiku fix it", ("haiku", "fix it")),
            ("  haiku", ("haiku", "")),
            ("", ("", "")),
        ] {
            assert_eq!(first_word(s), want, "{s:?}");
        }
    }
}
