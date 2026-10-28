//! A tool call said in words, for the one line a list row has: "running
//! pnpm test", "reading view.go", "searching for PrettyModel". Port of
//! internal/adapters/claude/claude/doing.go, tools.go:14-125 (Call, the
//! fields Doing reads) and internal/agent/tool/doing.go.

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::json::str_or_empty;

/// What a tool call does (Go's tool.Kind), as far as Doing tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Other,
    Shell,
    Read,
    Edit,
    Write,
    Delete,
    Move,
    Search,
    Glob,
    Fetch,
    WebSearch,
    Subagent,
    Todo,
    Question,
    PlanMode,
    Mcp,
    Notebook,
    Think,
}

/// What each of Claude Code's tools does.
pub fn kind_of(name: &str) -> Kind {
    match name {
        "Bash" => Kind::Shell,
        "Read" => Kind::Read,
        "Edit" | "MultiEdit" => Kind::Edit,
        "Write" => Kind::Write,
        "NotebookEdit" => Kind::Notebook,
        "Grep" => Kind::Search,
        "Glob" => Kind::Glob,
        "WebFetch" => Kind::Fetch,
        "WebSearch" => Kind::WebSearch,
        "Task" | "Agent" => Kind::Subagent,
        "TodoWrite" => Kind::Todo,
        "AskUserQuestion" => Kind::Question,
        "ExitPlanMode" => Kind::PlanMode,
        _ => match name.strip_prefix("mcp__") {
            Some(rest) if rest.contains("__") => Kind::Mcp,
            _ => Kind::Other,
        },
    }
}

/// A call as Doing reads it: what it works on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    pub kind: Option<Kind>,
    pub title: String,
    pub command: String,
    pub description: String,
    pub path: String,
    pub pattern: String,
    pub query: String,
    pub url: String,
    pub prompt: String,
    pub server: String,
    pub tool: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct In<'a> {
    #[serde(borrow)]
    command: Option<&'a RawValue>,
    #[serde(borrow)]
    description: Option<&'a RawValue>,
    #[serde(borrow)]
    file_path: Option<&'a RawValue>,
    #[serde(borrow)]
    notebook_path: Option<&'a RawValue>,
    #[serde(borrow)]
    path: Option<&'a RawValue>,
    #[serde(borrow)]
    pattern: Option<&'a RawValue>,
    #[serde(borrow)]
    query: Option<&'a RawValue>,
    #[serde(borrow)]
    url: Option<&'a RawValue>,
    #[serde(borrow)]
    prompt: Option<&'a RawValue>,
    #[serde(borrow)]
    questions: Option<&'a RawValue>,
    #[serde(borrow)]
    skill: Option<&'a RawValue>,
    #[serde(borrow)]
    summary: Option<&'a RawValue>,
}

fn input(raw: &str) -> In<'_> {
    serde_json::from_str(raw).unwrap_or_default()
}

/// One of Claude Code's tool calls as rush's own, the fields Doing reads.
pub fn call(name: &str, raw: &str) -> Call {
    let in_ = input(raw);
    let s = |r| str_or_empty(r).into_owned();
    let mut c = Call {
        name: name.to_string(),
        kind: Some(kind_of(name)),
        description: s(in_.description),
        prompt: s(in_.prompt),
        ..Call::default()
    };
    // As Go's Call: an mcp tool is Other until its name is split.
    let k = match kind_of(name) {
        Kind::Mcp => Kind::Other,
        k => k,
    };
    match k {
        Kind::Shell => c.command = s(in_.command),
        Kind::Read | Kind::Edit | Kind::Write => c.path = s(in_.file_path),
        Kind::Notebook => c.path = s(in_.notebook_path),
        Kind::Search | Kind::Glob => {
            c.pattern = s(in_.pattern);
            c.path = s(in_.path);
        }
        Kind::Fetch => c.url = s(in_.url),
        Kind::WebSearch => c.query = s(in_.query),
        Kind::Question => {
            #[derive(Deserialize)]
            struct Q<'a> {
                #[serde(borrow)]
                question: Option<&'a RawValue>,
            }
            if let Some(q) = in_.questions
                && let Ok(qs) = serde_json::from_str::<Vec<Q>>(q.get())
                && let Some(first) = qs.first()
            {
                c.title = s(first.question);
            }
        }
        Kind::Other => {
            if let Some(rest) = name.strip_prefix("mcp__")
                && let Some((server, t)) = rest.split_once("__")
            {
                c.kind = Some(Kind::Mcp);
                c.server = server.strip_prefix("claude_ai_").unwrap_or(server).into();
                c.tool = t.into();
            } else {
                c.kind = Some(Kind::Other);
                c.command = s(in_.command);
                c.path = s(in_.file_path);
                c.pattern = s(in_.pattern);
                c.url = s(in_.url);
                c.query = s(in_.query);
            }
        }
        _ => {}
    }
    c
}

