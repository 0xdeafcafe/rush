//! A file change Codex reports as a unified diff, or a whole file's
//! content, read as rush's patches. Port of
//! internal/adapters/codex/diff.go:1-123.

use rush_core::tool::Patch;

/// A unified diff's hunks as patches to path. File headers ("diff --git",
/// "---", "+++", "index") are skipped; a hunk header without line numbers
/// ("@@") starts a hunk at line 0.
pub(crate) fn parse_diff(path: &str, diff: &str) -> Vec<Patch> {
    let mut out: Vec<Patch> = Vec::new();
    let mut cur: Option<usize> = None;
    // Lines the current hunk's header says are left; -1 when it didn't say.
    let (mut rem_old, mut rem_new) = (0i64, 0i64);
    for ln in diff.strip_suffix('\n').unwrap_or(diff).split('\n') {
        if ln.starts_with("@@") {
            out.push(hunk(path, ln));
            let p = out.last().expect("just pushed");
            (rem_old, rem_new) = (p.old_lines, p.new_lines);
            cur = Some(out.len() - 1);
            continue;
        }
        let Some(i) = cur else {
            continue; // file headers before a hunk
        };
        if ln.starts_with('\\') {
            continue; // \ No newline at end of file
        }
        let ln = if ln.is_empty() { " " } else { ln };
        let c = ln.as_bytes()[0];
        if c != b' ' && c != b'-' && c != b'+' {
            cur = None;
            continue;
        }
        out[i].lines.push(ln.to_string());
        if c != b'+' && rem_old > 0 {
            rem_old -= 1;
        }
        if c != b'-' && rem_new > 0 {
            rem_new -= 1;
        }
        if rem_old == 0 && rem_new == 0 {
            cur = None; // the hunk is whole; what follows is the next file's headers
        }
    }
    for p in &mut out {
        fill_counts(p);
    }
    out
}

/// A patch from a header such as "@@ -12,3 +12,4 @@ func f()".
fn hunk(path: &str, header: &str) -> Patch {
    let mut p = Patch {
        path: path.to_string(),
        old_lines: -1,
        new_lines: -1,
        ..Patch::default()
    };
    for f in header.trim_start_matches("@@").split_whitespace() {
        if f == "@@" {
            break;
        }
        let b = f.as_bytes();
        if b.len() < 2 || (b[0] != b'-' && b[0] != b'+') {
            continue;
        }
        let (start, count) = match f[1..].split_once(',') {
            Some((s, c)) => (s, Some(c)),
            None => (&f[1..], None),
        };
        let s: i64 = start.parse().unwrap_or(0);
        let n: i64 = count.map_or(1, |c| c.parse().unwrap_or(0));
        if b[0] == b'-' {
            (p.old_start, p.old_lines) = (s, n);
        } else {
            (p.new_start, p.new_lines) = (s, n);
        }
    }
    p
}

/// Counts a hunk's lines when its header didn't.
fn fill_counts(p: &mut Patch) {
    let (mut old, mut new) = (0, 0);
    for l in &p.lines {
        let c = l.as_bytes()[0];
        if c != b'+' {
            old += 1;
        }
        if c != b'-' {
            new += 1;
        }
    }
    if p.old_lines < 0 {
        p.old_lines = old;
    }
    if p.new_lines < 0 {
        p.new_lines = new;
    }
}

/// A file's content as one hunk: all added, or all removed.
pub(crate) fn whole_file(path: &str, content: &str, added: bool) -> Vec<Patch> {
    if content.is_empty() {
        return Vec::new();
    }
    let sign = if added { '+' } else { '-' };
    let lines: Vec<String> = content
        .strip_suffix('\n')
        .unwrap_or(content)
        .split('\n')
        .map(|l| format!("{sign}{l}"))
        .collect();
    let n = lines.len() as i64;
    let mut p = Patch {
        path: path.to_string(),
        lines,
        ..Patch::default()
    };
    if added {
        (p.new_start, p.new_lines) = (1, n);
    } else {
        (p.old_start, p.old_lines) = (1, n);
    }
    vec![p]
}

/// Whether s is a unified diff rather than a file's content.
pub(crate) fn is_diff(s: &str) -> bool {
    s.starts_with("@@")
        || s.contains("\n@@")
        || s.starts_with("--- ")
        || s.starts_with("diff --git")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(old: (i64, i64), new: (i64, i64), lines: &[&str]) -> Patch {
        Patch {
            path: "p".into(),
            old_start: old.0,
            old_lines: old.1,
            new_start: new.0,
            new_lines: new.1,
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }

    // codex_test.go:TestParseDiff
    #[test]
    fn parse_diff_reads_hunks() {
        let cases: Vec<(&str, &str, Vec<Patch>)> = vec![
            (
                "two hunks",
                "@@ -1,2 +1,2 @@\n-a\n+b\n c\n@@ -10 +10,2 @@ func f()\n x\n+y\n",
                vec![
                    patch((1, 2), (1, 2), &["-a", "+b", " c"]),
                    patch((10, 1), (10, 2), &[" x", "+y"]),
                ],
            ),
            (
                "file headers",
                "diff --git a/p b/p\nindex 1..2\n--- a/p\n+++ b/p\n@@ -3,1 +3,1 @@\n-old\n+new\n\\ No newline at end of file\n",
                vec![patch((3, 1), (3, 1), &["-old", "+new"])],
            ),
            (
                "new file",
                "@@ -0,0 +1,2 @@\n+a\n+b\n",
                vec![patch((0, 0), (1, 2), &["+a", "+b"])],
            ),
            (
                "two files",
                "--- a/p\n+++ b/p\n@@ -1 +1 @@\n-a\n+b\n--- a/q\n+++ b/q\n@@ -1 +1 @@\n-c\n+d\n",
                vec![
                    patch((1, 1), (1, 1), &["-a", "+b"]),
                    patch((1, 1), (1, 1), &["-c", "+d"]),
                ],
            ),
            (
                "bare header",
                "@@\n-a\n+b\n+c\n",
                vec![patch((0, 1), (0, 2), &["-a", "+b", "+c"])],
            ),
            ("no hunks", "", vec![]),
        ];
        for (name, diff, want) in cases {
            assert_eq!(parse_diff("p", diff), want, "{name}");
        }
    }

    #[test]
    fn whole_files_and_diffs() {
        assert_eq!(
            whole_file("p", "one\ntwo\n", true),
            vec![patch((0, 0), (1, 2), &["+one", "+two"])]
        );
        assert_eq!(
            whole_file("p", "x", false),
            vec![patch((1, 1), (0, 0), &["-x"])]
        );
        assert!(whole_file("p", "", true).is_empty());
        for (s, want) in [
            ("@@ -1 +1 @@", true),
            ("a\n@@ x", true),
            ("--- a/p", true),
            ("diff --git a b", true),
            ("package a\n", false),
        ] {
            assert_eq!(is_diff(s), want, "{s}");
        }
    }
}
