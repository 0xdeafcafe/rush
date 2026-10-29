//! Pi's commands (prompt templates and skills), settings files and memory
//! (its AGENTS.md files). Port of internal/adapters/pi/extras.go:1-99, with
//! the reading of a folder of skills or commands from
//! internal/adapters/claude/claude/commands.go:136-219 (readSkills,
//! readCommands, frontmatter), which Pi keeps as Claude Code does.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rush_core::adapter::{Command, Memory, MemoryFile, Profile, SettingsFile};

use crate::discover::mtime;

/// The prompt templates (/name) and skills (/skill:name) a session in cwd
/// has: the project's .pi first, then p's own, then the .agents skills Pi
/// shares with other agents. Packages' and settings' extra folders aren't
/// read.
pub(crate) fn commands(p: &Profile, cwd: &str) -> Vec<Command> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut seen = HashSet::new();
    let mut out: Vec<Command> = Vec::new();
    let mut add = |cs: Vec<Command>, source: &str| {
        for mut c in cs {
            if !c.name.is_empty() && seen.insert(c.name.clone()) {
                c.source = source.into();
                out.push(c);
            }
        }
    };
    for (dir, source) in [
        (Path::new(cwd).join(".pi"), "project"),
        (PathBuf::from(&p.dir), "yours"),
    ] {
        if cwd.is_empty() && source == "project" {
            continue;
        }
        add(read_commands(&dir.join("prompts"), ""), source);
        add(read_skills(&dir.join("skills"), "skill:"), source);
    }
    if !cwd.is_empty() {
        add(
            read_skills(&Path::new(cwd).join(".agents").join("skills"), "skill:"),
            "project",
        );
    }
    if !home.is_empty() {
        add(
            read_skills(&Path::new(&home).join(".agents").join("skills"), "skill:"),
            "yours",
        );
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn sorted_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    v
}

