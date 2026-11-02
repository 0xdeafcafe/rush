//! `rush memory`: the memory notes agents keep for this project, listed,
//! reviewed, tidied, and added to from a shell or an agent. Port of Go's
//! cmd/rush/memory.go (memoryCmd :31, isTerminal :167, first :177,
//! memoryWhere :200); the usage, addFlags and readsText are rush-memory's.

use std::io::{Read, Write};
use std::path::Path;

use rush_core::adapter::{LEGACY_KIND, MemoryNote, Profile, Registry};

/// Where `rush memory` works, as Go's memoryWhere reads it from the
/// process: the folder it's run in, CLAUDE_CONFIG_DIR, RUSH_SESSION.
pub struct Where {
    pub cwd: String,
    pub config_dir: String,
    pub session: String,
}

impl Where {
    pub fn from_process() -> Where {
        Where {
            cwd: std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            config_dir: std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_default(),
            session: std::env::var("RUSH_SESSION").unwrap_or_default(),
        }
    }
}

/// What a note is drafted with: (system, prompt) to the cheap model's
/// answer (memory.Asker).
pub type Ask<'a> = &'a dyn Fn(&str, &str) -> Result<String, String>;

/// The cheap model Go's memory.Asker asks: Haiku when there is one.
pub fn cheap_ask(reg: &Registry) -> impl Fn(&str, &str) -> Result<String, String> + '_ {
    move |system, prompt| {
        rush_core::adapter::ask_cheap(reg, system, prompt, "haiku")
            .map(|(_, out)| out)
            .map_err(|e| e.to_string())
    }
}

/// Runs rush memory args; its exit status. stdin is read for add's text
/// when there's none and it isn't a terminal.
#[allow(clippy::too_many_arguments)]
pub fn memory_cmd(
    reg: &Registry,
    at: &Where,
    args: &[String],
    stdin: &mut dyn Read,
    stdin_terminal: bool,
    ask: Ask,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut fail = |e: &str| {
        let _ = writeln!(stderr, "rush memory: {e}");
        1
    };
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        let _ = writeln!(stdout, "{}", rush_memory::USAGE);
        return 0;
    }
    let (p, cwd) = match memory_where(reg, at) {
        Ok(w) => w,
        Err(e) => return fail(&e),
    };
    let ad = reg.get(p.kind.as_str());
    let (Some(mr), Some(mk)) = (
        ad.as_ref().and_then(|a| a.memory_reader()),
        ad.as_ref().and_then(|a| a.memory_keeper()),
    ) else {
        return fail(&format!("{} keeps no memory rush can read", p.kind));
    };
    let (cmd, args) = match args.split_first() {
        Some((c, rest)) => (c.as_str(), rest),
        None => ("list", args),
    };
    match cmd {
        "list" | "ls" => {
            let mut mem = mr.memory(&p, &cwd, "");
            mk.count_opens(&p, &mut mem);
            let mut n = 0;
            for f in &mem.files {
                if f.group != "Auto memory" || f.name == "MEMORY.md" || f.missing {
                    continue;
                }
                n += 1;
                let o = rush_memory::Opens {
                    reads: f.reads as i32,
                    last_read: f.last_read,
                    counted: f.counted,
                };
                let _ = writeln!(
                    stdout,
                    "{:<10} {:<30} {} · {}",
                    f.kind,
                    f.name,
                    f.about,
                    rush_memory::reads_text(&o)
                );
            }
            if n == 0 {
                let _ = writeln!(stdout, "no memory notes for {cwd}");
            }
            if !mem.dir.is_empty() {
                let _ = writeln!(stdout, "\nin {}", mem.dir);
            }
            0
        }
        "review" => {
            let mem = mr.memory(&p, &cwd, "");
            let now = rush_core::time::now();
            let mut n = 0;
            for f in &mem.files {
                let why = rush_memory::why(f, now);
                if !why.is_empty() {
                    n += 1;
                    let _ = writeln!(stdout, "{:<30} {why}", f.name);
                }
            }
            if n == 0 {
                let _ = writeln!(stdout, "nothing to review");
            }
            0
        }
        "tidy" => {
            let mem = mr.memory(&p, &cwd, "");
            if mem.dir.is_empty() {
                return fail(&format!("no memory for {cwd}"));
            }
            match mk.tidy_memory(&mem.dir) {
                Err(e) => fail(&e.to_string()),
                Ok(did) => {
                    let did = if did.is_empty() {
                        vec!["nothing to tidy".to_string()]
                    } else {
                        did
                    };
                    let _ = writeln!(stdout, "{}", did.join("\n"));
                    0
                }
            }
        }
        "add" => {
            let (n, words) = match rush_memory::add_flags(args) {
                Ok(x) => x,
                Err(e) => return fail(&e),
            };
            let mut text = words.join(" ");
            if text == "-" || text.is_empty() && !stdin_terminal {
                let mut b = String::new();
                let _ = stdin.read_to_string(&mut b);
                text = b.trim().to_string();
            }
            if text.is_empty() {
                return fail("nothing to remember: rush memory add <text>");
            }
            let mut n = MemoryNote {
                name: n.name,
                title: n.title,
                description: n.description,
                kind: n.kind,
                body: n.body,
            };
            if n.name.is_empty() || n.description.is_empty() || n.kind.is_empty() {
                let d = match rush_memory::draft(ask, &text, "") {
                    Ok(d) => d,
                    Err(e) => return fail(&e),
                };
                n.name = first(&n.name, &d.name);
                n.description = first(&n.description, &d.description);
                n.kind = first(&n.kind, &d.kind);
                (n.title, n.body) = (d.title, d.body);
            } else {
                n.body = text;
            }
            let path = match mk.remember(&p, &cwd, &n) {
                Ok(p) => p,
                Err(e) => return fail(&e.to_string()),
            };
            if let Ok(w) = mk.read_note(&path) {
                n = w;
            }
            let _ = writeln!(stdout, "remembered ({}): {}\n{path}", n.kind, n.description);
            0
        }
        _ => fail(&format!("no command {cmd:?}: rush memory help")),
    }
}

