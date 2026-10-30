//! The bundled plugin that refuses an agent's wasteful searches: a
//! recursive grep, a git grep or rg with a giant alternation, a loop of
//! whole-tree git greps. The refusal tells the agent the cheaper command.
//! rush gate hook asks it, before gating.
//! Ported from internal/bundled/searchguard/plugin.go:1-284.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use rush_plugin::{Bundle, Manifest};

use crate::gatecore;

/// The plugin's.
pub const NAME: &str = "search-guard";

/// The search guard's.
pub fn manifest() -> Manifest {
    crate::manifest(
        NAME,
        "",
        "Refuses agents' wasteful searches (grep -r, a git grep or rg with dozens of alternatives, \
         one whole-tree git grep per name in a loop) and tells them the cheaper command.",
        16,
    )
}

pub(crate) fn bundle() -> Bundle {
    crate::bundle_of(manifest(), true, crate::run_switch)
}

/// Whether you have the search guard on. It reads the disk.
pub fn on() -> bool {
    rush_plugin::bundled_on(NAME)
}

/// The most alternatives one git grep or rg may search.
const MAX_ALTERNATIVES: usize = 15;

pub const RECURSIVE_GREP: &str = "search-guard: recursive grep walks everything, nested worktrees and node_modules included. \
     Use `rg -n <pattern> <path>` (respects .gitignore, skips nested worktrees) or `git grep -n <pattern>`.";
pub const BIG_ALTERNATION: &str = "search-guard: a pattern with more than 15 alternatives makes git grep and rg build a huge \
     automaton (git grep has used 10-12 GB). Write the words to a file, one per line, and run one pass: \
     `rg -n -F -w -f words.txt`.";
pub const GREP_PER_NAME: &str = "search-guard: this runs one whole-tree git grep per name. Write the names to a file, one per \
     line, and run one pass: `rg -n -F -w -f words.txt` (add -o to see which name matched).";

fn base(w: &str) -> &str {
    Path::new(w)
        .file_name()
        .and_then(|b| b.to_str())
        .unwrap_or(w)
}

/// Why command is a wasteful search, with the cheaper command to run
/// instead, or "" when it's fine.
pub fn refusal(command: &str) -> &'static str {
    let cmds = gatecore::commands(&command.replace("{}", XARGS_ARG)).unwrap_or_default();
    let mut in_loop = false;
    for words in &cmds {
        let words = skip_lead(words);
        if words.is_empty() {
            continue;
        }
        match base(&words[0]) {
            "for" | "while" | "until" => in_loop = true,
            "done" => in_loop = false,
            "grep" | "egrep" | "fgrep" => {
                if recursive(&words[1..]) {
                    return RECURSIVE_GREP;
                }
            }
            "rg" => {
                if alternatives(&words[1..]) > MAX_ALTERNATIVES {
                    return BIG_ALTERNATION;
                }
            }
            "git" => {
                if let Some(args) = git_grep(&words[1..]) {
                    if alternatives(args) > MAX_ALTERNATIVES {
                        return BIG_ALTERNATION;
                    }
                    if in_loop && !has_pathspec(args) {
                        return GREP_PER_NAME;
                    }
                }
            }
            "xargs" if xargs_search(&words[1..]) => return GREP_PER_NAME,
            _ => {}
        }
    }
    if SCRIPTED_GIT_GREP.is_match(command) && SCRIPT_LOOP.is_match(command) {
        return GREP_PER_NAME;
    }
    ""
}

/// Stands in for xargs's {}, which the gate's command splitter splits at.
const XARGS_ARG: &str = "\x1a";

/// xargs's flags that take the next word.
fn xargs_value_flag(a: &str) -> bool {
    matches!(
        a,
        "-n" | "-L"
            | "-P"
            | "-d"
            | "-E"
            | "-s"
            | "-a"
            | "--max-args"
            | "--max-procs"
            | "--delimiter"
            | "--arg-file"
    )
}

/// Whether xargs's args run a git grep, or an rg searching for each input
/// (-I's replacement in its pattern), itself or by sh -c.
fn xargs_search(args: &[String]) -> bool {
    let mut repl = String::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "-I" && i + 1 < args.len() {
            repl = args[i + 1].clone();
            i += 1;
        } else if a == "-i" || a == "--replace" {
            repl = XARGS_ARG.into();
        } else if a.starts_with("-I") || a.starts_with("-i") {
            repl = a[2..].to_string();
        } else if let Some(r) = a.strip_prefix("--replace=") {
            repl = r.to_string();
        } else if xargs_value_flag(a) {
            i += 1;
        } else if !a.starts_with('-') {
            return per_input(&args[i..], &repl, false);
        }
        i += 1;
    }
    false
}