/// What a Claude Code tool call is doing, in words. A command's own
/// description wins when Claude gave one.
pub fn doing(name: &str, raw: &str) -> String {
    match name {
        "BashOutput" | "TaskOutput" => "checking a background task".into(),
        "KillShell" | "TaskStop" => "stopping a background task".into(),
        "TaskCreate" | "TaskUpdate" => "updating its todo list".into(),
        "Skill" => match str_or_empty(input(raw).skill).as_ref() {
            "" => "using".into(),
            s => format!("using {s}"),
        },
        "ToolSearch" => "looking up tools".into(),
        "ScheduleWakeup" | "Monitor" => "waiting".into(),
        "SendMessage" => match str_or_empty(input(raw).summary).as_ref() {
            "" => "messaging".into(),
            s => format!("messaging {s}"),
        },
        "SubagentHandback" => "reporting back".into(),
        "ListAgents" => "listing agents".into(),
        "PushNotification" => "notifying you".into(),
        "EnterWorktree" => "entering a worktree".into(),
        _ => doing_call(&call(name, raw)),
    }
}

/// What a call is doing, whichever agent made it (Go's tool.Doing).
pub fn doing_call(c: &Call) -> String {
    let with = |verb: &str, what: &str| {
        let w = one_line(what);
        if w.is_empty() {
            verb.to_string()
        } else {
            format!("{verb} {w}")
        }
    };
    match c.kind.unwrap_or(Kind::Other) {
        Kind::Shell => {
            let d = one_line(&c.description);
            if !d.is_empty() {
                return lower_first(d);
            }
            shell_doing(one_line(&c.command))
        }
        Kind::Read => with("reading", base(&c.path)),
        Kind::Edit | Kind::Notebook => with("editing", base(&c.path)),
        Kind::Write => with("writing", base(&c.path)),
        Kind::Delete => with("deleting", base(&c.path)),
        Kind::Move => with("moving", base(&c.path)),
        Kind::Search => with("searching for", &c.pattern),
        Kind::Glob => with("finding", &c.pattern),
        Kind::Fetch => match host(one_line(&c.url)) {
            Some(h) => format!("reading {}", h.strip_prefix("www.").unwrap_or(h)),
            None => "reading a web page".into(),
        },
        Kind::WebSearch => with("searching the web for", &c.query),
        Kind::Subagent => with("subagent:", &lower_first(one_line(&c.description))),
        Kind::Todo => "updating its todo list".into(),
        Kind::Question => match one_line(&c.title) {
            "" => "asking you".into(),
            t => t.into(),
        },
        Kind::PlanMode => "has a plan for you".into(),
        Kind::Think => "thinking".into(),
        Kind::Mcp => format!("{} · {}", c.tool.replace('_', " "), c.server),
        Kind::Other => {
            let t = one_line(&c.title);
            if !t.is_empty() {
                return t.into();
            }
            for v in [
                &c.description,
                &c.command,
                &c.path,
                &c.pattern,
                &c.url,
                &c.query,
                &c.prompt,
            ] {
                let v = one_line(v);
                if !v.is_empty() {
                    return format!("{} {v}", c.name);
                }
            }
            c.name.clone()
        }
    }
}

/// What a shell command does: "searching for X" for grep and rg, else
/// "running" and the program it runs with its first argument, past any
/// cd, env or variables in front of it: "running pnpm test".
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
    pattern_args(rest.split_once(' ').map(|(_, r)| r).unwrap_or("")) // past the program
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
            after = after.trim().split_once(' ').map(|(_, r)| r).unwrap_or(""); // its value
        }
        rest = after;
    }
}

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

