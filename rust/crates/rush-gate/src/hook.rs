//! Claude Code's Bash calls are gated by a PreToolUse hook that rewrites
//! the command to run under `rush gate run`: shims miss programs npx and
//! pnpm exec find in node_modules/.bin. Ported from
//! internal/gate/hook.go:1-279; the splitting of a command into words is
//! rush_sys::shellwords, ported from the same file.

use rush_sys::shellwords;

use crate::heavy::heavy;
use crate::node::base;
use crate::rules::Rules;

/// Marks a command already rewritten.
const WRAPPED: &str = " gate run --name ";

/// The first program in command the rules gate, as the command word of any
/// simple command in it, or run by npx, bunx, pnpm exec or dlx, or yarn,
/// in a run [`heavy`] says costs; "" when none is, or it's already
/// wrapped.
pub fn gated(command: &str, rules: Option<&Rules>) -> String {
    let Some(rules) = rules.filter(|r| !r.is_empty()) else {
        return String::new();
    };
    if command.contains(WRAPPED) {
        return String::new();
    }
    for words in shellwords::commands(command).unwrap_or_default() {
        for c in programs(&words) {
            if rules.contains_key(&c.name) && heavy(&c.name, &c.args) {
                return c.name;
            }
        }
    }
    String::new()
}

/// A program and the words after it.
#[derive(Debug, Default)]
struct Call {
    name: String,
    args: Vec<String>,
}

/// command run under the gate as name, from dir, by rush at exe.
pub fn wrap(exe: &str, name: &str, dir: &str, command: &str) -> String {
    format!(
        "{}{WRAPPED}{} --dir {} -- {} -c {}",
        quote(exe),
        quote(name),
        quote(dir),
        shell_for(command),
        quote(command)
    )
}

/// The shell the gate runs command in. On macOS, bash 5.1 and later write
/// a here-document of up to a pipe's size into a pipe before anything
/// reads it, and macOS shrinks its pipes to 512 bytes when kernel memory
/// runs short: a longer one then never finishes writing, and the command
/// hangs for good. macOS's own bash, 3.2, writes every one to a file.
fn shell_for(command: &str) -> &'static str {
    if cfg!(target_os = "macos") && here_doc(command) {
        return "/bin/bash";
    }
    "bash"
}

/// Whether command has a here-document.
pub fn here_doc(command: &str) -> bool {
    shellwords::here_doc(command)
}

/// The command [`wrap`] was given, or command as it is.
pub fn unwrap(command: &str) -> String {
    let Some(i) = command.find(WRAPPED) else {
        return command.to_string();
    };
    let mut sep = " -- bash -c ";
    let j = match command[i..].find(&format!("{sep}'")) {
        Some(j) => j,
        None => {
            sep = " -- /bin/bash -c ";
            match command[i..].find(&format!("{sep}'")) {
                Some(j) => j,
                None => return command.to_string(),
            }
        }
    };
    let q = &command[i + j + sep.len()..];
    if q.len() < 2 || !q.ends_with('\'') {
        return command.to_string();
    }
    let s = q[1..q.len() - 1].replace(r"'\''", "'");
    if quote(&s) != q {
        return command.to_string();
    }
    s
}

/// s as one word to sh.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Come before the program they run: shell words, and wrappers whose flags
/// and numbers (nice -n 5, timeout 30) are skipped too.
fn skipped(w: &str) -> bool {
    matches!(
        w,
        "if" | "then"
            | "else"
            | "elif"
            | "do"
            | "while"
            | "until"
            | "!"
            | "time"
            | "nice"
            | "nohup"
            | "command"
            | "builtin"
            | "exec"
            | "env"
            | "stdbuf"
            | "timeout"
            | "sudo"
            | "noglob"
    )
}

/// The program words runs, and the one a runner in it runs.
fn programs(words: &[String]) -> Vec<Call> {
    let mut words = words;
    while let Some(w) = words.first() {
        if skipped(w) || is_assign(w) || is_arg(w) {
            words = &words[1..];
        } else {
            break;
        }
    }
    let Some(first) = words.first() else {
        return Vec::new();
    };
    let p = base(first).to_string();
    let rest = &words[1..];
    let call = |rest: &[String]| Call {
        name: p.clone(),
        args: rest.to_vec(),
    };
    match p.as_str() {
        "pnpm" => {
            if let Some(i) = rest.iter().position(|w| w == "exec" || w == "dlx") {
                return vec![call(rest), first_program(&rest[i + 1..])];
            }
        }
        "yarn" => {
            let rest = match rest.first().map(String::as_str) {
                Some("exec" | "dlx" | "run") => &rest[1..],
                _ => rest,
            };
            return vec![call(rest), first_program(rest)];
        }
        "npx" | "bunx" | "pnpx" => return vec![call(rest), first_program(rest)],
        _ => {}
    }
    vec![call(rest)]
}

