//! The custom commands and skills a session can use, and where other
//! agents find Claude Code's skills. Port of
//! internal/adapters/claude/claude/commands.go.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use rush_core::adapter::Command;
use serde::Deserialize;

use crate::json::nd;

/// How long a reading of a folder's commands is kept.
const KEEP: Duration = Duration::from_secs(30);

static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Vec<Command>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The custom commands and skills a session in cwd can use: the
/// account's own, the project's (cwd up to the filesystem root), and those
/// of installed and synced plugins, named plugin:name. It is read at most
/// every 30 seconds.
pub fn commands(config_dir: &str, cwd: &str) -> Vec<Command> {
    let key = format!("{config_dir}\x00{cwd}");
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, cmds)) = cache.get(&key)
        && at.elapsed() < KEEP
    {
        return cmds.clone();
    }
    let mut seen = HashSet::new();
    let mut out: Vec<Command> = Vec::new();
    let mut add = |c: Command| {
        if !c.name.is_empty() && seen.insert(c.name.clone()) {
            out.push(c);
        }
    };
    let scan = |root: &Path, prefix: &str, source: &str, add: &mut dyn FnMut(Command)| {
        for mut c in read_skills(&root.join("skills"), prefix) {
            c.source = source.to_string();
            add(c);
        }
        for mut c in read_commands(&root.join("commands"), prefix) {
            c.source = source.to_string();
            add(c);
        }
    };
    let mut d = Path::new(cwd);
    while !cwd.is_empty() && d != Path::new("/") && d != Path::new(".") && !d.as_os_str().is_empty() {
        scan(&d.join(".claude"), "", "project", &mut add);
        match d.parent() {
            Some(up) => d = up,
            None => break,
        }
    }
    let cfg = Path::new(config_dir);
    scan(cfg, "", "yours", &mut add);
    for d in glob_dirs(&cfg.join("skills").join("synced")) {
        for mut c in read_skills(&d, "") {
            c.source = "claude.ai".into();
            add(c);
        }
    }
    for root in plugin_roots(config_dir, cwd) {
        let name = plugin_name(&root);
        scan(&root, &format!("{name}:"), &name, &mut add);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    cache.insert(key, (Instant::now(), out.clone()));
    out
}