fn lower_first(s: &str) -> String {
    let mut it = s.chars();
    let (Some(a), b) = (it.next(), it.next()) else {
        return s.to_string();
    };
    if b.is_some_and(|b| b.is_ascii_uppercase()) {
        return s.to_string(); // an acronym: "PR", "CI"
    }
    if a.is_ascii_uppercase() {
        let mut out = String::with_capacity(s.len());
        out.push(a.to_ascii_lowercase());
        out.push_str(&s[1..]);
        return out;
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
    match t.rfind('/') {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// A URL's host (with its port), as Go's url.Parse finds it.
fn host(u: &str) -> Option<&str> {
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
    let h = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
    (!h.is_empty()).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_doing() {
        for (tool, input, want) in [
            (
                "Bash",
                r#"{"command":"cd /Users/lw/Source/github.com/langwatch/langwatch && pnpm test"}"#,
                "running pnpm test",
            ),
            (
                "Bash",
                r#"{"command":"grep -rn \"agtools\\.\" internal","description":"Find agtools uses"}"#,
                "find agtools uses",
            ),
            (
                "Bash",
                r#"{"command":"GOFLAGS=-mod=mod go vet ./..."}"#,
                "running go vet",
            ),
            (
                "Bash",
                r#"{"command":"grep -n \"func PrettyModel\" -A12 internal/ui"}"#,
                "searching for func PrettyModel",
            ),
            ("Bash", r#"{"description":"PR checks"}"#, "PR checks"),
            (
                "Bash",
                r#"{"command":"grep -rn \"agtools\\.\\|mcp\" internal"}"#,
                r"searching for agtools\.\|mcp",
            ),
            ("Bash", r#"{"command":"cd x"}"#, "running a command"),
            ("Read", r#"{"file_path":"/a/b/view.go"}"#, "reading view.go"),
            (
                "Grep",
                r#"{"pattern":"func PrettyModel"}"#,
                "searching for func PrettyModel",
            ),
            (
                "WebFetch",
                r#"{"url":"https://www.example.com/x"}"#,
                "reading example.com",
            ),
            (
                "Agent",
                r#"{"description":"Explore the repo"}"#,
                "subagent: explore the repo",
            ),
            (
                "mcp__claude_ai_Notion__notion-search",
                "{}",
                "notion-search · Notion",
            ),
            ("Frobnicate", r#"{"query":"x"}"#, "Frobnicate x"),
            ("Skill", r#"{"skill":"loop"}"#, "using loop"),
            ("SendMessage", "{}", "messaging"),
            (
                "AskUserQuestion",
                r#"{"questions":[{"question":"Which one?\nmore"}]}"#,
                "Which one?",
            ),
            (
                "Bash",
                r#"{"command":"grep -e foo x"}"#,
                "searching for foo",
            ),
            ("Bash", r#"{"command":"rg -n"}"#, "searching the code"),
        ] {
            assert_eq!(doing(tool, input), want, "doing({tool}, {input})");
        }
    }

    #[test]
    fn tool_doing() {
        let c = |kind, f: &dyn Fn(&mut Call)| {
            let mut c = Call {
                kind: Some(kind),
                ..Call::default()
            };
            f(&mut c);
            doing_call(&c)
        };
        assert_eq!(
            c(Kind::Shell, &|c| c.command = "cd x && pnpm test".into()),
            "running pnpm test"
        );
        assert_eq!(
            c(Kind::Shell, &|c| {
                c.command = "rg -n foo".into();
                c.description = "Find foo".into();
            }),
            "find foo"
        );
        assert_eq!(
            c(Kind::Delete, &|c| c.path = "/a/b.go".into()),
            "deleting b.go"
        );
        assert_eq!(
            c(Kind::Question, &|c| c.title = "Which one?\nmore".into()),
            "Which one?"
        );
        assert_eq!(
            c(Kind::Mcp, &|c| {
                c.server = "linear".into();
                c.tool = "get_issue".into();
            }),
            "get issue · linear"
        );
        assert_eq!(
            c(Kind::Other, &|c| {
                c.name = "update_plan".into();
                c.title = "Planning".into();
            }),
            "Planning"
        );
        assert_eq!(c(Kind::Other, &|c| c.name = "frob".into()), "frob");
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("MultiEdit"), Kind::Edit);
        assert_eq!(kind_of("mcp__linear__get_issue"), Kind::Mcp);
        assert_eq!(kind_of("mcp__x"), Kind::Other);
        assert_eq!(kind_of("ScheduleWakeup"), Kind::Other);
    }
}
