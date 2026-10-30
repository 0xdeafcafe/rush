//! The files a shell command plainly writes, for a session's Changes.
//! Port of internal/convo/shellwrites.go (ShellWrite :12, ShellWrites :30,
//! ShellRun :38, resolve :170, scratch :188, inPlace :199, inPlaceFiles
//! :210, looksLikeFile :241, commandLines :252, shellSegs :280).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::render::clean_path as clean;

/// A file a shell command writes, as the command shows it. content is what
/// it writes, when the command holds all of it (a heredoc sent to the
/// file).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellWrite {
    pub path: String,
    pub content: String,
    /// content is all the file holds now.
    pub whole: bool,
}

/// A heredoc's opening: <<EOF, <<-'EOF', << "EOF".
static HEREDOC_OPEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<<-?\s*['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?"#).expect("a valid pattern")
});

/// The files shell command cmd plainly writes, run in cwd: redirects, tee,
/// touch, the target of cp, mv, ln and rsync, files edited in place (sed
/// -i, perl -i, gofmt -w, prettier --write), and the files a script fed on
/// stdin is given (python3 - a.go b.go), which is how agents edit with a
/// script. A cd in it moves where the rest runs.
///
/// shortcut: a word-split read, not a shell parser: quotes with spaces in,
/// variables and globs fool it, and a script's file arguments may only be
/// read. Enough to show most of what a session changes outside its edits.
pub fn shell_writes(cmd: &str, cwd: &str) -> Vec<ShellWrite> {
    shell_run(cmd, cwd).0
}

const NOT_A_PATH: &[char] = &['$', '*', '?', '`', '(', ')', '{', '}', '<', '>', '|'];

/// shell_writes, with the files the command removes (rm) and the folder
/// it ends in: an agent's shell that keeps its folder between commands
/// runs the next there.
pub fn shell_run(cmd: &str, cwd: &str) -> (Vec<ShellWrite>, Vec<String>, String) {
    let mut cwd = cwd.to_string();
    let mut out: Vec<ShellWrite> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let keep = |out: &mut Vec<ShellWrite>,
                seen: &mut HashMap<String, usize>,
                cwd: &str,
                p: &str,
                content: &str,
                whole: bool| {
        if p.is_empty()
            || p == "-"
            || p.contains(NOT_A_PATH)
            || p.starts_with("/dev/")
            || p.starts_with('&')
        {
            return;
        }
        let p = resolve(p, cwd);
        if p.is_empty() {
            return;
        }
        if let Some(&i) = seen.get(&p) {
            if whole {
                out[i].content = content.to_string();
                out[i].whole = true;
            }
            return;
        }
        seen.insert(p.clone(), out.len());
        out.push(ShellWrite {
            path: p,
            content: content.to_string(),
            whole,
        });
    };
    for line in command_lines(cmd) {
        for seg in shell_segs(&line.text) {
            let mut skip = 0;
            while skip < seg.words.len() {
                let w = &seg.words[skip];
                if w.contains('=')
                    || ["sudo", "env", "time", "command", "rtk"].contains(&w.as_str())
                {
                    skip += 1;
                } else {
                    break;
                }
            }
            let f = &seg.words[skip..];
            let quoted = &seg.quoted[skip..];
            if f.is_empty() {
                continue;
            }
            if f[0] == "cd" {
                if f.len() > 1 && !f[1].contains(['$', '`']) {
                    let mut d = f[1].trim_matches(['"', '\'']).to_string();
                    if !d.starts_with('/') && !cwd.is_empty() {
                        d = clean(&format!("{cwd}/{d}"));
                    }
                    if d.starts_with('/') {
                        cwd = clean(&d);
                    }
                }
                continue;
            }
            let mut args: Vec<&str> = Vec::new(); // what isn't a flag or a redirect
            let mut redirected: Vec<&str> = Vec::new();
            let mut i = 0;
            while i < f.len() {
                let w = f[i].trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
                if quoted[i] {
                    if i > 0 {
                        args.push(&f[i]);
                    }
                } else if w == ">" || w == ">>" || w == ">|" {
                    if i + 1 < f.len() {
                        redirected.push(&f[i + 1]);
                        i += 1;
                    }
                } else if w.starts_with('>') && !w.starts_with(">&") {
                    redirected.push(w.trim_start_matches(['>', '|']));
                } else if f[i].starts_with('<') {
                    if ["<", "<<", "<<-", "<<<"].contains(&f[i].as_str()) {
                        i += 1; // its word is read, not written
                    }
                } else if i == 0 || w.starts_with('-') && w != "-" {
                } else {
                    args.push(&f[i]);
                }
                i += 1;
            }
            for r in &redirected {
                // cat > f <<EOF: the heredoc is the whole file.
                let whole = !line.body.is_empty() && f[0] == "cat" && !seg.appends;
                keep(&mut out, &mut seen, &cwd, r, &line.body, whole);
            }
            let prog = f[0].rsplit('/').next().unwrap_or(&f[0]);
            let mut each = |xs: &[&str]| {
                for a in xs {
                    keep(&mut out, &mut seen, &cwd, a, "", false);
                }
            };
            match prog {
                "rm" => {
                    for a in &args {
                        let p = resolve(a, &cwd);
                        if !p.is_empty() && !a.contains(['$', '*', '?', '`']) {
                            removed.push(p);
                        }
                    }
                }
                "touch" | "tee" => each(&args),
                "cp" | "mv" | "ln" | "rsync" | "install" => {
                    if args.len() >= 2 {
                        each(&args[args.len() - 1..]);
                    }
                }
                "sed" | "gsed" | "perl" => {
                    if in_place(f, "-i") || in_place(f, "-pi") {
                        let files = in_place_files(f);
                        each(&files);
                    }
                }
                "gofmt" | "goimports" | "gofumpt" => {
                    if in_place(f, "-w") {
                        each(&args);
                    }
                }
                "prettier" | "black" | "rustfmt" => {
                    if prog != "prettier" || in_place(f, "--write") {
                        each(&args);
                    }
                }
                "python" | "python3" | "node" | "ruby" | "perl5" => {
                    // A script on stdin, edit-in-place style: the files it's given.
                    if let Some(dash) = f.iter().position(|x| x == "-")
                        && dash > 0
                        && !line.body.is_empty()
                    {
                        let files: Vec<&str> = f[dash + 1..]
                            .iter()
                            .map(String::as_str)
                            .filter(|a| {
                                !a.starts_with('-') && !a.starts_with('<') && looks_like_file(a)
                            })
                            .collect();
                        each(&files);
                    }
                }
                _ => {}
            }
        }
    }
    (out, removed, cwd)
}