fn file_size(p: &Path) -> i64 {
    std::fs::metadata(p).map_or(0, |m| m.len() as i64)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A folder of <name>/SKILL.md skills.
fn read_skills(dir: &Path, prefix: &str) -> Vec<Command> {
    let mut out = Vec::new();
    for d in sorted_dir(dir) {
        let f = d.join("SKILL.md");
        if !f.exists() {
            continue;
        }
        let fm = frontmatter(&f);
        let get = |k: &str| fm.get(k).cloned().unwrap_or_default();
        let mut name = get("name");
        if name.is_empty() {
            name = d
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        out.push(Command {
            name: format!("{prefix}{name}"),
            description: get("description"),
            argument_hint: get("argument-hint"),
            skill: true,
            path: s(&f),
            size: file_size(&f),
            ..Command::default()
        });
    }
    out
}

/// A folder of markdown commands, where a subfolder becomes part of the
/// name (git/push.md is git:push).
fn read_commands(dir: &Path, prefix: &str) -> Vec<Command> {
    fn walk(root: &Path, d: &Path, prefix: &str, out: &mut Vec<Command>) {
        for p in sorted_dir(d) {
            let Ok(ft) = std::fs::symlink_metadata(&p).map(|m| m.file_type()) else {
                continue;
            };
            if ft.is_dir() {
                walk(root, &p, prefix, out);
                continue;
            }
            let ps = s(&p);
            let Some(stem) = ps.strip_suffix(".md") else {
                continue;
            };
            let rel = Path::new(stem)
                .strip_prefix(root)
                .map(s)
                .unwrap_or_default();
            let fm = frontmatter(&p);
            let get = |k: &str| fm.get(k).cloned().unwrap_or_default();
            let mut description = get("description");
            if description.is_empty() {
                description = get("");
            }
            out.push(Command {
                name: format!("{prefix}{}", rel.replace('/', ":")),
                description,
                argument_hint: get("argument-hint"),
                path: ps.clone(),
                size: file_size(&p),
                ..Command::default()
            });
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, prefix, &mut out);
    out
}

/// The simple key: value lines between --- fences at the top of a
/// markdown file. Without one, "" holds the first line of text.
fn frontmatter(path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(f) = std::fs::File::open(path) else {
        return out;
    };
    let mut inside = false;
    for (n, line) in BufReader::new(f).lines().enumerate() {
        let Ok(line) = line else { break };
        if n >= 60 {
            break;
        }
        if n == 0 && line.trim() == "---" {
            inside = true;
            continue;
        }
        if !inside {
            let t = line.trim_start_matches(['#', ' ']).trim();
            if !t.is_empty() {
                out.insert(String::new(), t.to_string());
                return out;
            }
            continue;
        }
        if line.trim() == "---" {
            return out;
        }
        if let Some((k, v)) = line.split_once(':')
            && !k.starts_with(' ')
        {
            out.insert(
                k.trim().to_string(),
                v.trim().trim_matches(['"', '\'']).to_string(),
            );
        }
    }
    out
}

/// p's settings.json, then the project's .pi one, which Pi lays over it.
pub(crate) fn settings_files(p: &Profile, cwd: &str) -> Vec<SettingsFile> {
    let mut files = vec![SettingsFile {
        label: "yours".into(),
        path: s(&Path::new(&p.dir).join("settings.json")),
    }];
    if !cwd.is_empty() {
        files.push(SettingsFile {
            label: "project".into(),
            path: s(&Path::new(cwd).join(".pi").join("settings.json")),
        });
    }
    files
}

/// Roughly the tokens n bytes of text are (Go's claude.EstTokens).
fn est_tokens(n: i64) -> i64 {
    (n + 3) / 4
}

/// The AGENTS.md files a session in cwd reads: p's own, then, from the
/// root down to cwd, each folder's AGENTS.md, or its CLAUDE.md when it has
/// none. p's, and the project's AGENTS.md when it has neither, are listed
/// even before they exist, so they can be written.
pub(crate) fn memory(p: &Profile, cwd: &str, root: &str) -> Memory {
    let mut files = Vec::new();
    let mut add = |path: PathBuf, name: String, always: bool| -> bool {
        let st = std::fs::metadata(&path);
        let there = st.as_ref().is_ok_and(|m| !m.is_dir());
        if !there && !always {
            return false;
        }
        let mut f = MemoryFile {
            group: "Instructions".into(),
            path: s(&path),
            name,
            missing: st.is_err(),
            ..MemoryFile::default()
        };
        if let Ok(m) = &st {
            (f.size, f.mod_time, f.up) = (m.len() as i64, mtime(m), est_tokens(m.len() as i64));
        }
        files.push(f);
        true
    };
    add(
        Path::new(&p.dir).join("AGENTS.md"),
        "yours · AGENTS.md".into(),
        true,
    );
    let mut dirs = Vec::new();
    let mut d = cwd.to_string();
    while !d.is_empty() && d != "/" && d != "." {
        dirs.insert(0, d.clone());
        d = match Path::new(&d).parent() {
            Some(p) if !p.as_os_str().is_empty() => s(p),
            _ => ".".into(),
        };
    }
    let top = if root.is_empty() { cwd } else { root };
    let depth = |d: &str| Path::new(d).components().count();
    for d in &dirs {
        let up = depth(cwd).saturating_sub(depth(d));
        let rel = |name: &str| {
            if up == 0 {
                name.to_string()
            } else {
                format!("{}{name}", "../".repeat(up))
            }
        };
        let dp = Path::new(d);
        if !add(dp.join("AGENTS.md"), rel("AGENTS.md"), false)
            && !add(dp.join("CLAUDE.md"), rel("CLAUDE.md"), false)
            && d == top
        {
            add(dp.join("AGENTS.md"), rel("AGENTS.md"), true);
        }
    }
    Memory {
        files,
        groups: vec!["Instructions".into()],
        when: [("Instructions".to_string(), "every session".to_string())].into(),
        problems: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pi's prompt templates and skills are its commands, and its AGENTS.md
    // files (a CLAUDE.md where there's none) its memory.
    #[test]
    fn commands_and_memory() {
        let (dir, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let write = |p: PathBuf, body: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        write(
            dir.path().join("prompts/review.md"),
            "---\ndescription: Review it\n---\nGo.",
        );
        write(
            cwd.path().join(".pi/skills/ship/SKILL.md"),
            "---\nname: ship\ndescription: Ship it\n---\n",
        );
        write(cwd.path().join("CLAUDE.md"), "be kind");
        let p = Profile {
            kind: crate::KIND.into(),
            dir: s(dir.path()),
            ..Profile::default()
        };
        let cwd_s = s(cwd.path());
        // HOME's own .agents skills aren't looked at here: read_skills of a
        // missing folder is nothing, so only the two written count.
        let cmds: Vec<Command> = commands(&p, &cwd_s)
            .into_iter()
            .filter(|c| c.path.starts_with(&p.dir) || c.path.starts_with(&cwd_s))
            .collect();
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        assert_eq!(
            (cmds[0].name.as_str(), cmds[0].description.as_str()),
            ("review", "Review it")
        );
        assert_eq!(
            (
                cmds[1].name.as_str(),
                cmds[1].skill,
                cmds[1].source.as_str()
            ),
            ("skill:ship", true, "project")
        );
        let got: Vec<String> = memory(&p, &cwd_s, "")
            .files
            .iter()
            .map(|f| {
                let base = Path::new(&f.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if f.missing { format!("-{base}") } else { base }
            })
            .collect();
        // The project's AGENTS.md shows as missing, to be written; its
        // CLAUDE.md is read, as Pi does.
        assert!(
            got.len() >= 2 && got[0] == "-AGENTS.md" && got.last().unwrap() == "CLAUDE.md",
            "{got:?}"
        );
        let m = memory(&p, &cwd_s, "");
        assert_eq!(m.files.last().map(|f| f.name.as_str()), Some("CLAUDE.md"));
        assert_eq!(m.files[0].name, "yours · AGENTS.md");
    }

    #[test]
    fn frontmatter_or_first_line() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.md");
        std::fs::write(&p, "\n# Deploy the app\nmore").unwrap();
        assert_eq!(
            frontmatter(&p).get("").map(String::as_str),
            Some("Deploy the app")
        );
    }
}
