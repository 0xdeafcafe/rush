//! Small text helpers the model reads its lines with. Port of
//! internal/convo/model.go:814-841,996-1026 (exitRe, isRejection,
//! firstLine, stripMarkdown, firstPlain), transcript.go:615-626,714-728
//! (between, attr, stripTags), where.go (WorktreeIn, DropCd) and
//! changes.go:553 (firstNonEmpty). Go's regexps are written out by hand:
//! the crate has no regex engine.

/// s trimmed, up to its first line break, trimmed again.
pub fn one_line(s: &str) -> &str {
    let s = s.trim();
    match s.find('\n') {
        Some(i) => s[..i].trim(),
        None => s,
    }
}

/// The first line of s, trimmed (Go's firstLine).
pub fn first_line(s: &str) -> &str {
    one_line(s)
}

/// The first of xs that isn't empty.
pub fn first_non_empty<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

/// s without markdown's bold, code and heading marks.
pub fn strip_markdown(s: &str) -> String {
    let s = strip_marks(s);
    s.trim_start_matches(['#', '>', ' ', '-', '*']).to_owned()
}

fn strip_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    let mut from = 0;
    while i < b.len() {
        let skip = match b[i] {
            b'*' if b.get(i + 1) == Some(&b'*') => 2,
            b'_' if b.get(i + 1) == Some(&b'_') => 2,
            b'`' => 1,
            _ => 0,
        };
        if skip > 0 {
            out.push_str(&s[from..i]);
            i += skip;
            from = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&s[from..]);
    out
}

/// The first line of s that has words once markdown's marks are off,
/// reading only as far as the line it returns.
pub fn first_plain(s: &str) -> String {
    let mut first = true;
    for line in s.split('\n') {
        let mut l = strip_marks(line);
        if first {
            let t = l.trim_start_matches(['#', '>', ' ', '-', '*']);
            l = t.to_owned();
            first = false;
        }
        let t = l.trim();
        if !t.is_empty() {
            return t.to_owned();
        }
    }
    String::new()
}

/// What's between the first a in s and the next b after it, trimmed; ""
/// when either is missing.
pub fn between<'a>(s: &'a str, a: &str, b: &str) -> &'a str {
    let Some(i) = s.find(a) else { return "" };
    let rest = &s[i + a.len()..];
    match rest.find(b) {
        Some(j) => rest[..j].trim(),
        None => "",
    }
}

/// The value of attribute name="…" in a tag.
pub fn attr<'a>(s: &'a str, name: &str) -> &'a str {
    let key = format!("{name}=\"");
    let Some(i) = s.find(&key) else { return "" };
    let rest = &s[i + key.len()..];
    match rest.find('"') {
        Some(j) => &rest[..j],
        None => "",
    }
}

/// s with each tag (`<…>`, up to 80 characters inside) put as a space,
/// trimmed: Go's `<[^>]{1,80}>`.
pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut from = 0;
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b'<'
            && let Some(end) = tag_end(&s[i + 1..])
        {
            out.push_str(&s[from..i]);
            out.push(' ');
            i += 1 + end + 1;
            from = i;
            continue;
        }
        i += 1;
    }
    out.push_str(&s[from..]);
    out.trim().to_owned()
}

/// Where the '>' closing a tag is in rest, if 1 to 80 characters that
/// aren't '>' come first.
fn tag_end(rest: &str) -> Option<usize> {
    for (n, (i, c)) in rest.char_indices().enumerate() {
        if c == '>' {
            return (n > 0).then_some(i);
        }
        if n == 80 {
            return None;
        }
    }
    None
}

/// The exit code a tool's text reports on a line of its own: Go's
/// `(?m)^(?:Error: )?Exit code (\d+)`.
pub fn exit_code_in(text: &str) -> Option<i32> {
    for line in text.split('\n') {
        let l = line.strip_prefix("Error: ").unwrap_or(line);
        let Some(rest) = l.strip_prefix("Exit code ") else {
            continue;
        };
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 {
            return Some(rest[..n].parse().unwrap_or(i32::MAX));
        }
    }
    None
}

