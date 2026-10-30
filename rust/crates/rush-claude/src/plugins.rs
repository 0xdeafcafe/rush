//! Claude Code's plugins and marketplaces, through `claude plugin`, so a
//! change is exactly what Claude Code's own screen would do. Port of
//! internal/adapters/claude/claude/plugins.go and adapters/claude/plugins.go.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::bytes::Regex;
use rush_core::adapter::{Marketplace, Plugin, PluginChange, PluginOp, PluginParts};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::account::Account;

/// Claude Code's program, run by name from PATH.
pub const PROGRAM: &str = "claude";

/// Runs `claude plugin …` for the account in dir.
pub fn plugin_cli(a: &Account, dir: &str, args: &[&str]) -> io::Result<Vec<u8>> {
    plugin_cli_with(PROGRAM, a, dir, args)
}

/// plugin_cli with the program named: tests give a fake.
pub fn plugin_cli_with(
    program: &str,
    a: &Account,
    dir: &str,
    args: &[&str],
) -> io::Result<Vec<u8>> {
    let mut c = Command::new(program);
    c.arg("plugin").args(args).env_clear();
    for e in a.env() {
        if let Some((k, v)) = e.split_once('=') {
            c.env(k, v);
        }
    }
    if !dir.is_empty() {
        c.current_dir(dir);
    }
    let out = c.stdin(Stdio::null()).output();
    let what = args.first().copied().unwrap_or_default();
    match out {
        Ok(o) if o.status.success() => Ok(o.stdout),
        Ok(o) => {
            let mut msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if msg.is_empty() {
                msg = String::from_utf8_lossy(&o.stdout).trim().to_string();
            }
            if msg.is_empty() {
                msg = o.status.to_string();
            }
            Err(io::Error::other(format!(
                "claude plugin {what}: {}",
                last_line(&msg)
            )))
        }
        Err(e) => Err(io::Error::other(format!("claude plugin {what}: {e}"))),
    }
}

