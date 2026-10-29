//! Another agent a shell command runs: claude -p, codex exec, copilot -p,
//! rush session start. Read from the command it's only a hint, which
//! `rush spawn` and the Session take apart the same way. Port of Go's
//! internal/convo/spawn.go:14-460 (Spawn, spawnNot, spawnSub,
//! spawnValued, spawnPrint, SpawnOf :64, shellCmd :98, closeQuote :183,
//! closeParen :198, withDir, piped, catRe, spawnIn :240, rushStart :365,
//! quoted, promptOf :425, heredoc :437, unquoteArg :447).

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use rush_core::adapter::{LEGACY_KIND, Registry};

use super::cards::arg_value;
use super::shell::{is_assign, segments};

/// Another agent a shell command ran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spawn {
    pub kind: String,
    /// What the agent is called: Codex, Claude Code.
    pub name: String,
    /// What it was asked; "" when it came from a file or a pipe.
    pub prompt: String,
    /// The file its prompt was fed from, when it was.
    pub from: String,
    pub model: String,
    /// Where it ran, when the command went somewhere first.
    pub dir: String,
    /// The run a harness's own subagent call names, when its harness keeps
    /// it apart.
    pub child: String,
}

/// First words that make an agent's program do something other than work.
const SPAWN_NOT: &[&str] = &[
    "mcp", "config", "doctor", "update", "upgrade", "install", "login", "logout", "auth", "plugin",
    "plugins", "setup-token", "migrate-installer", "completion", "features", "apply", "app-server",
    "mcp-server", "sandbox", "debug", "cloud", "models", "version", "help", "agents", "acp",
    "--version", "-v", "-V", "--help", "-h", "--acp",
];

/// First words that start a run and name nothing else.
const SPAWN_SUB: &[&str] = &["exec", "e", "run"];

/// Flags that take a value, which isn't the prompt.
const SPAWN_VALUED: &[&str] = &[
    "--model", "-m", "--output-format", "--input-format", "--append-system-prompt",
    "--system-prompt", "--allowedTools", "--allowed-tools", "--disallowedTools",
    "--disallowed-tools", "--permission-mode", "--add-dir", "--max-turns", "--resume", "-r",
    "--session-id", "--settings", "--mcp-config", "--agents", "--agent", "--fallback-model",
    "--effort", "-C", "--cd", "-c", "--config", "-s", "--sandbox", "-i", "--image", "--profile",
    "-a", "--ask-for-approval", "--output-last-message", "-o", "--color", "--output-schema",
    "--log-level", "--allow-tool", "--deny-tool", "--betas", "--setting-sources", "--plugin-dir",
    "--json-schema", "--max-budget-usd", "--permission-prompt-tool", "--permission-prompts",
    "--name", "-n", "--reasoning-effort", "--agent-file", "--workdir", "--dir",
];

/// Flags that run the agent once and print.
const SPAWN_PRINT: &[&str] = &["-p", "--print", "--prompt"];

/// A prompt read from a file as it's given: "$(cat prompt.md)".
static CAT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^"?\$\((?:cat|<)\s+("[^"]+"|'[^']+'|[^\s)]+)\s*\)"?$"#).expect("catRe")
});