/// A tool call you (or a rule) refused, as against one that failed on its
/// own, like a file it had no permission to read.
pub fn is_rejection(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "user declined",
        "user rejected",
        "doesn't want to proceed",
        "tool use was rejected",
        "requires approval",
        "permission to use",
        "haven't granted",
        "has been denied",
        "auto mode classifier",
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// The worktree a command or a call's input points into, by name: the last
/// one it names (`/\.?worktrees/([^/\s"'\\]+)`). "" when none.
pub fn worktree_in(s: &str) -> &str {
    worktrees_in(s).last().copied().unwrap_or("")
}

/// Every worktree s names, in order: each folder right after a
/// "worktrees/" or ".worktrees/" in a path.
pub fn worktrees_in(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(k) = s[from..].find("worktrees/") {
        let at = from + k;
        from = at + "worktrees/".len();
        let pre = &s[..at];
        if !(pre.ends_with('/') || pre.ends_with("/.")) {
            continue;
        }
        let rest = &s[from..];
        let end = rest
            .find(|c: char| c == '/' || c.is_whitespace() || c == '"' || c == '\'' || c == '\\')
            .unwrap_or(rest.len());
        if end > 0 {
            out.push(&rest[..end]);
            from += end;
        }
    }
    out
}

/// cmd without a leading cd into a worktree, which the row says by name
/// instead: "cd …/.worktrees/x && make" is "make".
pub fn drop_cd(cmd: &str) -> &str {
    let Some(after) = cmd.strip_prefix("cd ") else {
        return cmd;
    };
    let Some((dir, rest)) = after.split_once(" && ") else {
        return cmd;
    };
    if worktree_in(dir).is_empty() || dir.contains([';', '|', '&']) {
        return cmd;
    }
    rest
}

/// Whether s ends in an `[Image #N]` marker.
pub fn ends_with_image(s: &str) -> bool {
    let Some(s) = s.strip_suffix(']') else {
        return false;
    };
    let digits = s.bytes().rev().take_while(u8::is_ascii_digit).count();
    digits > 0 && s[..s.len() - digits].ends_with("[Image #")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktrees() {
        let cmd = "cd /Users/lw/src/langwatch/.worktrees/go-ports-graph && rm -rf .claude/tmp";
        assert_eq!(worktree_in(cmd), "go-ports-graph");
        assert_eq!(drop_cd(cmd), "rm -rf .claude/tmp");
        assert_eq!(
            worktree_in(r#"{"file_path":"/r/.claude/worktrees/lane-a/x.go"}"#),
            "lane-a"
        );
        for keep in ["cd /tmp && make", "cd a/.worktrees/b; make && x", "make"] {
            assert_eq!(drop_cd(keep), keep);
        }
        assert_eq!(worktree_in("/a/worktrees/x and /b/.worktrees/y"), "y");
        assert_eq!(worktree_in("myworktrees/x"), "");
        assert_eq!(
            worktrees_in("cd /r/.claude/worktrees/a && cp /r/.claude/worktrees/b/x ."),
            ["a", "b"]
        );
    }

    #[test]
    fn exit_codes() {
        let cases = [
            ("Exit code 1\nboom", Some(1)),
            ("Error: Exit code 2\nboom", Some(2)),
            ("boom\nExit code 127", Some(127)),
            ("said Exit code 3", None),
            ("Exit code x", None),
            ("", None),
        ];
        for (text, want) in cases {
            assert_eq!(exit_code_in(text), want, "{text:?}");
        }
    }

    #[test]
    fn plain_words() {
        assert_eq!(first_plain("## **Fixed** the `bug`\nmore"), "Fixed the bug");
        assert_eq!(first_plain("  - \nsecond"), "second");
        assert_eq!(first_plain("\n  - x"), "- x");
        assert_eq!(first_plain(""), "");
        assert_eq!(strip_markdown("> **a** b"), "a b");
        assert_eq!(first_line("  a \n b"), "a");
    }

    #[test]
    fn tags() {
        assert_eq!(strip_tags("<b>hi</b> there"), "hi  there");
        assert_eq!(strip_tags("a <> b"), "a <> b");
        assert_eq!(
            strip_tags(&format!("<{}>", "x".repeat(81))),
            format!("<{}>", "x".repeat(81))
        );
        assert_eq!(between("x <a> hi </a>", "<a>", "</a>"), "hi");
        assert_eq!(attr(r#"<m from-name="bob">"#, "from-name"), "bob");
        assert!(ends_with_image("see [Image #12]"));
        assert!(!ends_with_image("see [Image #]"));
        assert!(!ends_with_image("see [Image #1] x"));
        assert!(is_rejection("The User Declined this"));
    }
}
