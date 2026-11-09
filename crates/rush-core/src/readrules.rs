//! Whether a shell command only reads, as a session's own Bash rules
//! allow it, once the well-known variables it names are taken as safe.
//! Claude Code asks about any command with a variable in it ("a variable
//! in this command can't be checked before it runs") or a backslash
//! before a space, whatever its allow rules say, even in bypass mode; the
//! cleanup agent's survey (`du "$TMPDIR"`, `echo $TMPDIR`) asked for each.
//! Its host answers those itself when this says yes (rush-host
//! server/readallow.rs). Rust-only: Go had no such rules.
//!
//! Strict on purpose: a command passes only when every simple command in
//! it (split at `;` and `&&`) is one a rule allows, or echo, printf, pwd
//! or getconf, and nothing in it runs or writes anything else: no
//! substitution (`$(…)`, backticks), no redirection, pipe, background,
//! subshell or group, no `VAR=` before a command, and no variable but
//! TMPDIR, HOME, RUSH_SESSION, PWD and USER.

/// The variables a read-only command may name: where the session's
/// scratch is, whose home, which session, where it is, who.
pub const WELL_KNOWN: &[&str] = &["TMPDIR", "HOME", "RUSH_SESSION", "PWD", "USER"];

/// Commands that only print, allowed whatever the rules.
const PRINTS: &[&str] = &["echo", "printf", "pwd", "getconf"];

/// A Bash rule as words: `Bash(du:*)` the prefix du, `Bash(git branch)`
/// exactly git branch. None for any other rule.
fn rule(r: &str) -> Option<(Vec<String>, bool)> {
    let body = r.strip_prefix("Bash(")?.strip_suffix(')')?;
    let (words, prefix) = match body.strip_suffix(":*") {
        Some(p) => (p, true),
        None => (body, false),
    };
    let w: Vec<String> = words.split_whitespace().map(str::to_string).collect();
    (!w.is_empty()).then_some((w, prefix))
}

/// The simple commands of cmd, each as its words, or None when anything
/// in it isn't plain: see the module's doc.
fn simple_commands(cmd: &str) -> Option<Vec<Vec<String>>> {
    let b: Vec<char> = cmd.chars().collect();
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut w = String::new();
    let mut inw = false;
    let push_word = |cur: &mut Vec<String>, w: &mut String, inw: &mut bool| {
        if *inw {
            cur.push(std::mem::take(w));
            *inw = false;
        }
    };
    // A variable at i ($NAME or ${NAME}): how many chars it takes, when
    // it's a well-known one.
    let var = |i: usize| -> Option<usize> {
        let braced = b.get(i + 1) == Some(&'{');
        let start = if braced { i + 2 } else { i + 1 };
        let mut j = start;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == '_') {
            j += 1;
        }
        let name: String = b[start..j].iter().collect();
        if !WELL_KNOWN.contains(&name.as_str()) {
            return None;
        }
        if braced {
            (b.get(j) == Some(&'}')).then_some(j + 1 - i)
        } else {
            Some(j - i)
        }
    };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            '\'' => {
                let j = b[i + 1..].iter().position(|&x| x == '\'')?;
                w.extend(&b[i + 1..i + 1 + j]);
                inw = true;
                i += j + 2;
            }
            '"' => {
                let mut j = i + 1;
                loop {
                    match *b.get(j)? {
                        '"' => break,
                        '\\' => {
                            w.push(*b.get(j + 1)?);
                            j += 2;
                        }
                        '`' => return None,
                        '$' => {
                            let n = var(j)?;
                            w.extend(&b[j..j + n]);
                            j += n;
                        }
                        x => {
                            w.push(x);
                            j += 1;
                        }
                    }
                }
                inw = true;
                i = j + 1;
            }
            '\\' => {
                let n = *b.get(i + 1)?;
                if n == '\n' {
                    return None;
                }
                w.push(n);
                inw = true;
                i += 2;
            }
            '$' => {
                let n = var(i)?;
                w.extend(&b[i..i + n]);
                inw = true;
                i += n;
            }
            ' ' | '\t' => {
                push_word(&mut cur, &mut w, &mut inw);
                i += 1;
            }
            ';' => {
                push_word(&mut cur, &mut w, &mut inw);
                out.push(std::mem::take(&mut cur));
                i += 1;
            }
            '&' if b.get(i + 1) == Some(&'&') => {
                push_word(&mut cur, &mut w, &mut inw);
                out.push(std::mem::take(&mut cur));
                i += 2;
            }
            '&' | '|' | '>' | '<' | '(' | ')' | '{' | '}' | '`' | '\n' | '\r' => return None,
            _ => {
                w.push(c);
                inw = true;
                i += 1;
            }
        }
    }
    push_word(&mut cur, &mut w, &mut inw);
    out.push(cur);
    // An empty command (`a ;; b`, a trailing `&&`) isn't plain either,
    // but a trailing `;` is.
    if out.last().is_some_and(Vec::is_empty) && cmd.trim_end().ends_with(';') {
        out.pop();
    }
    if out.is_empty() || out.iter().any(Vec::is_empty) {
        return None;
    }
    Some(out)
}

/// Whether cmd only does what rules (Claude Code's Bash rules, as the
/// session was given them) allow, every command in it, its variables
/// only well-known ones (WELL_KNOWN).
pub fn reads_only(cmd: &str, rules: &[String]) -> bool {
    let Some(cmds) = simple_commands(cmd) else {
        return false;
    };
    let rules: Vec<(Vec<String>, bool)> = rules.iter().filter_map(|r| rule(r.as_str())).collect();
    cmds.iter().all(|words| {
        let first = words[0].as_str();
        if first.contains('=') || first.contains('$') {
            return false; // VAR=x cmd, or a command named by a variable
        }
        if PRINTS.contains(&first) {
            return true;
        }
        rules.iter().any(|(r, prefix)| {
            if *prefix {
                words.len() >= r.len() && words[..r.len()] == r[..]
            } else {
                words[..] == r[..]
            }
        })
    })
}