/// A prompt fed in by $(cat <<'EOF' …) (cards.go heredocArg).
static HEREDOC_ARG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^"?\$\(cat\s*<<-?\s*['"]?(\w+)['"]?"#).expect("heredocArg"));

/// The agent cmd runs, when it runs one: a registered agent's program,
/// asked to work rather than to set up or report (SpawnOf).
pub fn spawn_of(reg: &Registry, cmd: &str) -> Option<Spawn> {
    let segs = segments(&cmd.trim().replace("\\\n", " "));
    let mut dir = String::new();
    for s in &segs {
        let c = shell_cmd(&s.text);
        if c.words.len() >= 2 && c.words[0] == "cd" {
            dir = unquote_arg(&c.words[1]);
        }
        if let Some(sp) = spawn_in(reg, &c, &s.body, "") {
            return Some(with_dir(sp, &dir));
        }
        // Fed a prompt through a pipe: echo "…" | claude -p.
        for f in &s.filters {
            if let Some(mut sp) = spawn_in(reg, &shell_cmd(f), &[], &piped(&c.words)) {
                if sp.prompt.is_empty()
                    && sp.from.is_empty()
                    && c.words.len() >= 2
                    && base(&c.words[0]) == "cat"
                {
                    sp.from = unquote_arg(&c.words[1]);
                }
                return Some(with_dir(sp, &dir));
            }
        }
    }
    None
}

fn base(p: &str) -> &str {
    Path::new(p)
        .file_name()
        .and_then(|b| b.to_str())
        .unwrap_or(p)
}

/// One simple command as the shell reads it (command).
#[derive(Default, Debug)]
struct Command {
    words: Vec<String>,
    /// A file fed with <.
    stdin: String,
    /// A heredoc fed with <<.
    tag: String,
}

/// The first simple command of s: up to an unquoted ;, &, | or newline,
/// its redirects taken out (shellCmd).
fn shell_cmd(s: &str) -> Command {
    let b = s.as_bytes();
    let mut c = Command::default();
    let mut cur: Vec<u8> = Vec::new();
    let mut in_word = false;
    let mut redir = String::new();
    let flush = |c: &mut Command, cur: &mut Vec<u8>, in_word: &mut bool, redir: &mut String| {
        if !*in_word {
            return;
        }
        let w = String::from_utf8_lossy(cur).into_owned();
        cur.clear();
        *in_word = false;
        match redir.as_str() {
            "<" => c.stdin = unquote_arg(&w),
            "<<" => c.tag = w.trim_matches(['\'', '"']).to_string(),
            "" => c.words.push(w),
            _ => {}
        }
        redir.clear();
    };
    let mut i = 0;
    while i < b.len() {
        let ch = b[i];
        match ch {
            b'\\' if i + 1 < b.len() => {
                cur.push(ch);
                cur.push(b[i + 1]);
                i += 1;
                in_word = true;
            }
            b'\'' => {
                let j = b[i + 1..]
                    .iter()
                    .position(|&x| x == b'\'')
                    .unwrap_or(b.len() - i - 1);
                cur.extend_from_slice(&b[i..b.len().min(i + j + 2)]);
                i += j + 1;
                in_word = true;
            }
            b'"' => {
                let j = close_quote(b, i);
                cur.extend_from_slice(&b[i..j]);
                i = j - 1;
                in_word = true;
            }
            b'$' if i + 1 < b.len() && b[i + 1] == b'(' => {
                let j = close_paren(b, i + 1);
                cur.extend_from_slice(&b[i..j]);
                i = j - 1;
                in_word = true;
            }
            b' ' | b'\t' => flush(&mut c, &mut cur, &mut in_word, &mut redir),
            b';' | b'|' | b'\n' => {
                flush(&mut c, &mut cur, &mut in_word, &mut redir);
                return c;
            }
            b'&' if !(i + 1 < b.len() && b[i + 1] == b'>') => {
                flush(&mut c, &mut cur, &mut in_word, &mut redir);
                return c;
            }
            b'<' | b'>' | b'&' => {
                flush(&mut c, &mut cur, &mut in_word, &mut redir);
                let mut op = String::from(ch as char);
                while i + 1 < b.len() && b"<>&".contains(&b[i + 1]) {
                    i += 1;
                    op.push(b[i] as char);
                }
                // 2>&1 and >&2 are whole in themselves.
                if op.ends_with('&') && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
                    i += 2;
                    redir.clear();
                    continue;
                }
                redir = match op.as_str() {
                    "<" | "<<" => op,
                    "<<-" => "<<".into(),
                    _ => ">".into(),
                };
                // What's written to is no word of the command.
                in_word = false;
            }
            b'0'..=b'9' if !in_word && i + 1 < b.len() && b[i + 1] == b'>' => {
                // the 2 of 2>
            }
            _ => {
                cur.push(ch);
                in_word = true;
            }
        }
        i += 1;
    }
    flush(&mut c, &mut cur, &mut in_word, &mut redir);
    c
}

/// Just past the " that closes the one at i, past any $(…) inside it and
/// its own quotes (closeQuote).
fn close_quote(s: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < s.len() {
        match s[j] {
            b'\\' => j += 1,
            b'$' if j + 1 < s.len() && s[j + 1] == b'(' => j = close_paren(s, j + 1) - 1,
            b'"' => return j + 1,
            _ => {}
        }
        j += 1;
    }
    s.len()
}

