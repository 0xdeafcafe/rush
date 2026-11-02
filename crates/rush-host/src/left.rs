//! What an agent wrote outside its project and its scratch folder: what it
//! may have left behind on the disk (Info.left), and the folder a session's
//! edits say it works in.
//!
//! Ported from internal/host/left.go:23-185 and edits.go:1-143 (the
//! ledger's helpers, workedIn, cdTarget, editedIn, nearest), and
//! agent.go:247-259 (followTo).

use std::path::{Path, PathBuf};

use rush_core::event;
use rush_core::tool::{self, Kind};
use rush_state::Time;

use crate::tmp::{clean, within};
use crate::wire::Left;

/// How many places a session's ledger keeps, the latest.
pub const MAX_LEFT: usize = 200;

/// How many edits or shell commands in a row, in one other checkout, move
/// a session there.
pub const EDITS_TO_MOVE: usize = 3;

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The paths m's tool calls write outside the project (proj, found from
/// the call's folder once, by proj_of), scratch (tmp) and the harness's
/// own folder: Go's noteLeft, without the server.
pub fn left_by(
    m: &event::Message,
    cfg_cwd: &str,
    tmp: &str,
    harness: &str,
    mut proj_of: impl FnMut(&str) -> String,
) -> Vec<Left> {
    let mut out = Vec::new();
    let (mut cwd, mut proj) = (String::new(), String::new());
    for p in &m.parts {
        let Some(c) = &p.call else { continue };
        let shell = c.kind == Kind::Shell;
        let paths: Vec<String> = if shell {
            shell_writes(&c.input.command)
        } else if c.kind == Kind::Move {
            vec![c.input.to.clone()]
        } else if c.kind.changes() && c.kind != Kind::Delete {
            let mut v = vec![c.input.path.clone()];
            v.extend(c.input.edits.iter().map(|e| e.path.clone()));
            v
        } else {
            Vec::new()
        };
        for path in paths {
            if path.is_empty() {
                continue;
            }
            if cwd.is_empty() {
                cwd = if c.input.cwd.is_empty() {
                    cfg_cwd.to_string()
                } else {
                    c.input.cwd.clone()
                };
                proj = proj_of(&cwd);
            }
            let path = abs_path(&path, &cwd);
            if !path.is_empty() && outside(&path, &cwd, &proj, tmp, harness) {
                out.push(Left {
                    path,
                    at: Time::now(),
                    shell,
                });
            }
        }
    }
    out
}

/// list with l added, as its latest, once.
pub fn add_left(mut list: Vec<Left>, l: Left) -> Vec<Left> {
    list.retain(|o| o.path != l.path);
    list.push(l);
    let n = list.len();
    if n > MAX_LEFT {
        list.drain(..n - MAX_LEFT);
    }
    list
}

/// p as an absolute path, ~ and $HOME read as home; relative ones from
/// cwd. "" when it can't be told ($VAR, globs).
pub fn abs_path(p: &str, cwd: &str) -> String {
    let home = home();
    let mut p = p.to_string();
    if p == "~" || p.starts_with("~/") {
        p = format!("{home}{}", &p[1..]);
    } else if p.starts_with("$HOME/") || p.starts_with("${HOME}/") {
        let i = p.find('/').unwrap_or(0);
        p = format!("{home}{}", &p[i..]);
    }
    if p.contains(['$', '*', '?', '`']) {
        return String::new();
    }
    let mut pb = PathBuf::from(&p);
    if !pb.is_absolute() {
        if cwd.is_empty() {
            return String::new();
        }
        pb = Path::new(cwd).join(&p);
    }
    clean(&pb).to_string_lossy().into_owned()
}

/// Home's dot folders agents keep their own state in.
const HARNESS_DIRS: &[&str] = &[
    ".claude",
    ".codex",
    ".gemini",
    ".kimi",
    ".pi",
    ".copilot",
    ".cursor",
    ".opencode",
    ".amp",
    ".qwen",
    ".antigravity",
];