fn first_program(words: &[String]) -> Call {
    for (i, w) in words.iter().enumerate() {
        if !w.starts_with('-') {
            return Call {
                name: base(w).to_string(),
                args: words[i + 1..].to_vec(),
            };
        }
    }
    Call::default()
}

fn is_assign(w: &str) -> bool {
    match w.find('=') {
        Some(i) if i > 0 => !w[..i].contains(['-', '/', '.']),
        _ => false,
    }
}

/// A wrapper's flag or number.
fn is_arg(w: &str) -> bool {
    w.starts_with('-') || w.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Settings, parse_rules};

    // gate_test.go TestGated
    #[test]
    fn gated_finds_the_program() {
        let rules = parse_rules(&Settings::from([(
            "names".to_string(),
            "tsc, go, vitest".to_string(),
        )]));
        let wrapped = wrap("/bin/rush", "tsc", "/x", "tsc");
        for (cmd, want) in [
            ("tsc --noEmit", "tsc"),
            ("cd apps/ui && FOO=1 pnpm exec tsc -b", "tsc"),
            ("pnpm --filter ui exec vitest run", "vitest"),
            ("npx -y tsc -p .", "tsc"),
            ("bunx vitest", "vitest"),
            ("git status; time go test ./... | tail", "go"),
            ("(cd x; ./node_modules/.bin/tsc)", "tsc"),
            ("echo $(go env GOPATH)", ""),
            ("echo $(go vet ./...)", "go"),
            ("tsc -w -p .", ""),
            ("pnpm exec vitest --watch", ""),
            ("go run ./cmd/server", ""),
            ("vitest run src/a.test.ts", "vitest"),
            ("nice -n 5 go build", "go"),
            (r#"git commit -m "go faster" && echo tsc"#, ""),
            ("ls -la", ""),
            ("pnpm typecheck", ""),
            ("echo 'unclosed", ""),
            (
                "cat > b.md <<'EOF'\nruns one server (tsc where it can\nEOF\ngh pr create",
                "",
            ),
            ("cat <<-END | wc\n\t(tsc\n\tEND\ngo test ./...", "go"),
            ("cat <<A <<\"B\"\n(tsc\nA\n(vitest\nB\nls", ""),
            ("cat <<< tsc", ""),
            (wrapped.as_str(), ""),
        ] {
            assert_eq!(gated(cmd, Some(&rules)), want, "Gated({cmd:?})");
        }
        assert_eq!(gated("tsc", None), "");
    }

    // gate_test.go TestUnwrap
    #[test]
    fn unwrap_gives_back_the_command() {
        let cmd = r#"cd 'a b' && echo "it's" | tsc"#;
        let w = wrap("/opt/my rush", "tsc", "/x/it's", cmd);
        assert_eq!(unwrap(&w), cmd);
        assert_eq!(unwrap("ls"), "ls");
        let doc = "cat > m.txt <<'EOF'\nfix\nEOF\ngo test ./...";
        assert_eq!(unwrap(&wrap("/r", "go", "/x", doc)), doc);
    }

    // gate_test.go TestHereDoc
    #[test]
    fn here_docs() {
        for (cmd, want) in [
            ("cat <<EOF\nx\nEOF", true),
            ("python3 - <<'EOF'\nx\nEOF", true),
            ("cat <<-\"END\"\n\tx\nEND", true),
            (r#"grep x <<< "$s""#, false),
            ("echo 'a << b'; ls", true),
            ("for i in 1 2; do echo; done", false),
        ] {
            assert_eq!(here_doc(cmd), want, "{cmd:?}");
        }
    }

    #[test]
    fn wrap_as_go_writes_it() {
        let shell = if cfg!(target_os = "macos") {
            "/bin/bash"
        } else {
            "bash"
        };
        assert_eq!(
            wrap("/bin/rush", "go", "/x", "cat <<E\nE\ngo test"),
            format!(
                "'/bin/rush' gate run --name 'go' --dir '/x' -- {shell} -c 'cat <<E\nE\ngo test'"
            )
        );
        assert_eq!(
            wrap("/bin/rush", "go", "/x", "go test"),
            "'/bin/rush' gate run --name 'go' --dir '/x' -- bash -c 'go test'"
        );
    }
}
