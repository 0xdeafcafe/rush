//! What a tool call is doing, in words, as a room's log has it: "running
//! pnpm test", "reading view.go", "searching for PrettyModel".
//! Ported from internal/agent/tool/doing.go:1-160, on rush_core's tool
//! call.

use rush_core::tool::{Call, Kind};

/// What a call is doing, for the one line a row has. A command's own
/// description wins when the agent gave one.
pub fn doing(c: &Call) -> String {
    let inp = &c.input;
    let with = |verb: &str, what: &str| -> String {
        let what = one_line(what);
        if what.is_empty() {
            verb.to_string()
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
            return match host(one_line(&inp.url)) {
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
                t.to_string()
            };
        }
        Kind::PlanMode => return "has a plan for you".into(),
        Kind::Think => return "thinking".into(),
        Kind::Mcp => return format!("{} · {}", inp.tool.replace('_', " "), inp.server),
        _ => {}
    }
    let t = one_line(&c.title);
    if !t.is_empty() {
        return t.to_string();
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
        while let Some(w) = f.first() {
            if w.contains('=') || matches!(*w, "env" | "sudo" | "time") {
                f.remove(0);
            } else {
                break;
            }
        }
        if f.is_empty() || f[0] == "cd" || f[0] == "export" {
            continue;
        }
        let prog = base(f[0]);
        if prog == "grep" || prog == "rg" {
            let pat = pattern(&part);
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
fn pattern(cmd: &str) -> String {
    let rest = cmd.trim();
    let mut rest = rest.split_once(' ').map_or("", |(_, r)| r); // the program
    loop {
        rest = rest.trim();
        if rest.is_empty() {
            return String::new();
        }
        let q = rest.as_bytes()[0];
        if q == b'"' || q == b'\'' {
            return match rest[1..].find(q as char) {
                Some(end) => rest[1..1 + end].to_string(),
                None => String::new(),
            };
        }
        let (word, mut after) = rest.split_once(' ').unwrap_or((rest, ""));
        if !word.starts_with('-') {
            return word.to_string();
        }
        if matches!(word, "-e" | "-A" | "-B" | "-C" | "-m" | "-g" | "-t") {
            after = after.trim().split_once(' ').map_or("", |(_, r)| r); // its value
            if word == "-e" {
                return pattern(&format!(
                    "grep {}",
                    rest.strip_prefix("-e").unwrap_or(rest).trim()
                ));
            }
        }
        rest = after;
    }
}

fn split_any(s: &str, seps: &[&str]) -> Vec<String> {
    let mut parts = vec![s.to_string()];
    for sep in seps {
        parts = parts
            .iter()
            .flat_map(|p| p.split(sep).map(String::from).collect::<Vec<_>>())
            .collect();
    }
    parts
}

fn one_line(s: &str) -> &str {
    let s = s.trim();
    match s.find('\n') {
        Some(i) => s[..i].trim(),
        None => s,
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    let Some(first) = c.next() else {
        return String::new();
    };
    if c.clone().next().is_some_and(|r| r.is_ascii_uppercase()) {
        return s.to_string(); // an acronym: "PR", "CI"
    }
    if first.is_ascii_uppercase() {
        return first.to_ascii_lowercase().to_string() + c.as_str();
    }
    s.to_string()
}

/// The last element of a path, as Go's filepath.Base; "" stays "".
fn base(p: &str) -> &str {
    if p.is_empty() {
        return "";
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/";
    }
    t.rsplit('/').next().unwrap_or(t)
}

/// A URL's host, as Go's url.Parse finds one: after a scheme's "//", up
/// to the path, port and all.
fn host(u: &str) -> Option<&str> {
    let (scheme, rest) = u.split_once("://")?;
    if scheme.is_empty()
        || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let h = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    (!h.is_empty()).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::tool::Input;

    fn call(kind: Kind, f: impl FnOnce(&mut Input)) -> Call {
        let mut c = Call {
            kind,
            name: "Tool".into(),
            ..Call::default()
        };
        f(&mut c.input);
        c
    }

    #[test]
    fn says_what_calls_do() {
        let cases = [
            (
                call(Kind::Shell, |i| i.command = "cd x && pnpm test".into()),
                "running pnpm test",
            ),
            (
                call(Kind::Shell, |i| i.description = "Run the tests".into()),
                "run the tests",
            ),
            (
                call(Kind::Shell, |i| i.description = "PR checks".into()),
                "PR checks",
            ),
            (
                call(Kind::Shell, |i| {
                    i.command = "rg -n 'Pretty Model' src".into()
                }),
                "searching for Pretty Model",
            ),
            (
                call(Kind::Shell, |i| i.command = "grep -e foo x".into()),
                "searching for foo",
            ),
            (
                call(Kind::Shell, |i| i.command = "FOO=1 go".into()),
                "running go",
            ),
            (
                call(Kind::Read, |i| i.path = "/a/view.go".into()),
                "reading view.go",
            ),
            (call(Kind::Read, |_| {}), "reading"),
            (
                call(Kind::Fetch, |i| i.url = "https://www.example.com/x".into()),
                "reading example.com",
            ),
            (
                call(Kind::Fetch, |i| i.url = "nope".into()),
                "reading a web page",
            ),
            (
                call(Kind::Mcp, |i| {
                    i.tool = "get_issue".into();
                    i.server = "linear".into()
                }),
                "get issue · linear",
            ),
            (call(Kind::Other, |i| i.command = "x y".into()), "Tool x y"),
            (call(Kind::Other, |_| {}), "Tool"),
        ];
        for (c, want) in cases {
            assert_eq!(doing(&c), want, "{c:?}");
        }
    }
}