/// Whether path is somewhere the ledger keeps: not the project or the
/// folder the agent works in, not its scratch, not an agent's own state
/// (its harness's or rush's), not a shared cache, not a device.
pub fn outside(path: &str, cwd: &str, proj: &str, tmp: &str, harness: &str) -> bool {
    let home = home();
    let hp = Path::new(&home);
    let state = rush_state::dir();
    let ins: [PathBuf; 12] = [
        cwd.into(),
        proj.into(),
        tmp.into(),
        harness.into(),
        state,
        "/dev".into(),
        hp.join(".cache"),
        hp.join("Library").join("Caches"),
        hp.join("go").join("pkg"),
        hp.join(".npm"),
        hp.join(".cargo"),
        hp.join(".rustup"),
    ];
    let p = Path::new(path);
    for i in &ins {
        if !i.as_os_str().is_empty() && within(i, p) {
            return false;
        }
    }
    // Every harness keeps its state in a dot folder of home: ~/.claude…
    if let Ok(rel) = clean(p).strip_prefix(clean(hp)) {
        let top = rel
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        if top.starts_with('.') && HARNESS_DIRS.contains(&top.as_str()) {
            return false;
        }
    }
    path != "/" && path != home
}

/// The paths a shell command plainly writes: redirects, output flags, and
/// what mkdir, touch, tee, cp, mv, ln, rsync and git clone make. Only
/// absolute and home paths; the rest are where it works.
/// shortcut: a word-split read, not a shell parser; quotes with spaces and
/// a cd inside the command fool it.
pub fn shell_writes(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut keep = |p: &str| {
        let p = p.trim_matches(['"', '\'']);
        if p.starts_with('/')
            || p.starts_with('~')
            || p.starts_with("$HOME")
            || p.starts_with("${HOME}")
        {
            out.push(p.to_string());
        }
    };
    let mut s = cmd.to_string();
    for sep in ["||", ";", "|", "\n"] {
        s = s.replace(sep, "&&");
    }
    for seg in s.split("&&") {
        let mut f: Vec<&str> = seg.split_whitespace().collect();
        while !f.is_empty()
            && (f[0].contains('=') || matches!(f[0], "sudo" | "env" | "time" | "command"))
        {
            f.remove(0);
        }
        let mut args: Vec<&str> = Vec::new(); // what isn't a flag or a redirect
        let mut i = 0;
        while i < f.len() {
            let w = f[i].trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
            if matches!(w, ">" | ">>" | "-o" | "--output" | "-O") {
                if i + 1 < f.len() {
                    keep(f[i + 1]);
                    i += 1;
                }
            } else if let Some(r) = w.strip_prefix('>') {
                keep(r.trim_start_matches('>'));
            } else if let Some(r) = w.strip_prefix("--output=") {
                keep(r);
            } else if w.starts_with('-') || i == 0 {
            } else {
                args.push(f[i]);
            }
            i += 1;
        }
        if f.is_empty() || args.is_empty() {
            continue;
        }
        let base = f[0].rsplit('/').next().unwrap_or(f[0]);
        match base {
            "mkdir" | "touch" | "tee" => {
                for a in &args {
                    keep(a);
                }
            }
            "cp" | "mv" | "ln" | "rsync" => keep(args[args.len() - 1]),
            "git" if args[0] == "clone" && args.len() >= 3 => keep(args[args.len() - 1]),
            _ => {}
        }
    }
    out
}

/// The folder call c works in, when it says: an edit's, or a shell
/// command's that cds first or runs elsewhere. "" otherwise: a command
/// that doesn't runs where the shell was put back. cwd is the session's,
/// start_cwd where its agent started.
pub fn worked_in(c: Option<&tool::Call>, cwd: &str, start_cwd: &str) -> String {
    let Some(c) = c else {
        return String::new();
    };
    if c.kind == Kind::Shell {
        let shell = [c.input.cwd.as_str(), start_cwd, cwd]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let d = cd_target(&c.input.command);
        if !d.is_empty() {
            return abs_path(&d, shell);
        }
        return c.input.cwd.clone();
    }
    if !c.kind.changes() || c.kind == Kind::Delete {
        return String::new();
    }
    let path = if c.kind == Kind::Move {
        &c.input.to
    } else {
        &c.input.path
    };
    let base = if c.input.cwd.is_empty() {
        cwd
    } else {
        &c.input.cwd
    };
    let path = abs_path(path, base);
    if path.is_empty() {
        return String::new();
    }
    Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into())
}