/// Whether words, as xargs runs them with replacement repl, search once
/// per input. Under sh -c, rg's "$1" is the input too.
fn per_input(words: &[String], repl: &str, script: bool) -> bool {
    match base(&words[0]) {
        "git" => git_grep(&words[1..]).is_some(),
        "rg" => {
            let p = rg_pattern(&words[1..]);
            !repl.is_empty() && p.contains(repl) || script && p.starts_with('$')
        }
        "sh" | "bash" | "zsh" => {
            if let Some(c) = index_of(words, "-c")
                && c > 0
                && c + 1 < words.len()
            {
                for sub in gatecore::commands(&words[c + 1]).unwrap_or_default() {
                    let sub = skip_lead(&sub);
                    if !sub.is_empty() && per_input(sub, repl, true) {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// rg's common flags that take the next word.
fn rg_value_flag(a: &str) -> bool {
    matches!(
        a,
        "-g" | "--glob"
            | "-t"
            | "--type"
            | "-T"
            | "--type-not"
            | "-A"
            | "-B"
            | "-C"
            | "-m"
            | "--max-count"
            | "-M"
            | "-j"
            | "-f"
            | "--file"
    )
}

/// rg's pattern: its first -e's, else its first operand.
fn rg_pattern(args: &[String]) -> String {
    let mut first = String::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if (a == "-e" || a == "--regexp") && i + 1 < args.len() {
            return args[i + 1].clone();
        } else if let Some(p) = a.strip_prefix("--regexp=") {
            return p.to_string();
        } else if a == "--" {
            if first.is_empty() && i + 1 < args.len() {
                first = args[i + 1].clone();
            }
            return first;
        } else if rg_value_flag(a) {
            i += 1;
        } else if !a.starts_with('-') && first.is_empty() {
            first = a.to_string();
        }
        i += 1;
    }
    first
}

/// Come before the program they run.
fn lead(w: &str) -> bool {
    matches!(
        w,
        "do" | "then"
            | "else"
            | "!"
            | "time"
            | "command"
            | "env"
            | "sudo"
            | "nice"
            | "nohup"
            | "exec"
    )
}

fn skip_lead(words: &[String]) -> &[String] {
    let mut w = words;
    while let Some(first) = w.first()
        && (lead(first) || first.contains('=') && !first.starts_with('-'))
    {
        w = &w[1..];
    }
    w
}

/// grep's short flags that take a value: the rest of their bundle, or the
/// next word.
const GREP_ARG_FLAGS: &[u8] = b"efmABCdD";

/// Whether grep's args ask it to walk directories.
fn recursive(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            return false;
        } else if a == "--recursive"
            || a == "--dereference-recursive"
            || a == "--directories=recurse"
        {
            return true;
        } else if (a == "-d" || a == "--directories") && i + 1 < args.len() {
            if args[i + 1] == "recurse" {
                return true;
            }
            i += 1;
        } else if !(a.starts_with("--") || !a.starts_with('-') || a.len() < 2) {
            let b = a.as_bytes();
            for (j, &c) in b.iter().enumerate().skip(1) {
                if c == b'r' || c == b'R' {
                    return true;
                }
                if GREP_ARG_FLAGS.contains(&c) {
                    if j == b.len() - 1 {
                        i += 1;
                    }
                    break;
                }
            }
        }
        i += 1;
    }
    false
}

/// The args of a git grep, past git's own options.
fn git_grep(args: &[String]) -> Option<&[String]> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "grep" {
            return Some(&args[i + 1..]);
        } else if a == "-C" || a == "-c" || a == "--git-dir" || a == "--work-tree" {
            i += 1;
        } else if !a.starts_with('-') {
            return None;
        }
        i += 1;
    }
    None
}

/// Roughly how many patterns args search for: each -e, and each branch of
/// a pattern's | or \|.
fn alternatives(args: &[String]) -> usize {
    let mut n = 0;
    for a in args {
        if a == "-e" || a == "--regexp" {
            n += 1;
        }
        let c = a.matches('|').count();
        if c > 0 {
            n += c + 1;
        }
    }
    n
}

fn has_pathspec(args: &[String]) -> bool {
    index_of(args, "--").is_some()
}

fn index_of(words: &[String], w: &str) -> Option<usize> {
    words.iter().position(|x| x == w)
}

/// A git grep run from a node or python script, and a loop around it.
static SCRIPTED_GIT_GREP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?:exec(?:File)?Sync|spawnSync|execFile|subprocess\.\w+|check_output)\(\s*"#,
        r#"(?:["'`]git["'`]\s*,\s*\[?\s*["'`]grep|["'`]git grep|\[\s*["'`]git["'`]\s*,\s*["'`]grep)"#
    ))
    .expect("a good pattern")
});
static SCRIPT_LOOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(?:map|forEach|flatMap|filter)\(|\bfor\s*\(|\bfor\s+\w+\s+(?:of|in)\b|\bwhile\b")
        .expect("a good pattern")
});