/// Everything in dir, by name, as filepath.Glob(dir/*).
fn glob_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    out.sort();
    out
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Installed {
    #[serde(deserialize_with = "nd")]
    plugins: BTreeMap<String, Vec<InstalledPlugin>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct InstalledPlugin {
    #[serde(deserialize_with = "nd")]
    pub scope: String,
    #[serde(rename = "projectPath", deserialize_with = "nd")]
    pub project_path: String,
    #[serde(rename = "installPath", deserialize_with = "nd")]
    pub install_path: String,
    #[serde(rename = "installedAt", deserialize_with = "nd")]
    pub installed_at: String,
}

/// The account's plugins/installed_plugins.json, by plugin id.
pub(crate) fn installed_plugins(config_dir: &Path) -> BTreeMap<String, Vec<InstalledPlugin>> {
    fs::read(config_dir.join("plugins").join("installed_plugins.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Installed>(&b).ok())
        .map(|i| i.plugins)
        .unwrap_or_default()
}

/// The folders of plugins installed for this account (user scope, or
/// project scope for a project containing cwd) and synced ones.
fn plugin_roots(config_dir: &str, cwd: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for list in installed_plugins(Path::new(config_dir)).into_values() {
        for p in list {
            if p.scope == "project"
                && !p.project_path.is_empty()
                && !format!("{cwd}/").starts_with(&format!("{}/", p.project_path))
            {
                continue;
            }
            roots.push(PathBuf::from(p.install_path));
        }
    }
    let synced = Path::new(config_dir).join("plugins").join("synced");
    for d in glob_dirs(&synced) {
        for p in glob_dirs(&d) {
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    roots
}

/// The folders of <name>/SKILL.md skills Claude Code at config_dir reads
/// in cwd, less the project's own: the user's, synced ones and each
/// plugin's. Another agent reads them for the same skills.
pub fn skill_roots(config_dir: &str, cwd: &str) -> Vec<String> {
    let cfg = Path::new(config_dir);
    let mut roots = vec![cfg.join("skills")];
    roots.extend(glob_dirs(&cfg.join("skills").join("synced")));
    for r in plugin_roots(config_dir, cwd) {
        roots.push(r.join("skills"));
    }
    roots.sort();
    roots.dedup();
    roots
        .into_iter()
        .filter(|d| d.is_dir())
        .map(|d| d.to_string_lossy().into_owned())
        .collect()
}

fn plugin_name(root: &Path) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        name: String,
    }
    let m: M = fs::read(root.join(".claude-plugin").join("plugin.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if !m.name.is_empty() {
        return m.name;
    }
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A folder of <name>/SKILL.md skills, for agents that keep theirs as
/// Claude Code does.
pub fn read_skills(dir: &Path, prefix: &str) -> Vec<Command> {
    let mut out = Vec::new();
    for d in glob_dirs(dir) {
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
            path: f.to_string_lossy().into_owned(),
            size: file_size(&f),
            ..Command::default()
        });
    }
    out
}

/// commands/*.md, where a subfolder becomes part of the name
/// (commands/git/push.md is git:push), for agents that keep theirs as
/// Claude Code does.
pub fn read_commands(dir: &Path, prefix: &str) -> Vec<Command> {
    let mut out = Vec::new();
    let _ = crate::files::walk_files(dir, &mut |p| {
        if p.extension().is_none_or(|x| x != "md") {
            return Ok(());
        }
        let rel = p.strip_prefix(dir).unwrap_or(p).with_extension("");
        let rel = rel.to_string_lossy().replace('/', ":");
        let fm = frontmatter(p);
        let get = |k: &str| fm.get(k).cloned().unwrap_or_default();
        let mut description = get("description");
        if description.is_empty() {
            description = get("");
        }
        out.push(Command {
            name: format!("{prefix}{rel}"),
            description,
            argument_hint: get("argument-hint"),
            path: p.to_string_lossy().into_owned(),
            size: file_size(p),
            ..Command::default()
        });
        Ok(())
    });
    out
}

/// The simple key: value lines between --- fences at the top of a
/// markdown file. Without one, "" holds the first line of text.
fn frontmatter(path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(f) = fs::File::open(path) else {
        return out;
    };
    let mut inside = false;
    for (n, line) in BufReader::new(f).lines().take(60).enumerate() {
        let Ok(line) = line else { break };
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

fn file_size(p: &Path) -> i64 {
    fs::metadata(p).map_or(0, |m| m.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    // commands_test.go:10 (TestCommands)
    #[test]
    fn commands_of_every_kind() {
        let (cfg, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (cfg, proj) = (cfg.path(), proj.path());
        write(&cfg.join("skills/pdf/SKILL.md"), "---\nname: pdf\ndescription: \"read pdfs\"\n---\nbody");
        write(&proj.join(".claude/commands/git/push.md"), "# Push the branch\n");
        let plug = cfg.join("plugins/synced/x/design");
        write(
            &plug.join("skills/critique/SKILL.md"),
            "---\nname: design-critique\ndescription: feedback\nargument-hint: \"<url>\"\n---\n",
        );
        let got: HashMap<String, Command> = commands(&cfg.to_string_lossy(), &proj.to_string_lossy())
            .into_iter()
            .map(|c| (c.name.clone(), c))
            .collect();
        let pdf = &got["pdf"];
        assert!(pdf.skill && pdf.description == "read pdfs", "{pdf:?}");
        assert_eq!(pdf.source, "yours");
        assert_eq!(got["git:push"].description, "Push the branch");
        assert_eq!(got["git:push"].source, "project");
        let c = &got["design:design-critique"];
        assert_eq!(c.argument_hint, "<url>", "{got:?}");
        assert_eq!(c.source, "design");

        let roots = skill_roots(&cfg.to_string_lossy(), &proj.to_string_lossy());
        assert_eq!(
            roots,
            vec![
                cfg.join("plugins/synced/x/design/skills").to_string_lossy().into_owned(),
                cfg.join("skills").to_string_lossy().into_owned(),
            ]
        );
    }
}