/// The folder a shell command cds (or pushds) into before anything else,
/// quotes taken off; "" when it doesn't, or cds home or back.
pub fn cd_target(cmd: &str) -> String {
    let cmd = cmd.trim();
    let Some(rest) = cmd
        .strip_prefix("cd ")
        .or_else(|| cmd.strip_prefix("pushd "))
    else {
        return String::new();
    };
    let rest = rest.trim();
    let d = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => match rest[1..].find(q) {
            Some(end) => &rest[1..end + 1],
            None => return String::new(),
        },
        _ => match rest.find([' ', '\t', '\n', ';', '&', '|', ')']) {
            Some(i) => &rest[..i],
            None => rest,
        },
    };
    if matches!(d, "-" | "~" | "") {
        return String::new();
    }
    d.to_string()
}

/// The top of the one checkout, not was's, every one of dirs is in.
pub fn edited_in(was: &str, dirs: &[String], root: impl Fn(&str) -> String) -> Option<String> {
    let mut top = String::new();
    for d in dirs {
        let t = root(d);
        if t.is_empty() || (!top.is_empty() && t != top) {
            return None;
        }
        top = t;
    }
    if top.is_empty() || top == root(was) {
        return None;
    }
    Some(top)
}

/// d, or the closest folder above it that's there: a file written into a
/// folder it made has git ask where it is.
pub fn nearest(d: &str) -> String {
    let mut d = PathBuf::from(d);
    loop {
        if d.is_dir() {
            return d.to_string_lossy().into_owned();
        }
        match d.parent() {
            Some(up) if up != d => d = up.to_path_buf(),
            _ => return d.to_string_lossy().into_owned(),
        }
    }
}