#[cfg(test)]
mod tests {
    use super::*;

    // plugin_test.go TestRefusal
    #[test]
    fn refusals() {
        let alts = |sep: &str, n: usize| {
            (0..n)
                .map(|i| format!("name{i}"))
                .collect::<Vec<_>>()
                .join(sep)
        };
        let cases: Vec<(String, &str)> = vec![
            // Recursive grep.
            ("grep -rn foo .".into(), RECURSIVE_GREP),
            ("grep -r foo src".into(), RECURSIVE_GREP),
            ("grep -R foo".into(), RECURSIVE_GREP),
            ("grep -nR foo .".into(), RECURSIVE_GREP),
            ("grep -i --recursive foo .".into(), RECURSIVE_GREP),
            ("grep -d recurse foo .".into(), RECURSIVE_GREP),
            ("cd app && grep -rln 'x' . | head".into(), RECURSIVE_GREP),
            ("LC_ALL=C grep -rn foo .".into(), RECURSIVE_GREP),
            ("egrep -r 'a|b' .".into(), RECURSIVE_GREP),
            // Giant alternations.
            (
                format!("git grep -n '{}'", alts(r"\|", 16)),
                BIG_ALTERNATION,
            ),
            (format!(r#"rg -n "{}" src"#, alts("|", 20)), BIG_ALTERNATION),
            (
                format!("git -C repo grep -nE '{}'", alts("|", 16)),
                BIG_ALTERNATION,
            ),
            // One whole-tree git grep per name.
            (
                "for n in a b c; do git grep -n \"$n\"; done".into(),
                GREP_PER_NAME,
            ),
            (
                "while read n; do echo $n; git grep -l \"$n\" | wc -l; done < names.txt".into(),
                GREP_PER_NAME,
            ),
            ("cat names | xargs -n1 git grep -n".into(), GREP_PER_NAME),
            (
                "cat names | xargs -I{} git grep -n {}".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs -I {} git grep -l {} | sort -u".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs -0 -P 8 -I% git grep -n %".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs -n 1 git -C repo grep -n".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs -I{} rg -n -w {} src".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs -I{} rg -n -e '\\b{}\\b'".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs --replace=NAME rg -l NAME".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs -I{} sh -c 'git grep -n \"{}\" | wc -l'".into(),
                GREP_PER_NAME,
            ),
            (
                "cat names | xargs -n1 sh -c 'rg -n \"$1\"' _".into(),
                GREP_PER_NAME,
            ),
            (
                r#"node -e 'names.map(n => execFileSync("git", ["grep", "-n", n]))'"#.into(),
                GREP_PER_NAME,
            ),
            (
                "node -e 'for (const n of names) execSync(`git grep -l ${n}`)'".into(),
                GREP_PER_NAME,
            ),
            (
                r#"python3 -c 'for n in names: subprocess.run(["git", "grep", n])'"#.into(),
                GREP_PER_NAME,
            ),
            // Fine.
            ("rg -n foo".into(), ""),
            ("git grep -n foo".into(), ""),
            ("grep foo file.txt".into(), ""),
            ("ps | grep x".into(), ""),
            ("ps aux | grep -i node".into(), ""),
            ("grep -e -r file.txt".into(), ""),
            ("grep -A 3 -B2 foo file.txt".into(), ""),
            ("grep --color=never foo file.txt".into(), ""),
            ("rg -n -F -w -f words.txt".into(), ""),
            ("git grep -nE 'a|b|c|d'".into(), ""),
            (format!("grep -E '{}' file.txt", alts("|", 30)), ""),
            ("for f in a b; do git grep -n foo -- $f; done".into(), ""),
            (
                "for f in *.go; do echo $f; done; git grep -n foo".into(),
                "",
            ),
            ("for f in *.go; do rg -n foo $f; done".into(), ""),
            (
                r#"node -e 'execFileSync("git", ["grep", "-n", "foo"])'"#.into(),
                "",
            ),
            ("echo 'grep -r is slow'".into(), ""),
            ("git log --grep foo".into(), ""),
            ("find . -name '*.go' | xargs rg -n foo".into(), ""),
            ("find . -name '*.go' | xargs -I{} rg -n foo {}".into(), ""),
            ("git ls-files | xargs -I{} rg -n -e foo -- {}".into(), ""),
            ("cat files | xargs -I{} wc -l {}".into(), ""),
            ("cat names | xargs -I{} echo {}".into(), ""),
            ("find . -name x -exec rm {} \\;".into(), ""),
        ];
        for (command, want) in cases {
            assert_eq!(refusal(&command), want, "refusal({command:?})");
        }
    }
}
