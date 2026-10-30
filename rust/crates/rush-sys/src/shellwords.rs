//! A shell command's simple commands and their words, roughly as sh splits
//! them, and whether it has a here-document: what `script` needs to tell a
//! long call from a one-liner.
//!
//! Ported from internal/gate/hook.go:58-279 (`HereDoc`, `commands`,
//! `hereDocWord`, `skipBodies`); the gate's port can use these.

/// Whether command has a here-document: <<, or <<-, then its word, bare or
/// quoted; not a here-string's <<<. (Go's regexp
/// `(^|[^<])<<-?[ \t]*['"]?[A-Za-z_]`.)
pub fn here_doc(command: &str) -> bool {
    let s = command.as_bytes();
    (0..s.len().saturating_sub(1)).any(|i| {
        if &s[i..i + 2] != b"<<" || (i > 0 && s[i - 1] == b'<') {
            return false;
        }
        let mut j = i + 2;
        if s.get(j) == Some(&b'-') {
            j += 1;
        }
        while matches!(s.get(j), Some(b' ' | b'\t')) {
            j += 1;
        }
        if matches!(s.get(j), Some(b'\'' | b'"')) {
            j += 1;
        }
        s.get(j)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    })
}

struct HereDocEnd {
    word: Vec<u8>,
    tabs: bool,
}

/// The words of each simple command in s, roughly as sh splits them; None
/// when a quote isn't closed.
pub fn commands(s: &str) -> Option<Vec<Vec<String>>> {
    let s = s.as_bytes();
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut w: Vec<u8> = Vec::new();
    let mut inw = false;
    let mut docs: Vec<HereDocEnd> = Vec::new(); // here-documents whose bodies start at the next newline
    fn word(cur: &mut Vec<String>, w: &mut Vec<u8>, inw: &mut bool) {
        if *inw {
            cur.push(String::from_utf8_lossy(w).into_owned());
            w.clear();
            *inw = false;
        }
    }
    fn end(out: &mut Vec<Vec<String>>, cur: &mut Vec<String>, w: &mut Vec<u8>, inw: &mut bool) {
        word(cur, w, inw);
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    }
    let find = |from: usize, c: u8| s[from..].iter().position(|&b| b == c);
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            b'\'' => {
                let j = find(i + 1, b'\'')?;
                w.extend_from_slice(&s[i + 1..i + 1 + j]);
                inw = true;
                i += 1 + j;
            }
            b'"' => {
                let mut j = i + 1;
                while j < s.len() && s[j] != b'"' {
                    if s[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j >= s.len() {
                    return None;
                }
                w.extend_from_slice(&s[i + 1..j]);
                inw = true;
                i = j;
            }
            b'\\' if i + 1 < s.len() => {
                if s[i + 1] != b'\n' {
                    w.push(s[i + 1]);
                    inw = true;
                }
                i += 1;
            }
            b'$' if s.get(i + 1) == Some(&b'{') => {
                let j = find(i, b'}')?;
                w.extend_from_slice(&s[i..i + j + 1]);
                inw = true;
                i += j;
            }
            b'$' if s.get(i + 1) == Some(&b'(') => {
                end(&mut out, &mut cur, &mut w, &mut inw);
                i += 1;
            }
            b'#' if !inw => {
                let j = find(i, b'\n').unwrap_or(s.len() - i);
                // Go's i += j-1 then i++: lands on the newline itself.
                i = (i + j).wrapping_sub(1);
            }
            b' ' | b'\t' => word(&mut cur, &mut w, &mut inw),
            b'<' if s[i..].starts_with(b"<<") && !s[i..].starts_with(b"<<<") => {
                word(&mut cur, &mut w, &mut inw);
                let (d, n) = here_doc_word(&s[i + 2..]);
                docs.push(d);
                i += 1 + n;
            }
            b'\n' if !docs.is_empty() => {
                end(&mut out, &mut cur, &mut w, &mut inw);
                i = skip_bodies(s, i + 1, &docs).wrapping_sub(1);
                docs.clear();
            }
            b';' | b'&' | b'|' | b'(' | b')' | b'{' | b'}' | b'`' | b'\n' => {
                end(&mut out, &mut cur, &mut w, &mut inw);
            }
            _ => {
                w.push(c);
                inw = true;
            }
        }
        i = i.wrapping_add(1);
    }
    end(&mut out, &mut cur, &mut w, &mut inw);
    Some(out)
}