/// Where a session in was moves when its agent works in cwd: another
/// checkout's top, or back up to a folder above it in its own. A cd deeper
/// into the same checkout doesn't move it.
pub fn follow_to(was: &str, cwd: &str, root: impl Fn(&str) -> String) -> Option<String> {
    let top = root(cwd);
    if cwd == was {
        None
    } else if !top.is_empty() && top != root(was) {
        Some(top)
    } else if top.is_empty() || was.starts_with(&format!("{cwd}/")) {
        Some(cwd.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    // left_test.go:13
    #[test]
    fn shell_writes_reads_redirects_and_makers() {
        let got = shell_writes(
            r#"cd x && git clone https://g/r /tmp/r 2>/dev/null; curl -o ~/dl.zip u | tee "/var/log/a" > out.txt; cp a b /opt/c && mkdir -p $HOME/w rel"#,
        );
        assert_eq!(
            got,
            [
                "/dev/null",
                "/tmp/r",
                "~/dl.zip",
                "/var/log/a",
                "/opt/c",
                "$HOME/w"
            ]
        );
    }

    fn call(k: Kind, f: impl FnOnce(&mut tool::Input)) -> event::Part {
        let mut c = tool::Call {
            kind: k,
            ..Default::default()
        };
        f(&mut c.input);
        event::Part {
            call: Some(Box::new(c)),
            ..Default::default()
        }
    }

    // left_test.go:23: the ledger keeps writes outside the project, its
    // scratch, harness state and caches, from file tools and shell alike.
    #[test]
    fn note_left() {
        let _home = Home::new();
        let home = std::env::var("HOME").unwrap();
        let proj = tempfile::tempdir().unwrap();
        let proj = proj.path().to_string_lossy().into_owned();
        let tmp = crate::tmp::temp_dir("s1").to_string_lossy().into_owned();
        let m = event::Message {
            role: "assistant".into(),
            parts: vec![
                call(Kind::Write, |i| i.path = format!("{proj}/in.go")),
                call(Kind::Write, |i| i.path = "/opt/elsewhere.txt".into()),
                call(Kind::Edit, |i| {
                    i.path = format!("{home}/.claude/settings.json")
                }),
                call(Kind::Shell, |i| {
                    i.command =
                        format!("echo hi > /tmp/log.txt 2>/dev/null; mkdir ~/.cache/x {tmp}/y")
                }),
                call(Kind::Read, |i| i.path = "/etc/hosts".into()),
            ],
            ..Default::default()
        };
        let mut list = Vec::new();
        for l in left_by(&m, &proj, &tmp, "", |c| c.to_string()) {
            list = add_left(list, l);
        }
        let got: Vec<&str> = list.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(got, ["/opt/elsewhere.txt", "/tmp/log.txt"]);
        assert!(list[1].shell && !list[0].shell);
    }

    #[test]
    fn ledger_keeps_the_latest() {
        let mut list = Vec::new();
        for i in 0..MAX_LEFT + 5 {
            list = add_left(
                list,
                Left {
                    path: format!("/p{i}"),
                    ..Default::default()
                },
            );
        }
        list = add_left(
            list,
            Left {
                path: "/p10".into(),
                ..Default::default()
            },
        );
        assert_eq!(list.len(), MAX_LEFT);
        assert_eq!(list.last().unwrap().path, "/p10");
        assert_eq!(list[0].path, "/p5");
    }

    fn root(d: &str) -> String {
        for r in ["/lw/.worktrees/ds", "/lw", "/rush"] {
            if d == r || d.starts_with(&format!("{r}/")) {
                return r.into();
            }
        }
        String::new()
    }

    // edits_test.go:12
    #[test]
    fn edited_in_one_other_checkout() {
        for (dirs, want) in [
            (vec!["/lw/a", "/lw/b", "/lw"], Some("/lw")), // another checkout
            (
                vec![
                    "/lw/.worktrees/ds/a",
                    "/lw/.worktrees/ds/b/c",
                    "/lw/.worktrees/ds",
                ],
                Some("/lw/.worktrees/ds"),
            ), // its worktree
            (vec!["/lw/a", "/rush/b", "/lw/a"], None),    // back home between
            (vec!["/lw/a", "/lw/.worktrees/ds/a", "/lw/a"], None), // two checkouts
            (vec!["/rush/a", "/rush/b", "/rush"], None),  // its own
            (vec!["/home/.claude/memory", "/lw/a", "/lw/b"], None), // no checkout
        ] {
            let dirs: Vec<String> = dirs.into_iter().map(String::from).collect();
            assert_eq!(
                edited_in("/rush/sub", &dirs, root).as_deref(),
                want,
                "{dirs:?}"
            );
        }
    }

    // edits_test.go:41
    #[test]
    fn worked_in_reads_cds_and_edits() {
        let home = std::env::var("HOME").unwrap();
        let shell = |cmd: &str, cwd: &str| tool::Call {
            kind: Kind::Shell,
            input: tool::Input {
                command: cmd.into(),
                cwd: cwd.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let file = |k: Kind, p: &str| tool::Call {
            kind: k,
            input: tool::Input {
                path: p.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        for (c, want) in [
            (
                Some(shell("cd /lw/.worktrees/ds && pnpm test", "")),
                "/lw/.worktrees/ds".to_string(),
            ),
            (Some(shell(r#"cd "/lw/a b"; ls"#, "")), "/lw/a b".into()),
            (Some(shell("cd ../lw&&ls", "")), "/lw".into()), // from where the shell was put back
            (Some(shell("pushd ~/x >/dev/null", "")), format!("{home}/x")),
            (Some(shell("git status", "")), "".into()),
            (Some(shell("ls", "/lw/m")), "/lw/m".into()),
            (Some(shell("cd - && ls", "")), "".into()),
            (Some(shell("echo cd /x", "")), "".into()),
            (Some(file(Kind::Edit, "/lw/src/a.ts")), "/lw/src".into()),
            (Some(file(Kind::Read, "/lw/src/a.ts")), "".into()),
            (None, "".into()),
        ] {
            assert_eq!(worked_in(c.as_ref(), "/lw", "/rush"), want, "{c:?}");
        }
    }

    // followto_test.go
    #[test]
    fn follow_to_other_checkouts_and_up() {
        let root = |d: &str| -> String {
            for r in ["/repo/.worktrees/b", "/repo"] {
                if d == r || d.starts_with(&format!("{r}/")) {
                    return r.into();
                }
            }
            String::new()
        };
        for (was, cwd, want) in [
            ("/repo", "/repo/modules/auth/process", None), // a cd inside it
            ("/repo", "/repo/.worktrees/b/x", Some("/repo/.worktrees/b")), // into a worktree
            ("/repo/modules/auth/process", "/repo", Some("/repo")), // back up
            ("/repo", "/tmp/x", Some("/tmp/x")),           // out of git
            ("/repo", "/repo", None),
        ] {
            assert_eq!(follow_to(was, cwd, root).as_deref(), want, "{was} {cwd}");
        }
    }
}
