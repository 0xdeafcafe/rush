//! A session's name from its first message, and how queued messages go to
//! the agent. Ported from internal/host/name.go, queue.go and
//! host.go:1546-1566 (queueCut, IsCommand).

use std::sync::LazyLock;

use regex::Regex;

static PASTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<pasted_content[^>]*>.*?</pasted_content[^>]*>").unwrap());
static PASTE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?pasted_content[^>]*>").unwrap());
static IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[Image #\d+\]").unwrap());

/// A session's name from its first message, until it names itself: the
/// first few words, with what was pasted and images left out, unless what
/// was pasted is all there is.
pub fn name_from(text: &str) -> String {
    let a = IMAGE.replace_all(&PASTE.replace_all(text, " "), " ").into_owned();
    let mut words: Vec<&str> = a.split_whitespace().collect();
    let b;
    if words.is_empty() {
        b = IMAGE
            .replace_all(&PASTE_TAG.replace_all(text, " "), " ")
            .into_owned();
        words = b.split_whitespace().collect();
    }
    words.truncate(6);
    let n = words.join(" ");
    let r: Vec<char> = n.chars().collect();
    if r.len() > 48 {
        return r[..47].iter().collect::<String>() + "…";
    }
    n
}

/// What a session started without a message is called until its first one
/// names it.
pub fn fresh_name(dir: &str) -> String {
    let base = std::path::Path::new(dir)
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_else(|| if dir.is_empty() { ".".into() } else { "/".into() });
    format!("fresh session in {base}")
}

/// Queued messages as one message that still reads as several: each is
/// numbered under its own heading, so the agent doesn't take them for one
/// run-on thought. A lone message goes as it is.
pub fn join_queue(items: &[String]) -> String {
    if items.len() == 1 {
        return items[0].clone();
    }
    let mut b = format!(
        "{} messages, queued while you worked. Each is its own message; take them in order.",
        items.len()
    );
    for (i, it) in items.iter().enumerate() {
        b.push_str(&format!("\n\n[Message {} of {}]\n{}", i + 1, items.len(), it));
    }
    b
}

/// How many of queue q go now: all, or one when separate. A command
/// (/compact, /clear…) goes alone, and what was queued after it waits for
/// it to finish: joined to other text it's only words.
pub fn queue_cut(q: &[String], separate: bool) -> usize {
    let n = if separate { 1.min(q.len()) } else { q.len() };
    for (i, t) in q[..n].iter().enumerate() {
        if is_command(t) {
            return i.max(1);
        }
    }
    n
}

/// Whether a queued message is a slash command.
pub fn is_command(t: &str) -> bool {
    let t = t.trim();
    t.starts_with('/')
        && t.split_whitespace()
            .next()
            .is_some_and(|w| !w[1..].contains(['/', '.']))
}

#[cfg(test)]
mod tests {
    use super::*;

    // name_test.go:5
    #[test]
    fn names_from_the_first_message() {
        for (input, want) in [
            ("fix the login page please", "fix the login page please"),
            (
                "look at this <pasted_content id=\"a\">\nlong log here\n</pasted_content id=\"a\"> [Image #1]",
                "look at this",
            ),
            (
                "<pasted_content id=\"18da\">\nThe feed is definitely better than it used to be\n</pasted_content id=\"18da\">",
                "The feed is definitely better than",
            ),
            ("[Image #1]", ""),
        ] {
            assert_eq!(name_from(input), want, "{input:?}");
        }
        assert_eq!(fresh_name("/src/app"), "fresh session in app");
    }

    // queue_test.go:11
    #[test]
    fn joins_the_queue() {
        assert_eq!(join_queue(&["only".into()]), "only");
        let want = "2 messages, queued while you worked. Each is its own message; take them in order.\n\n[Message 1 of 2]\nfix the test\n\n[Message 2 of 2]\nthen commit";
        assert_eq!(
            join_queue(&["fix the test".into(), "then commit".into()]),
            want
        );
    }

    // queue_test.go:21
    #[test]
    fn cuts_the_queue() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        for (q, sep, want) in [
            (s(&["a", "b"]), false, 2),
            (s(&["a", "b"]), true, 1),
            (s(&["/compact", "then this"]), false, 1),
            (s(&["a", "/compact", "b"]), false, 1),
            (s(&["/Users/lw/x is broken", "b"]), false, 2),
        ] {
            assert_eq!(queue_cut(&q, sep), want, "{q:?} {sep}");
        }
    }
}
