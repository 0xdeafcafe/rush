//! What a call is doing, in words, for the one line a list row has:
//! "running pnpm test", "reading view.go", "searching for PrettyModel".
//! The one copy every screen says it with: an adapter with words of its own
//! for some of its tools (a Describer) says those, and this for the rest.
//! Port of internal/agent/tool/doing.go.

use super::{Call, Kind};

/// What a call is doing, in words. A command's own description wins when
/// the agent gave one.
pub fn doing(c: &Call) -> String {
    let inp = &c.input;
    let with = |verb: &str, what: &str| -> String {
        let what = one_line(what);
        if what.is_empty() {
            verb.to_owned()
        } else {
            format!("{verb} {what}")
        }
    };
    match c.kind {
        Kind::Shell => {
            let d = one_line(&inp.description);
            if !d.is_empty() {
                return lower_first(d);
            }
            return shell_doing(one_line(&inp.command));
        }
        Kind::Read => return with("reading", base(&inp.path)),
        Kind::Edit | Kind::Notebook => return with("editing", base(&inp.path)),
        Kind::Write => return with("writing", base(&inp.path)),
        Kind::Delete => return with("deleting", base(&inp.path)),
        Kind::Move => return with("moving", base(&inp.path)),
        Kind::Search => return with("searching for", &inp.pattern),
        Kind::Glob => return with("finding", &inp.pattern),
        Kind::Fetch => {
            return match url_host(one_line(&inp.url)) {
                Some(h) => format!("reading {}", h.strip_prefix("www.").unwrap_or(h)),
                None => "reading a web page".into(),
            };
        }
        Kind::WebSearch => return with("searching the web for", &inp.query),
        Kind::Subagent => return with("subagent:", &lower_first(one_line(&inp.description))),
        Kind::Todo => return "updating its todo list".into(),
        Kind::Question => {
            let t = one_line(&c.title);
            return if t.is_empty() {
                "asking you".into()
            } else {
                t.to_owned()
            };
        }
        Kind::PlanMode => return "has a plan for you".into(),
        Kind::Think => return "thinking".into(),
        Kind::Mcp => return format!("{} · {}", inp.tool.replace('_', " "), inp.server),
        Kind::Other => {}
    }
    let t = one_line(&c.title);
    if !t.is_empty() {
        return t.to_owned();
    }
    for v in [
        &inp.description,
        &inp.command,
        &inp.path,
        &inp.pattern,
        &inp.url,
        &inp.query,
        &inp.prompt,
    ] {
        let v = one_line(v);
        if !v.is_empty() {
            return format!("{} {v}", c.name);
        }
    }
    c.name.clone()
}

/// What a shell command does: "searching for X" for grep and rg, else
/// "running" and the program it runs with its first argument, past any cd,
/// env or variables in front of it: "running pnpm test".
pub fn shell_doing(cmd: &str) -> String {
    for part in split_any(cmd, &["&&", ";", "||"]) {
        let mut f: Vec<&str> = part.split_whitespace().collect();
        let skip = f
            .iter()
            .take_while(|w| w.contains('=') || matches!(**w, "env" | "sudo" | "time"))
            .count();
        f.drain(..skip);
        if f.is_empty() || f[0] == "cd" || f[0] == "export" {
            continue;
        }
        let prog = base(f[0]);
        if prog == "grep" || prog == "rg" {
            let pat = pattern(part);
            if !pat.is_empty() {
                return format!("searching for {pat}");
            }
            return "searching the code".into();
        }
        if f.len() > 1
            && !f[1].starts_with('-')
            && !f[1].contains(['"', '\'', '/', '$', '|', '<', '>'])
        {
            return format!("running {prog} {}", f[1]);
        }
        return format!("running {prog}");
    }
    "running a command".into()
}

/// grep's first argument that isn't a flag, quotes and all.
fn pattern(cmd: &str) -> &str {
    let rest = cmd.trim();
    pattern_args(rest.split_once(' ').map_or("", |(_, r)| r)) // past the program
}