/// The end of the here-document whose << s follows, and how many bytes of
/// s say it.
fn here_doc_word(s: &[u8]) -> (HereDocEnd, usize) {
    let mut i = 0;
    let tabs = s.first() == Some(&b'-');
    if tabs {
        i += 1;
    }
    while i < s.len() && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    let mut word = Vec::new();
    while i < s.len() && !b" \t\n;&|()<>".contains(&s[i]) {
        if !matches!(s[i], b'\'' | b'"' | b'\\') {
            word.push(s[i]);
        }
        i += 1;
    }
    (HereDocEnd { word, tabs }, i)
}

/// Where s goes on after the bodies of docs, which start at from, one
/// after another.
fn skip_bodies(s: &[u8], from: usize, docs: &[HereDocEnd]) -> usize {
    let mut i = from;
    for d in docs {
        while i < s.len() {
            let (mut line, next) = match s[i..].iter().position(|&b| b == b'\n') {
                Some(j) => (&s[i..i + j], i + j + 1),
                None => (&s[i..], s.len()),
            };
            if d.tabs {
                while let [b'\t', rest @ ..] = line {
                    line = rest;
                }
            }
            i = next;
            if line == d.word.as_slice() {
                break;
            }
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn here_docs() {
        for (cmd, want) in [
            ("cat <<EOF\nx\nEOF", true),
            ("python3 - <<'EOF'\nx\nEOF", true),
            ("cat <<-\"END\"\n\tx\nEND", true),
            (r#"grep x <<< "$s""#, false),
            ("echo 'a << b'; ls", true), // quoted, but taking it as one only costs the wrapping
            ("for i in 1 2; do echo; done", false),
            ("<<", false),
        ] {
            assert_eq!(here_doc(cmd), want, "{cmd:?}");
        }
    }

    #[test]
    fn splits_commands() {
        let w = |v: &[&[&str]]| {
            Some(
                v.iter()
                    .map(|c| c.iter().map(|s| s.to_string()).collect())
                    .collect(),
            )
        };
        for (cmd, want) in [
            ("ls -la", w(&[&["ls", "-la"]])),
            (
                "cd apps/ui && FOO=1 pnpm exec tsc -b",
                w(&[&["cd", "apps/ui"], &["FOO=1", "pnpm", "exec", "tsc", "-b"]]),
            ),
            (
                r#"git commit -m "go faster" && echo tsc"#,
                w(&[&["git", "commit", "-m", "go faster"], &["echo", "tsc"]]),
            ),
            (
                "echo $(go vet ./...)",
                w(&[&["echo"], &["go", "vet", "./..."]]),
            ),
            (
                "echo ${HOME}/x # a comment\nls",
                w(&[&["echo", "${HOME}/x"], &["ls"]]),
            ),
            ("a\\ b c\\\nd", w(&[&["a b", "cd"]])), // a line continued joins the word
            (
                "cat > b.md <<'EOF'\nruns (tsc\nEOF\ngh pr create",
                w(&[&["cat", ">", "b.md"], &["gh", "pr", "create"]]),
            ),
            (
                "cat <<-END | wc\n\t(tsc\n\tEND\ngo test",
                w(&[&["cat"], &["wc"], &["go", "test"]]),
            ),
            // As Go reads it: the second < starts a here-document named tsc.
            ("cat <<< tsc", w(&[&["cat", "<"]])),
            ("echo 'unclosed", None),
            ("echo \"unclosed", None),
            ("# only a comment", w(&[])),
        ] {
            assert_eq!(commands(cmd), want, "{cmd:?}");
        }
    }
}