/// Just past the ) that closes the ( at i (closeParen).
fn close_paren(s: &[u8], i: usize) -> usize {
    let mut depth = 0i32;
    let mut j = i;
    while j < s.len() {
        match s[j] {
            b'\\' => j += 1,
            b'\'' => {
                if let Some(k) = s[j + 1..].iter().position(|&x| x == b'\'') {
                    j += k + 1;
                }
            }
            b'"' => j = close_quote(s, j) - 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    s.len()
}

fn with_dir(mut sp: Spawn, dir: &str) -> Spawn {
    if sp.dir.is_empty() {
        sp.dir = dir.to_string();
    }
    sp
}

/// What echo or printf writes into a pipe: a prompt given that way.
fn piped(words: &[String]) -> String {
    if words.len() >= 2 && (words[0] == "echo" || words[0] == "printf") {
        for w in &words[1..] {
            if !w.starts_with('-') {
                return unquote_arg(w);
            }
        }
    }
    String::new()
}

/// One command read as an agent run: its program (past any runner such
/// as timeout or npx), then its prompt and model (spawnIn).
fn spawn_in(reg: &Registry, c: &Command, body: &[String], stdin: &str) -> Option<Spawn> {
    let mut words: &[String] = &c.words;
    while !words.is_empty() && is_assign(&words[0]) {
        words = &words[1..];
    }
    while words.len() > 1 {
        match base(&words[0]) {
            "timeout" | "gtimeout" => {
                words = &words[1..];
                while words.len() > 1 && words[0].starts_with('-') {
                    words = &words[1..];
                }
                if words.len() > 1 {
                    words = &words[1..]; // the duration
                }
                continue;
            }
            "env" | "nohup" | "time" | "exec" | "command" | "caffeinate" | "npx" | "bunx"
            | "pnpx" => {
                words = &words[1..];
                while words.len() > 1 && (words[0].starts_with('-') || is_assign(&words[0])) {
                    words = &words[1..];
                }
                continue;
            }
            _ => {}
        }
        break;
    }
    if words.is_empty() {
        return None;
    }
    if words.len() >= 3 && base(&words[0]) == "rush" && words[1] == "session" && words[2] == "start"
    {
        return rush_start(reg, &words[3..], c, body, stdin);
    }
    let k = reg.program_kind(&unquote_arg(&words[0]))?;
    let mut sp = Spawn {
        kind: k.as_str().to_string(),
        name: reg.get(k.as_str()).map_or_else(|| k.as_str().to_string(), |a| a.name()),
        ..Spawn::default()
    };
    let args = &words[1..];
    let (mut print, mut sub, mut from_stdin, mut streamed) = (false, false, false, false);
    // Bare words after a flag rush doesn't know, which may be its value.
    let mut loose: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // The subcommand is the first bare word, past any global flags.
        let first =
            !sub && !print && sp.prompt.is_empty() && sp.from.is_empty() && loose.is_empty();
        if a == "--help" || a == "-h" || a == "--version" {
            return None; // it reports, wherever it's asked to
        } else if SPAWN_NOT.contains(&a) && sp.prompt.is_empty() && !sub {
            return None;
        } else if SPAWN_SUB.contains(&a) && first {
            sub = true;
        } else if SPAWN_PRINT.contains(&a) {
            print = true;
        } else if a == "-" {
            from_stdin = true;
        } else if a.starts_with('-') {
            let (name, val, eq) = match a.split_once('=') {
                Some((n, v)) => (n, v, true),
                None => (a, "", false),
            };
            let next = if eq {
                val
            } else {
                args.get(i + 1).map_or("", String::as_str)
            };
            match name {
                "--model" | "-m" => sp.model = unquote_arg(next),
                "-C" | "--cd" | "--workdir" | "--dir" => sp.dir = unquote_arg(next),
                "--input-format" => streamed = unquote_arg(next) == "stream-json",
                _ => {}
            }
            if eq {
            } else if SPAWN_VALUED.contains(&name) {
                i += 1;
            } else if name.starts_with("--")
                && !next.is_empty()
                && !next.starts_with('-')
                && !quoted(next)
                && (!first || !SPAWN_SUB.contains(&next))
            {
                loose.push(next);
                i += 1;
            }
        } else if sp.prompt.is_empty() && sp.from.is_empty() {
            if let Some(m) = CAT_RE.captures(a) {
                sp.from = unquote_arg(&m[1]);
            } else {
                sp.prompt = prompt_of(a, body);
            }
        }
        i += 1;
    }
    // A bare word after a flag rush doesn't know is the prompt only when
    // nothing else is.
    if sp.prompt.is_empty()
        && sp.from.is_empty()
        && !from_stdin
        && c.stdin.is_empty()
        && c.tag.is_empty()
        && stdin.is_empty()
        && let Some(l) = loose.last()
    {
        sp.prompt = unquote_arg(l);
    }
    if sp.prompt.is_empty() && sp.from.is_empty() {
        fed(&mut sp, c, body, stdin, false);
    }
    // Messages as JSON aren't a prompt to show.
    if streamed {
        sp.prompt.clear();
    }
    // A bare program with nothing asked of it opens its own screen, and
    // codex needs exec to run without one.
    if sp.kind == "codex" && !sub {
        return None;
    }
    if !print && !sub && sp.prompt.is_empty() && sp.from.is_empty() {
        return None;
    }
    Some(sp)
}

/// A prompt fed on stdin: through a pipe, a heredoc or a file (any file
/// but /dev/null unless any).
fn fed(sp: &mut Spawn, c: &Command, body: &[String], stdin: &str, any: bool) {
    if !stdin.is_empty() {
        sp.prompt = stdin.to_string();
    } else if !c.tag.is_empty() {
        sp.prompt = heredoc(body, &c.tag).join("\n").trim().to_string();
    } else if !c.stdin.is_empty() && (any || c.stdin != "/dev/null") {
        sp.from = c.stdin.clone();
    }
}

/// rush session start's args as the agent it starts (rushStart).
fn rush_start(
    reg: &Registry,
    args: &[String],
    c: &Command,
    body: &[String],
    stdin: &str,
) -> Option<Spawn> {
    let mut sp = Spawn {
        kind: LEGACY_KIND.into(),
        ..Spawn::default()
    };
    if args.iter().any(|a| a == "--resume") {
        return None; // one it had already
    }
    for i in 0..args.len().saturating_sub(1) {
        let (name, val) = match args[i].split_once('=') {
            Some((n, v)) => (n, v),
            None => (args[i].as_str(), args[i + 1].as_str()),
        };
        let v = unquote_arg(val);
        match name {
            "--agent" => sp.kind = v,
            "--model" => sp.model = v,
            "--cwd" => sp.dir = v,
            "--prompt-file" => sp.from = v,
            _ => {}
        }
    }
    if sp.from == "-" {
        sp.from.clear();
        fed(&mut sp, c, body, stdin, true);
    }
    sp.name = reg
        .get(&sp.kind)
        .map_or_else(|| sp.kind.clone(), |a| a.name());
    Some(sp)
}

fn quoted(w: &str) -> bool {
    w.starts_with('"') || w.starts_with('\'')
}

/// A prompt as written: quoted, or fed in by $(cat <<'EOF' …) (promptOf).
fn prompt_of(w: &str, body: &[String]) -> String {
    if let Some(m) = HEREDOC_ARG_RE.captures(w) {
        // Quoted, the heredoc is part of the word; bare, it's the lines
        // that follow the command's.
        if w.contains('\n')
            && let Some(v) = arg_value(w)
        {
            return v.trim().to_string();
        }
        return heredoc(body, &m[1]).join("\n").trim().to_string();
    }
    unquote_arg(w)
}

/// A heredoc's lines, up to its tag.
fn heredoc<'a>(body: &'a [String], tag: &str) -> &'a [String] {
    for (i, l) in body.iter().enumerate() {
        let t = l.trim();
        if t == tag || t.starts_with(&format!("{tag})")) {
            return &body[..i];
        }
    }
    body
}