fn last_line(s: &str) -> &str {
    s.trim().rsplit('\n').next().unwrap_or("").trim()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct List {
    installed: Option<Vec<Plugin>>,
    available: Option<Vec<Available>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Available {
    #[serde(flatten)]
    plugin: Plugin,
    #[serde(rename = "pluginId")]
    plugin_id: String,
}

/// The installed plugins (with what each brings) and those the account's
/// marketplaces offer, most installed first.
pub fn plugins(a: &Account, dir: &str) -> io::Result<(Vec<Plugin>, Vec<Plugin>)> {
    parse_plugins(&plugin_cli(a, dir, &["list", "--available", "--json"])?)
}

/// Reads `claude plugin list --available --json`.
pub fn parse_plugins(out: &[u8]) -> io::Result<(Vec<Plugin>, Vec<Plugin>)> {
    let raw: List = serde_json::from_slice(out)
        .map_err(|e| io::Error::other(format!("claude plugin list: {e}")))?;
    let mut installed = Vec::new();
    for mut p in raw.installed.unwrap_or_default() {
        p.installed = true;
        let (name, market) = p.id.split_once('@').unwrap_or((&p.id, ""));
        (p.name, p.marketplace) = (name.to_string(), market.to_string());
        if p.name.is_empty() {
            p.name = p.id.clone();
        }
        read_manifest(&mut p);
        p.parts = plugin_parts(&p.install_path);
        installed.push(p);
    }
    installed.sort_by(|a, b| a.id.cmp(&b.id));
    let mut available: Vec<Plugin> = raw
        .available
        .unwrap_or_default()
        .into_iter()
        .map(|a| Plugin {
            id: a.plugin_id,
            ..a.plugin
        })
        .collect();
    available.sort_by_key(|p| std::cmp::Reverse(p.installs));
    Ok((installed, available))
}

/// The account's plugin marketplaces.
pub fn marketplaces(a: &Account, dir: &str) -> io::Result<Vec<Marketplace>> {
    let out = plugin_cli(a, dir, &["marketplace", "list", "--json"])?;
    serde_json::from_slice::<Option<Vec<Marketplace>>>(&out)
        .map(Option::unwrap_or_default)
        .map_err(|e| io::Error::other(format!("claude plugin marketplace list: {e}")))
}

static ALWAYS_ON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Always-on:\s*(~?[\d.,]+k?\s*tok)").unwrap());

/// Roughly how many tokens a plugin adds to every session, from `claude
/// plugin details`.
pub fn plugin_cost(a: &Account, dir: &str, id: &str) -> io::Result<String> {
    Ok(always_on(&plugin_cli(a, dir, &["details", id])?))
}

/// The always-on cost `claude plugin details` printed; "" when none.
pub fn always_on(out: &[u8]) -> String {
    ALWAYS_ON
        .captures(out)
        .map(|m| String::from_utf8_lossy(&m[1]).into_owned())
        .unwrap_or_default()
}

/// The `claude plugin` arguments that make change c.
pub fn change_args(c: &PluginChange) -> Vec<String> {
    let scope = if c.scope.is_empty() { "user" } else { &c.scope };
    let with_scope = |verb: &str| {
        vec![
            verb.to_string(),
            c.id.clone(),
            "--scope".into(),
            scope.to_string(),
        ]
    };
    match c.op {
        PluginOp::Enable => with_scope("enable"),
        PluginOp::Disable => with_scope("disable"),
        PluginOp::Update => with_scope("update"),
        PluginOp::Remove => with_scope("uninstall"),
        PluginOp::Install => with_scope("install"),
        PluginOp::MarketAdd => vec!["marketplace".into(), "add".into(), c.id.clone()],
        PluginOp::MarketUpdate => {
            let mut a = vec!["marketplace".to_string(), "update".into()];
            if !c.id.is_empty() {
                a.push(c.id.clone());
            }
            a
        }
        PluginOp::MarketRemove => vec!["marketplace".into(), "remove".into(), c.id.clone()],
    }
}

/// Makes a change through `claude plugin`.
pub fn change_plugins(a: &Account, dir: &str, c: &PluginChange) -> io::Result<()> {
    let args = change_args(c);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    plugin_cli(a, dir, &args).map(|_| ())
}

fn read_manifest(p: &mut Plugin) {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct M {
        description: Option<String>,
        version: Option<String>,
    }
    let Ok(b) = fs::read(
        Path::new(&p.install_path)
            .join(".claude-plugin")
            .join("plugin.json"),
    ) else {
        return;
    };
    if let Ok(m) = serde_json::from_slice::<M>(&b) {
        if p.description.is_empty() {
            p.description = m.description.unwrap_or_default();
        }
        if p.version.is_empty() {
            p.version = m.version.unwrap_or_default();
        }
    }
}

/// What a plugin folder holds: skills/<name>/SKILL.md, agents/*.md,
/// commands/*.md, hooks/hooks.json's events and .mcp.json's servers.
pub fn plugin_parts(dir: &str) -> PluginParts {
    let mut p = PluginParts::default();
    if dir.is_empty() {
        return p;
    }
    let dir = Path::new(dir);
    let sorted = |d: &Path| -> Vec<std::path::PathBuf> {
        let mut v: Vec<_> = fs::read_dir(d)
            .map(|r| r.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort();
        v
    };
    for s in sorted(&dir.join("skills")) {
        if s.join("SKILL.md").exists() {
            p.skills.push(base(&s));
        }
    }
    let md = |sub: &str| -> Vec<String> {
        sorted(&dir.join(sub))
            .into_iter()
            .filter(|f| f.extension().is_some_and(|x| x == "md"))
            .map(|f| base(&f.with_extension("")))
            .collect()
    };
    p.agents = md("agents");
    p.commands = md("commands");
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Hooks {
        hooks: Option<BTreeMap<String, Box<RawValue>>>,
    }
    if let Ok(b) = fs::read(dir.join("hooks").join("hooks.json"))
        && let Ok(h) = serde_json::from_slice::<Hooks>(&b)
    {
        p.hooks = h.hooks.unwrap_or_default().into_keys().collect();
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Mcp {
        #[serde(rename = "mcpServers")]
        servers: Option<BTreeMap<String, Box<RawValue>>>,
    }
    if let Ok(b) = fs::read(dir.join(".mcp.json"))
        && let Ok(m) = serde_json::from_slice::<Mcp>(&b)
    {
        p.mcp = m.servers.unwrap_or_default().into_keys().collect();
    }
    p
}

fn base(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_plugin_list() {
        let dir = tempfile::tempdir().unwrap();
        let inst = dir.path().join("design");
        fs::create_dir_all(inst.join(".claude-plugin")).unwrap();
        fs::write(
            inst.join(".claude-plugin/plugin.json"),
            r#"{"description":"draws","version":"1.2"}"#,
        )
        .unwrap();
        fs::create_dir_all(inst.join("skills/critique")).unwrap();
        fs::write(inst.join("skills/critique/SKILL.md"), "").unwrap();
        fs::create_dir_all(inst.join("agents")).unwrap();
        fs::write(inst.join("agents/rev.md"), "").unwrap();
        fs::create_dir_all(inst.join("hooks")).unwrap();
        fs::write(
            inst.join("hooks/hooks.json"),
            r#"{"hooks":{"Stop":[],"PreToolUse":[]}}"#,
        )
        .unwrap();
        fs::write(inst.join(".mcp.json"), r#"{"mcpServers":{"figma":{}}}"#).unwrap();
        let out = format!(
            r#"{{"installed":[{{"id":"design@x","enabled":true,"scope":"user","installPath":"{}"}}],
"available":[{{"pluginId":"a@x","name":"a","installCount":3}},{{"pluginId":"b@x","name":"b","installCount":30}}]}}"#,
            inst.display()
        );
        let (installed, available) = parse_plugins(out.as_bytes()).unwrap();
        let p = &installed[0];
        assert!(p.installed && p.enabled);
        assert_eq!((p.name.as_str(), p.marketplace.as_str()), ("design", "x"));
        assert_eq!(
            (p.description.as_str(), p.version.as_str()),
            ("draws", "1.2")
        );
        assert_eq!(p.parts.skills, vec!["critique"]);
        assert_eq!(p.parts.agents, vec!["rev"]);
        assert_eq!(p.parts.hooks, vec!["PreToolUse", "Stop"]);
        assert_eq!(p.parts.mcp, vec!["figma"]);
        let ids: Vec<&str> = available.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["b@x", "a@x"], "most installed first");
    }

    #[test]
    fn change_args_as_claude_takes_them() {
        let c = |op, id: &str, scope: &str| {
            change_args(&PluginChange {
                op,
                id: id.into(),
                scope: scope.into(),
            })
            .join(" ")
        };
        assert_eq!(c(PluginOp::Enable, "p@m", ""), "enable p@m --scope user");
        assert_eq!(
            c(PluginOp::Remove, "p@m", "project"),
            "uninstall p@m --scope project"
        );
        assert_eq!(c(PluginOp::MarketAdd, "o/r", ""), "marketplace add o/r");
        assert_eq!(c(PluginOp::MarketUpdate, "", ""), "marketplace update");
        assert_eq!(c(PluginOp::MarketUpdate, "m", ""), "marketplace update m");
    }

    #[test]
    fn always_on_cost() {
        assert_eq!(always_on(b"Name: x\nAlways-on: ~1.2k tok\n"), "~1.2k tok");
        assert_eq!(always_on(b"nothing"), "");
    }

    // A fake claude that fails, as the real one does when a plugin isn't
    // there: its last line of stderr is the error.
    #[test]
    fn a_failed_run_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("claude");
        fs::write(
            &bin,
            "#!/bin/sh\necho \"$@\" >&2\necho 'no such plugin' >&2\nexit 1\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let a = Account::at("", dir.path());
        let err = plugin_cli_with(&bin.to_string_lossy(), &a, "", &["details", "x"]).unwrap_err();
        assert_eq!(err.to_string(), "claude plugin details: no such plugin");
    }
}