fn first(a: &str, b: &str) -> String {
    if a.is_empty() { b } else { a }.to_string()
}

/// The profile and folder rush memory works on: the CLAUDE_CONFIG_DIR set
/// (a Claude Code session's own), else the rush session's account when its
/// agent keeps memory rush can write, else Claude Code's first.
fn memory_where(reg: &Registry, at: &Where) -> Result<(Profile, String), String> {
    let Some(a) = reg.get(LEGACY_KIND) else {
        return Err("no Claude Code".into());
    };
    if !at.config_dir.is_empty() {
        let name = Path::new(&at.config_dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let p = Profile {
            kind: LEGACY_KIND.into(),
            name,
            dir: at.config_dir.clone(),
        };
        return Ok((p, at.cwd.clone()));
    }
    if !at.session.is_empty()
        && let Ok(cfg) = rush_host::client::read_config(&at.session)
        && !cfg.account.dir.is_empty()
    {
        let mut p = cfg.account;
        if p.kind.is_empty() {
            p.kind = LEGACY_KIND.into();
        }
        if reg
            .get(p.kind.as_str())
            .is_some_and(|a| a.memory_keeper().is_some())
        {
            return Ok((p, at.cwd.clone()));
        }
    }
    match a.profiles().into_iter().next() {
        Some(p) => Ok((p, at.cwd.clone())),
        None => Err("no Claude Code account".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(reg: &Registry, at: &Where, args: &[&str], ask: Ask) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = memory_cmd(
            reg,
            at,
            &args,
            &mut std::io::empty(),
            true,
            ask,
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn no_model(_: &str, _: &str) -> Result<String, String> {
        Err("no model in tests".into())
    }

    // add as given, list, review and tidy, in a temp Claude Code config.
    #[test]
    fn adds_lists_and_tidies() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg");
        let cwd = tmp.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let reg = Registry::new();
        reg.register(rush_claude::Claude::at(rush_claude::Account::at(
            "test", &cfg,
        )));
        let at = Where {
            cwd: cwd.to_string_lossy().into_owned(),
            config_dir: cfg.to_string_lossy().into_owned(),
            session: String::new(),
        };
        let (code, out, err) = run(
            &reg,
            &at,
            &[
                "add",
                "--type=feedback",
                "--name",
                "tests-first",
                "--description",
                "write the test before the fix",
                "Write",
                "the",
                "failing",
                "test",
                "first.",
            ],
            &no_model,
        );
        assert_eq!(code, 0, "{err}");
        assert!(
            out.starts_with("remembered (feedback): write the test before the fix\n"),
            "{out}"
        );
        assert!(out.trim_end().ends_with("/memory/tests-first.md"), "{out}");
        let (code, out, _) = run(&reg, &at, &[], &no_model);
        assert_eq!(code, 0);
        assert!(
            out.contains("feedback   tests-first ") && out.contains("only its line in MEMORY.md"),
            "{out}"
        );
        let (code, out, _) = run(&reg, &at, &["tidy"], &no_model);
        assert_eq!((code, out.as_str()), (0, "nothing to tidy\n"));
        // Without --name and --description, the model is asked.
        let (code, _, err) = run(&reg, &at, &["add", "likes", "tabs"], &no_model);
        assert_eq!(
            (code, err.as_str()),
            (1, "rush memory: no model in tests\n")
        );
        let drafted = |_: &str, _: &str| -> Result<String, String> {
            Ok(r#"{"name":"likes-tabs","title":"Likes tabs","description":"indents with tabs","type":"user","body":"Indent with tabs."}"#.into())
        };
        let (code, out, err) = run(&reg, &at, &["add", "likes", "tabs"], &drafted);
        assert_eq!(code, 0, "{err}");
        assert!(
            out.starts_with("remembered (user): indents with tabs\n"),
            "{out}"
        );
        let (code, _, err) = run(&reg, &at, &["add"], &no_model);
        assert_eq!(
            (code, err.as_str()),
            (
                1,
                "rush memory: nothing to remember: rush memory add <text>\n"
            )
        );
        let (code, _, err) = run(&reg, &at, &["frob"], &no_model);
        assert_eq!(
            (code, err.as_str()),
            (1, "rush memory: no command \"frob\": rush memory help\n")
        );
        let (code, out, _) = run(&reg, &at, &["help"], &no_model);
        assert_eq!(code, 0);
        assert!(out.starts_with("rush memory — the memory notes"));
    }

    #[test]
    fn no_claude_code_says_so() {
        let at = Where {
            cwd: "/x".into(),
            config_dir: String::new(),
            session: String::new(),
        };
        let (code, _, err) = run(&Registry::new(), &at, &[], &no_model);
        assert_eq!((code, err.as_str()), (1, "rush memory: no Claude Code\n"));
    }
}