/// Path p, from cwd, made whole: "" for home (the Left ledger has those),
/// temp folders, or a relative path with nowhere to be from.
fn resolve(p: &str, cwd: &str) -> String {
    if p.starts_with("~/") {
        return String::new();
    }
    let p = if p.starts_with('/') {
        clean(p)
    } else if cwd.is_empty() {
        return String::new();
    } else {
        clean(&format!("{cwd}/{p}"))
    };
    if scratch(&p) { String::new() } else { p }
}

/// Whether p is in a temp folder: thrown away, not changed.
fn scratch(p: &str) -> bool {
    let tmp = format!("{}/", clean(&std::env::temp_dir().to_string_lossy()));
    [
        "/tmp/",
        "/private/tmp/",
        "/var/folders/",
        "/private/var/folders/",
        tmp.as_str(),
    ]
    .iter()
    .any(|d| p.starts_with(d))
}

/// Whether the command's words have flag, alone or with a suffix (sed
/// -i.bak, -i'').
fn in_place(f: &[String], flag: &str) -> bool {
    f[1..]
        .iter()
        .any(|w| w == flag || w.starts_with(flag) && flag.len() == 2)
}

/// The files sed or perl edits in place: its words less flags, the script
/// (-e's word, or else the first) and macOS's empty backup suffix.
fn in_place_files(f: &[String]) -> Vec<&str> {
    let mut files = Vec::new();
    let mut script = false;
    let mut i = 1;
    while i < f.len() {
        let w = f[i].as_str();
        if w.is_empty() {
        } else if w == "-e" || w == "--expression" {
            i += 1;
            script = true;
        } else if w.starts_with('-') {
        } else if !script {
            script = true;
        } else {
            files.push(w);
        }
        i += 1;
    }
    files
}

/// Whether a word names a file: a path, or a name with an extension.
fn looks_like_file(a: &str) -> bool {
    let base = clean(a);
    let base = base.rsplit('/').next().unwrap_or("");
    a.contains('/') || base.contains('.') && !base.starts_with('.')
}

/// One line of a command, and the heredoc that follows it, if it opens
/// one.
struct CmdLine {
    text: String,
    body: String,
}

/// cmd's lines with heredoc bodies taken out: a script's lines aren't
/// commands.
fn command_lines(cmd: &str) -> Vec<CmdLine> {
    let lines: Vec<&str> = cmd.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut l = CmdLine {
            text: lines[i].to_string(),
            body: String::new(),
        };
        if let Some(m) = HEREDOC_OPEN.captures(lines[i]) {
            let end = &m[1];
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != end {
                body.push(lines[i]);
                i += 1;
            }
            l.body = body.join("\n") + "\n";
        }
        out.push(l);
        i += 1;
    }
    out
}

