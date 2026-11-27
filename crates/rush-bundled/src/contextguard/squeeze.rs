//! What the context guard does to a shell command's output before the
//! agent sees it: terminal codes out, a progress bar down to its last
//! state, spaces at line ends gone, blank lines and repeated lines run
//! together, and a very long output cut to its head and tail (the whole
//! of it kept in a file the agent is pointed to). Pure, and the same
//! output always squeezes the same. Never for a command whose output is a
//! file's own text (cat, git diff, rg…): an edit needs its exact spaces.
//! Rust-only.

use crate::gatecore;

/// Past this many lines, an output is cut to its head and tail.
pub const CAP_LINES: usize = 600;
/// How many lines of each end a cut output keeps.
pub const KEEP_LINES: usize = 200;
/// Past this many bytes, an output is cut too, to a third of them at each
/// end: long lines (a compiler's, a minified file) pass a line count.
pub const CAP_BYTES: usize = 32 * 1024;

/// s without its terminal codes: colours, cursor moves, titles.
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            // CSI: parameters and intermediates, then a final byte.
            Some('[') => {
                for c in it.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            // Intermediates, then a final byte (ESC ( B).
            Some(c) if ('\x20'..='\x2f').contains(&c) => {
                while it.peek().is_some_and(|c| ('\x20'..='\x2f').contains(c)) {
                    it.next();
                }
                it.next();
            }
            _ => {}
        }
    }
    out
}

/// A line as a terminal leaves it after carriage returns: what was
/// written last.
fn last_state(line: &str) -> &str {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if !line.contains('\r') {
        return line;
    }
    line.rsplit('\r')
        .find(|p| !p.trim().is_empty())
        .unwrap_or("")
}