fn pattern_args(mut rest: &str) -> &str {
    loop {
        rest = rest.trim();
        if rest.is_empty() {
            return "";
        }
        let q = rest.as_bytes()[0];
        if q == b'"' || q == b'\'' {
            return match rest[1..].find(q as char) {
                Some(end) => &rest[1..1 + end],
                None => "",
            };
        }
        let (word, mut after) = rest.split_once(' ').unwrap_or((rest, ""));
        if !word.starts_with('-') {
            return word;
        }
        if matches!(word, "-e" | "-A" | "-B" | "-C" | "-m" | "-g" | "-t") {
            if word == "-e" {
                return pattern_args(&rest[2..]);
            }
            after = after.trim().split_once(' ').map_or("", |(_, r)| r); // its value
        }
        rest = after;
    }
}

/// Splits on each separator in turn, as strings.Split on each would.
fn split_any<'a>(s: &'a str, seps: &[&str]) -> Vec<&'a str> {
    let mut parts = vec![s];
    for sep in seps {
        parts = parts.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    parts
}

/// s trimmed to its first line.
pub fn one_line(s: &str) -> &str {
    let s = s.trim();
    match s.find('\n') {
        Some(i) => s[..i].trim(),
        None => s,
    }
}

/// s with its first letter lower-cased, unless it opens an acronym ("PR",
/// "CI").
pub fn lower_first(s: &str) -> String {
    let mut it = s.chars();
    let (Some(a), b) = (it.next(), it.next()) else {
        return s.to_owned();
    };
    if b.is_some_and(|b| b.is_ascii_uppercase()) {
        return s.to_owned();
    }
    if a.is_ascii_uppercase() {
        let mut out = String::with_capacity(s.len());
        out.push(a.to_ascii_lowercase());
        out.push_str(&s[1..]);
        return out;
    }
    s.to_owned()
}

/// The last element of a path, as Go's filepath.Base; "" stays "".
pub fn base(p: &str) -> &str {
    if p.is_empty() {
        return "";
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/";
    }
    match t.rfind('/') {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// A URL's host (with its port), as Go's url.Parse finds it.
fn url_host(u: &str) -> Option<&str> {
    let rest = match u.find("://") {
        Some(i)
            if u[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            &u[i + 3..]
        }
        _ => u.strip_prefix("//")?,
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let h = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    (!h.is_empty()).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::super::Input;
    use super::*;

    // doing_test.go
    #[test]
    fn says_what_a_call_does() {
        let call = |kind, name: &str, title: &str, input: Input| Call {
            kind,
            name: name.into(),
            title: title.into(),
            input,
            ..Call::default()
        };
        let cases = [
            (
                call(
                    Kind::Shell,
                    "",
                    "",
                    Input {
                        command: "cd x && pnpm test".into(),
                        ..Input::default()
                    },
                ),
                "running pnpm test",
            ),
            (
                call(
                    Kind::Shell,
                    "",
                    "",
                    Input {
                        command: "rg -n foo".into(),
                        description: "Find foo".into(),
                        ..Input::default()
                    },
                ),
                "find foo",
            ),
            (
                call(
                    Kind::Delete,
                    "",
                    "",
                    Input {
                        path: "/a/b.go".into(),
                        ..Input::default()
                    },
                ),
                "deleting b.go",
            ),
            (
                call(Kind::Question, "", "Which one?\nmore", Input::default()),
                "Which one?",
            ),
            (
                call(
                    Kind::Mcp,
                    "",
                    "",
                    Input {
                        server: "linear".into(),
                        tool: "get_issue".into(),
                        ..Input::default()
                    },
                ),
                "get issue · linear",
            ),
            (
                call(Kind::Other, "update_plan", "Planning", Input::default()),
                "Planning",
            ),
            (call(Kind::Other, "frob", "", Input::default()), "frob"),
            (
                call(
                    Kind::Fetch,
                    "",
                    "",
                    Input {
                        url: "https://www.example.com/x".into(),
                        ..Input::default()
                    },
                ),
                "reading example.com",
            ),
        ];
        for (c, want) in cases {
            assert_eq!(doing(&c), want, "{c:?}");
        }
    }

    #[test]
    fn says_what_a_command_runs() {
        for (cmd, want) in [
            ("cd x && pnpm test", "running pnpm test"),
            ("GOFLAGS=-mod=mod go vet ./...", "running go vet"),
            (
                "grep -n \"func PrettyModel\" -A12 internal/ui",
                "searching for func PrettyModel",
            ),
            ("grep -e foo x", "searching for foo"),
            ("rg -n", "searching the code"),
            ("cd x", "running a command"),
            ("ls /tmp", "running ls"),
        ] {
            assert_eq!(shell_doing(cmd), want, "{cmd}");
        }
    }
}