/// A word as the program gets it, escapes and all (unquoteArg).
pub fn unquote_arg(w: &str) -> String {
    let b = w.as_bytes();
    if b.len() >= 2
        && (b[0] == b'"' || b[0] == b'\'')
        && let Some(v) = arg_value(w)
    {
        return v;
    }
    w.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::{Adapter, Features, Kind, Level, Profile, Programmer};

    struct Prog(&'static str);

    impl Adapter for Prog {
        fn kind(&self) -> Kind {
            Kind::from(self.0)
        }
        fn name(&self) -> String {
            self.0.to_string()
        }
        fn features(&self) -> Features {
            Features::new()
        }
        fn level(&self) -> Level {
            Level::Preview
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        fn programmer(&self) -> Option<&dyn Programmer> {
            Some(self)
        }
    }

    impl Programmer for Prog {
        fn program(&self) -> (String, Vec<String>) {
            (self.0.to_string(), Vec::new())
        }
    }

    const CODEX_CHECK: &str = "command -v codex; codex --version; codex exec --help | head -40; grep model ~/.codex/config.toml";

    // spawn_test.go:16 TestSpawnOf: (cmd, kind, prompt, from, dir, model);
    // kind "" is no spawn.
    #[test]
    fn spawn_of_is_gos() {
        let reg = Registry::new();
        for k in ["claude", "codex", "copilot"] {
            reg.register(Prog(k));
        }
        let cases: &[(&str, &str, &str, &str, &str, &str)] = &[
            (r#"claude -p "fix the flaky test" --model sonnet"#, "claude", "fix the flaky test", "", "", "sonnet"),
            (r#"claude --print --output-format json 'say hi'"#, "claude", "say hi", "", "", ""),
            (r#"cd /work/app && timeout 600 claude -p "review it""#, "claude", "review it", "", "/work/app", ""),
            ("claude -p \"$(cat <<'EOF'\nread the spec\nand plan it\nEOF\n)\"", "claude", "read the spec\nand plan it", "", "", ""),
            (r#"echo "summarise the diff" | claude -p"#, "claude", "summarise the diff", "", "", ""),
            ("cat prompt.md | claude -p", "claude", "", "prompt.md", "", ""),
            ("claude -p < prompt.md", "claude", "", "prompt.md", "", ""),
            (r#"codex exec -m gpt-5 -C /work/x "port it to rust""#, "codex", "port it to rust", "", "/work/x", "gpt-5"),
            (r#"codex exec --full-auto "tidy up" 2>&1 | tail -20"#, "codex", "tidy up", "", "", ""),
            (r#"codex --search exec "look it up""#, "codex", "look it up", "", "", ""),
            (r#"codex -m gpt-5 exec "tidy up""#, "codex", "tidy up", "", "", "gpt-5"),
            (r#"copilot -p "explain main.go" --allow-all-tools"#, "copilot", "explain main.go", "", "", ""),
            ("CLAUDE_CONFIG_DIR=~/.claude-2 claude -p hi", "claude", "hi", "", "", ""),
            ("nohup codex exec --sandbox read-only - \\\n  < \"$JOB/tmp/l1-prompt.md\" > \"$JOB/l1.log\" 2>&1 &", "codex", "", "$JOB/tmp/l1-prompt.md", "", ""),
            (r#"nohup claude -p "$(cat "$JOB/tmp/port-prompt.md")" --model claude-opus-4-6 > port.log 2>&1 &"#, "claude", "", "$JOB/tmp/port-prompt.md", "", "claude-opus-4-6"),
            (r#"nohup codex exec -m $M -c model_reasoning_effort="$E" "$(cat $T/lanes/$L.txt)" > $T/$L.log 2>&1 < /dev/null &"#, "codex", "", "$T/lanes/$L.txt", "", "$M"),
            ("claude -p --model haiku --input-format stream-json --permission-prompts host < in.jsonl > out.jsonl", "claude", "", "in.jsonl", "", "haiku"),
            ("claude -p <<'EOF'\nwrite the plan\nEOF", "claude", "write the plan", "", "", ""),
            ("cat > run.sh <<'EOF'\nclaude -p \"hi\"\nEOF\nchmod +x run.sh", "", "", "", "", ""),
            ("ps -ax | grep 'claude -p'", "", "", "", "", ""),
            ("claude --version", "", "", "", "", ""),
            (CODEX_CHECK, "", "", "", "", ""),
            ("claude -p --help", "", "", "", "", ""),
            ("claude mcp list", "", "", "", "", ""),
            ("codex login", "", "", "", "", ""),
            (r#"codex "interactive""#, "", "", "", "", ""),
            ("claude", "", "", "", "", ""),
            ("which claude", "", "", "", "", ""),
            ("go test ./...", "", "", "", "", ""),
            ("rush session start --agent codex --name walk --cwd /work --prompt-file .claude/tmp/walk.md --json", "codex", "", ".claude/tmp/walk.md", "/work", ""),
            ("rush session start --agent codex --prompt-file - <<'EOF'\nwalk the site\nEOF", "codex", "walk the site", "", "", ""),
            ("rush session start --session-id x --resume", "", "", "", "", ""),
        ];
        for &(cmd, kind, prompt, from, dir, model) in cases {
            let got = spawn_of(&reg, cmd);
            match (got, kind) {
                (None, "") => {}
                (Some(sp), "") => panic!("{cmd:?}: a spawn {sp:?}, want none"),
                (None, _) => panic!("{cmd:?}: no spawn, want {kind}"),
                (Some(sp), _) => assert!(
                    sp.kind == kind
                        && sp.prompt == prompt
                        && sp.from == from
                        && sp.dir == dir
                        && sp.model == model,
                    "{cmd:?}: got {sp:?}"
                ),
            }
        }
    }
}