/// output squeezed: terminal codes and progress redraws out, line ends
/// trimmed, runs of blank lines made one, and a line repeated in a row
/// written once with (×N) where that's shorter.
pub fn squeeze(output: &str) -> String {
    let text = strip_ansi(output);
    let ends_nl = text.ends_with('\n');
    let body = text.strip_suffix('\n').unwrap_or(&text);
    let mut lines: Vec<&str> = Vec::new();
    let mut blank = false;
    for l in body.split('\n') {
        let l = last_state(l).trim_end();
        if l.is_empty() {
            if blank || lines.is_empty() {
                continue;
            }
            blank = true;
        } else {
            blank = false;
        }
        lines.push(l);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let mut n = 1;
        while i + n < lines.len() && lines[i + n] == l {
            n += 1;
        }
        let mark = format!(" (×{n})");
        if n > 1 && (n - 1) * (l.len() + 1) > mark.len() {
            out.push_str(l);
            out.push_str(&mark);
            out.push('\n');
        } else {
            for _ in 0..n {
                out.push_str(l);
                out.push('\n');
            }
        }
        i += n;
    }
    if !ends_nl && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// output cut to its first and last keep lines when it has more than
/// over, and to a third of max_bytes at each end when it's longer than
/// that (as set: CAP_LINES, KEEP_LINES, CAP_BYTES unless), saying where the
/// whole of it is kept; None when it's short enough. A byte cut ends at a
/// line's end where one is near enough, else inside the line.
pub fn cap(
    output: &str,
    kept_at: &str,
    over: usize,
    keep: usize,
    max_bytes: usize,
) -> Option<String> {
    let lines: Vec<&str> = output.split_inclusive('\n').collect();
    let by_lines = lines.len() > over;
    if !by_lines && output.len() <= max_bytes {
        return None;
    }
    let (mut head, mut tail) = (output.len(), 0);
    if by_lines {
        let keep = keep.min(over / 2);
        head = lines[..keep].iter().map(|l| l.len()).sum();
        tail = output.len()
            - lines[lines.len() - keep..]
                .iter()
                .map(|l| l.len())
                .sum::<usize>();
    }
    let budget = max_bytes / 3;
    let mut by_bytes = false;
    if output.len() > max_bytes {
        if head > budget {
            let mut at = floor(output, budget);
            if let Some(nl) = output[..at].rfind('\n')
                && nl + 1 > budget / 2
            {
                at = nl + 1;
            }
            head = at;
            by_bytes = true;
        }
        if output.len() - tail > budget {
            let from = ceil(output, output.len() - budget);
            let mut at = from;
            if let Some(nl) = output[from..].find('\n')
                && nl < budget / 2
                && from + nl + 1 < output.len()
            {
                at = from + nl + 1;
            }
            tail = at;
            by_bytes = true;
        }
    }
    if tail <= head {
        return None;
    }
    let gone = &output[head..tail];
    let mut out = output[..head].to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if by_bytes {
        let n = gone.matches('\n').count();
        out.push_str(&format!(
            "… {n} lines, {} KB, left out (the whole output: {kept_at}) …\n",
            gone.len().div_ceil(1024)
        ));
    } else {
        let left = lines.len() - 2 * keep.min(over / 2);
        out.push_str(&format!(
            "… {left} lines left out (the whole output: {kept_at}) …\n"
        ));
    }
    out.push_str(&output[tail..]);
    Some(out)
}

/// The char boundary at or before i.
fn floor(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The char boundary at or after i.
fn ceil(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Whether a command's output is squeezed: not when what it prints is a
/// file's own text, or a diff, whose exact spaces an edit needs.
pub fn squeezes(command: &str) -> bool {
    let Some(cmds) = gatecore::commands(command) else {
        return false;
    };
    for words in &cmds {
        let mut w: &[String] = words;
        while let Some(first) = w.first()
            && (matches!(
                first.as_str(),
                "sudo"
                    | "env"
                    | "time"
                    | "command"
                    | "exec"
                    | "nice"
                    | "nohup"
                    | "!"
                    | "do"
                    | "then"
                    | "else"
            ) || first.contains('=') && !first.starts_with('-'))
        {
            w = &w[1..];
        }
        let Some(prog) = w.first() else {
            continue;
        };
        let prog = prog.rsplit('/').next().unwrap_or(prog.as_str());
        if matches!(
            prog,
            "cat"
                | "bat"
                | "head"
                | "tail"
                | "sed"
                | "awk"
                | "gawk"
                | "nl"
                | "less"
                | "more"
                | "diff"
                | "cmp"
                | "patch"
                | "xxd"
                | "od"
                | "hexdump"
                | "base64"
                | "jq"
                | "yq"
                | "grep"
                | "egrep"
                | "fgrep"
                | "rg"
                | "ag"
                | "ack"
                | "cut"
                | "tr"
                | "column"
                | "printf"
        ) {
            return false;
        }
        if prog == "git" && git_shows_text(&w[1..]) {
            return false;
        }
    }
    true
}

/// Whether a git command prints files' text or patches.
fn git_shows_text(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        // git -C dir, -c k=v: their value is the next word.
        if matches!(args[i].as_str(), "-C" | "-c" | "--git-dir" | "--work-tree") {
            i += 1;
        }
        i += 1;
    }
    let Some(sub) = args.get(i) else {
        return false;
    };
    let rest = &args[i + 1..];
    match sub.as_str() {
        "diff" | "show" | "blame" | "cat-file" | "format-patch" | "grep" | "apply" | "am" => true,
        "log" | "whatchanged" => rest
            .iter()
            .any(|a| a == "-p" || a == "--patch" || a.starts_with("-L") || a.starts_with("-U")),
        "stash" => rest.first().is_some_and(|a| a == "show"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squeezes_output() {
        let cases: &[(&str, &str, &str)] = &[
            ("plain stays", "a\nb\n", "a\nb\n"),
            ("no newline stays", "a\nb", "a\nb"),
            (
                "colours out",
                "\x1b[1m\x1b[32mCompiling\x1b[0m x\n",
                "Compiling x\n",
            ),
            (
                "title and charset out",
                "\x1b]0;title\x07a\x1b(Bb\n",
                "ab\n",
            ),
            ("line ends trimmed", "a  \nb\t\n", "a\nb\n"),
            ("CRLF", "a\r\nb\r\n", "a\nb\n"),
            (
                "a progress bar's last state",
                "  10% [=>   ]\r  50% [===> ]\r 100% [=====]\ndone\n",
                " 100% [=====]\ndone\n",
            ),
            ("a cleared progress line", "50%\r\x1b[K\rdone\n", "done\n"),
            ("blank runs made one", "a\n\n\n\nb\n", "a\n\nb\n"),
            ("leading and trailing blanks out", "\n\na\n\n\n", "a\n"),
            (
                "repeats written once",
                "warning: unused\nwarning: unused\nwarning: unused\nok\n",
                "warning: unused (×3)\nok\n",
            ),
            (
                "short repeats kept when that's shorter",
                "a\na\nb\n",
                "a\na\nb\n",
            ),
            (
                "not when apart",
                "x line\ny\nx line\n",
                "x line\ny\nx line\n",
            ),
            ("empty", "", ""),
        ];
        for (what, input, want) in cases {
            assert_eq!(&squeeze(input), want, "{what}");
            // Squeezing twice changes nothing more.
            assert_eq!(squeeze(&squeeze(input)), squeeze(input), "{what}: twice");
        }
    }

    /// What squeezing saves on real-looking outputs, in bytes: DESIGN.md's
    /// table (Context guard) is these numbers.
    #[test]
    fn measured_on_fixtures() {
        let cases: &[(&str, &str, usize, usize)] = &[
            (
                "cargo build",
                include_str!("testdata/cargo-build.txt"),
                16168,
                5542,
            ),
            (
                "cargo test",
                include_str!("testdata/cargo-test.txt"),
                11815,
                9618,
            ),
            (
                "npm install",
                include_str!("testdata/npm-install.txt"),
                34414,
                325,
            ),
            ("git log", include_str!("testdata/git-log.txt"), 3153, 2646),
            ("pytest", include_str!("testdata/pytest.txt"), 1755, 1397),
            ("download", include_str!("testdata/download.txt"), 80081, 80),
        ];
        for (what, input, before, after) in cases {
            let got = squeeze(input);
            assert_eq!(
                (input.len(), got.len()),
                (*before, *after),
                "{what}: squeezed to\n{got}"
            );
            assert_eq!(squeeze(&got), got, "{what}: twice");
        }
        // Nothing the agent needs goes: the failure and its numbers stay.
        let t = squeeze(include_str!("testdata/cargo-test.txt"));
        assert!(
            t.contains("test module_5::tests::case_117 ... FAILED\n"),
            "{t}"
        );
        assert!(t.contains("239 passed; 1 failed"), "{t}");
        let b = squeeze(include_str!("testdata/cargo-build.txt"));
        assert!(b.contains("12 |     let x = 1;\n"), "{b}");
        assert!(b.contains("Finished `dev` profile"), "{b}");
    }

    #[test]
    fn caps_long_output() {
        let short: String = (0..CAP_LINES).map(|i| format!("{i}\n")).collect();
        assert_eq!(
            cap(&short, "/tmp/x", CAP_LINES, KEEP_LINES, CAP_BYTES),
            None
        );
        let long: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let got = cap(&long, "/tmp/out.txt", CAP_LINES, KEEP_LINES, CAP_BYTES).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines.len(), 2 * KEEP_LINES + 1);
        assert_eq!(lines[0], "line 0");
        assert_eq!(lines[KEEP_LINES - 1], "line 199");
        assert_eq!(
            lines[KEEP_LINES],
            "… 600 lines left out (the whole output: /tmp/out.txt) …"
        );
        assert_eq!(lines[KEEP_LINES + 1], "line 800");
        assert_eq!(lines.last(), Some(&"line 999"));
        // As set: output of n lines, longer than, kept each end: the lines
        // after the cut, or None.
        let cases: &[(usize, usize, usize, Option<usize>)] = &[
            (1000, 300, 100, Some(201)),
            (1000, 1200, 400, None),
            (1300, 1200, 400, Some(801)),
            (300, 300, 100, None),
            (301, 300, 100, Some(201)),
        ];
        for &(n, over, keep, want) in cases {
            let out: String = (0..n).map(|i| format!("line {i}\n")).collect();
            let got = cap(&out, "/tmp/o", over, keep, CAP_BYTES).map(|c| c.lines().count());
            assert_eq!(got, want, "{n} lines, over {over}, keep {keep}");
        }
    }

    #[test]
    fn caps_long_output_by_bytes() {
        // Output; the cap in bytes; the lines kept before the note, its
        // words, and the bytes after it; None: left as it is.
        type Case = (
            &'static str,
            String,
            usize,
            Option<(usize, &'static str, usize)>,
        );
        let wide = |n: usize| -> String {
            (0..n)
                .map(|i| format!("{i:04} {}\n", "x".repeat(95)))
                .collect()
        };
        let cases: Vec<Case> = vec![
            ("short enough", wide(100), 16 * 1024, None),
            ("just at the cap", "a".repeat(16 * 1024), 16 * 1024, None),
            (
                // 300 lines of 100 bytes: under the line cap, over 16 KB.
                "long lines under the line cap",
                wide(300),
                16 * 1024,
                Some((
                    54,
                    "… 192 lines, 19 KB, left out (the whole output: /tmp/o) …",
                    5454,
                )),
            ),
            (
                "one huge line is cut inside it",
                "y".repeat(100_000),
                32 * 1024,
                Some((
                    1,
                    "… 0 lines, 77 KB, left out (the whole output: /tmp/o) …",
                    10922,
                )),
            ),
            (
                "over both caps: the bytes win",
                wide(1000),
                32 * 1024,
                Some((
                    108,
                    "… 784 lines, 78 KB, left out (the whole output: /tmp/o) …",
                    10908,
                )),
            ),
            (
                "over the lines only: cut by lines",
                (0..1000).map(|i| format!("{i}\n")).collect(),
                32 * 1024,
                Some((
                    200,
                    "… 600 lines left out (the whole output: /tmp/o) …",
                    800,
                )),
            ),
        ];
        for (what, out, max, want) in cases {
            let got = cap(&out, "/tmp/o", CAP_LINES, KEEP_LINES, max);
            let got = got.map(|c| {
                let (i, note) = c
                    .lines()
                    .enumerate()
                    .find(|(_, l)| l.starts_with("… "))
                    .unwrap();
                let after = c.split_once(&format!("{note}\n")).unwrap().1.len();
                // What's kept is the output's own start and end.
                let head = c.split_once("… ").unwrap().0;
                assert!(out.starts_with(head.trim_end_matches('\n')), "{what}: head");
                assert!(out.ends_with(&c[c.len() - after..]), "{what}: tail");
                assert!(c.len() <= max, "{what}: {} bytes", c.len());
                (i, note.to_string(), after)
            });
            let want = want.map(|(i, n, a)| (i, n.to_string(), a));
            assert_eq!(got, want, "{what}");
        }
        // A cut never splits a character.
        let s = "é".repeat(40_000);
        let c = cap(&s, "/tmp/o", CAP_LINES, KEEP_LINES, CAP_BYTES).unwrap();
        assert!(c.starts_with('é') && c.ends_with('é'));
    }

    #[test]
    fn which_commands() {
        let cases: &[(&str, bool)] = &[
            ("cargo build", true),
            ("cargo test --workspace 2>&1", true),
            ("npm install", true),
            ("pytest -q", true),
            ("git log --oneline -20", true),
            ("git status", true),
            ("curl -LO https://x/y.tar.gz", true),
            ("make && ./run.sh", true),
            ("RUST_LOG=debug cargo run", true),
            ("cat src/main.rs", false),
            ("head -50 a.txt", false),
            ("sed -n 10,20p a.rs", false),
            ("git diff", false),
            ("git -C repo show HEAD:a.rs", false),
            ("git log -p -3", false),
            ("git stash show -p", false),
            ("rg -n foo src", false),
            ("cargo test 2>&1 | grep FAIL", false),
            ("cd x && cat y", false),
            ("jq . a.json", false),
            ("/usr/bin/diff a b", false),
            ("echo 'unclosed", false),
        ];
        for (command, want) in cases {
            assert_eq!(squeezes(command), *want, "squeezes({command:?})");
        }
    }
}