/// One simple command of a line: its words, quotes taken off, and which of
/// them were quoted (a quoted > is a word, not a redirect).
#[derive(Default)]
struct ShellSeg {
    words: Vec<String>,
    quoted: Vec<bool>,
    /// It has a >>.
    appends: bool,
}

/// line split at &&, ||, ; and | outside quotes, and each part into its
/// words.
fn shell_segs(line: &str) -> Vec<ShellSeg> {
    let mut out = Vec::new();
    let mut cur = ShellSeg::default();
    let mut w = String::new();
    let (mut in_word, mut quoted) = (false, false);
    let end = |cur: &mut ShellSeg, w: &mut String, in_word: &mut bool, quoted: &mut bool| {
        if *in_word {
            if w == ">>" && !*quoted {
                cur.appends = true;
            }
            cur.words.push(std::mem::take(w));
            cur.quoted.push(*quoted);
        }
        w.clear();
        *in_word = false;
        *quoted = false;
    };
    let mut q: Option<char> = None;
    let rs: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < rs.len() {
        let r = rs[i];
        if let Some(qc) = q {
            if r == qc {
                q = None;
            } else if r == '\\' && qc == '"' && i + 1 < rs.len() {
                i += 1;
                w.push(rs[i]);
            } else {
                w.push(r);
            }
        } else if r == '\'' || r == '"' {
            q = Some(r);
            in_word = true;
            quoted = true;
        } else if r == '\\' && i + 1 < rs.len() {
            i += 1;
            w.push(rs[i]);
            in_word = true;
        } else if r == ' ' || r == '\t' {
            end(&mut cur, &mut w, &mut in_word, &mut quoted);
        } else if r == ';' || r == '|' || r == '&' && i + 1 < rs.len() && rs[i + 1] == '&' {
            if r == '&' || r == '|' && i + 1 < rs.len() && rs[i + 1] == '|' {
                i += 1;
            }
            end(&mut cur, &mut w, &mut in_word, &mut quoted);
            out.push(std::mem::take(&mut cur));
        } else if r == '#' && !in_word {
            end(&mut cur, &mut w, &mut in_word, &mut quoted);
            out.push(cur);
            return out;
        } else {
            w.push(r);
            in_word = true;
        }
        i += 1;
    }
    end(&mut cur, &mut w, &mut in_word, &mut quoted);
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // shellwrites_test.go: TestShellWrites.
    #[test]
    fn shell_writes_read() {
        let cases: &[(&str, &[&str])] = &[
            (
                "echo hi > out.txt 2>/dev/null; cat a | tee -a log/b.log",
                &["/w/out.txt", "/w/log/b.log"],
            ),
            (
                "cd internal/host && sed -i '' 's/a b/c d/g' host.go inbox.go",
                &["/w/internal/host/host.go", "/w/internal/host/inbox.go"],
            ),
            (
                "python3 -I - internal/ui/panelsubs.go <<'EOF'\nimport sys\nopen(sys.argv[1],'w').write('x > y')\ncat > fake.txt\nEOF\ngo build ./...",
                &["/w/internal/ui/panelsubs.go"],
            ),
            (
                "gofmt -w a.go && go vet ./... && cp a.go /tmp/b.go && cp a.go /opt/b.go",
                &["/w/a.go", "/opt/b.go"],
            ),
            (
                "git status --short; rg -n 'x > y' internal | head; go test ./... 2>&1 | tail -3",
                &[],
            ),
            (
                "perl -pi -e 's/x/y/' internal/a.go; sed -n 1,20p b.go",
                &["/w/internal/a.go"],
            ),
            ("echo $x > \"$f\"; mv a.go b.go # > c.go", &["/w/b.go"]),
        ];
        for (cmd, want) in cases {
            let got: Vec<String> = shell_writes(cmd, "/w")
                .into_iter()
                .map(|w| w.path)
                .collect();
            assert_eq!(got, *want, "{cmd:?}");
        }
        let w = shell_writes("cat > notes/plan.md <<'EOF'\n# Plan\nstep one\nEOF", "/w");
        assert!(
            w.len() == 1 && w[0].whole && w[0].content == "# Plan\nstep one\n",
            "a heredoc into a file: {w:?}"
        );
    }

    #[test]
    fn shell_run_follows_cd_and_rm() {
        let (w, gone, end) = shell_run("cd sub && touch a.txt && rm a.txt b.txt", "/w");
        assert_eq!(w.len(), 1);
        assert_eq!(gone, ["/w/sub/a.txt", "/w/sub/b.txt"]);
        assert_eq!(end, "/w/sub");
    }
}
