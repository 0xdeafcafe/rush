//! Where an install came from, and how to update it.
//!
//! Ported from internal/harnessup/plan.go:1-95.

use std::path::Path;

/// Where an agent's program says it's published (Go `agent.Published`):
/// its npm or PyPI package, and the arguments of its own updater.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Published {
    pub npm: String,
    pub pypi: String,
    pub update: Vec<String>,
}

/// How to bring a harness up to date: label for the flash and the page,
/// argv to run (the first word is found as rush finds agents).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub label: String,
    pub argv: Vec<String>,
}

/// Where an install came from, read from its real path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// A Homebrew formula's name.
    pub brew: String,
    /// A Homebrew cask's name.
    pub cask: String,
    /// The npm package it sits in.
    pub npm: String,
    /// The node package manager that put it there.
    pub manager: String,
    /// The uv tool it is.
    pub uv: String,
    /// The pipx tool it is.
    pub pipx: String,
}

/// Reads where a program at real (its path past symlinks and rush's
/// stand-ins) was installed from.
pub fn source_of(real: &str) -> Source {
    let mut s = Source::default();
    let parts: Vec<&str> = real.split('/').collect();
    let after = |dir: &str| {
        parts
            .iter()
            .position(|p| *p == dir)
            .filter(|&i| i + 1 < parts.len())
            .map(|i| parts[i + 1].to_string())
    };
    if let Some(n) = after("Caskroom") {
        s.cask = n;
    }
    if let Some(n) = after("Cellar") {
        s.brew = n;
    }
    if let Some(mut n) = after("node_modules") {
        if n.starts_with('@') {
            for (i, p) in parts.iter().enumerate() {
                if *p == "node_modules" && i + 2 < parts.len() {
                    n.push('/');
                    n.push_str(parts[i + 2]);
                }
            }
        }
        s.npm = n;
        s.manager = if real.contains("/.bun/") {
            "bun"
        } else if real.contains("pnpm") {
            "pnpm"
        } else {
            "npm"
        }
        .into();
    }
    if let Some(n) = after("tools")
        && real.contains("/uv/tools/")
    {
        s.uv = n;
    }
    if let Some(n) = after("venvs")
        && real.contains("/pipx/venvs/")
    {
        s.pipx = n;
    }
    s
}

/// How to update the program at path (s is where it resolves from): brew
/// when brew installed it, else the program's own updater, else the
/// package manager it sits under. Empty when none.
pub fn plan_for(publ: &Published, path: &str, s: &Source) -> Update {
    let u = |label: String, argv: &[&str]| Update {
        label,
        argv: argv.iter().map(|a| a.to_string()).collect(),
    };
    if !s.cask.is_empty() {
        return u(
            format!("brew upgrade --cask {}", s.cask),
            &["brew", "upgrade", "--cask", &s.cask],
        );
    }
    if !s.brew.is_empty() {
        return u(
            format!("brew upgrade {}", s.brew),
            &["brew", "upgrade", &s.brew],
        );
    }
    if !publ.update.is_empty() {
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        let mut argv = vec![path.to_string()];
        argv.extend(publ.update.iter().cloned());
        return Update {
            label: format!("{name} {}", publ.update.join(" ")),
            argv,
        };
    }
    if !s.npm.is_empty() {
        let pkg = format!("{}@latest", s.npm);
        if s.manager == "npm" {
            return u(
                format!("npm install -g {pkg}"),
                &["npm", "install", "-g", &pkg],
            );
        }
        return u(
            format!("{} add -g {pkg}", s.manager),
            &[&s.manager, "add", "-g", &pkg],
        );
    }
    if !s.uv.is_empty() {
        return u(
            format!("uv tool upgrade {}", s.uv),
            &["uv", "tool", "upgrade", &s.uv],
        );
    }
    if !s.pipx.is_empty() {
        return u(
            format!("pipx upgrade {}", s.pipx),
            &["pipx", "upgrade", &s.pipx],
        );
    }
    Update::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        let claude = Published {
            npm: "@anthropic-ai/claude-code".into(),
            update: vec!["update".into()],
            ..Published::default()
        };
        let none = Published::default();
        for (publ, path, real, want) in [
            (
                &claude,
                "/opt/homebrew/bin/claude",
                "/opt/homebrew/Caskroom/claude-code@latest/2.1.284/claude",
                "brew upgrade --cask claude-code@latest",
            ),
            (
                &claude,
                "/Users/x/.local/bin/claude",
                "/Users/x/.local/share/claude/versions/2.1.284",
                "claude update",
            ),
            (
                &none,
                "/opt/homebrew/bin/gemini",
                "/opt/homebrew/lib/node_modules/@google/gemini-cli/bundle/gemini.js",
                "npm install -g @google/gemini-cli@latest",
            ),
            (
                &none,
                "/x/pnpm/pi",
                "/x/pnpm/global/5/node_modules/@earendil-works/pi-coding-agent/cli.js",
                "pnpm add -g @earendil-works/pi-coding-agent@latest",
            ),
            (
                &none,
                "/opt/homebrew/bin/vibe",
                "/opt/homebrew/Cellar/mistral-vibe/2.25.0/libexec/bin/vibe",
                "brew upgrade mistral-vibe",
            ),
            (
                &none,
                "/Users/x/.kimi-code/bin/kimi",
                "/Users/x/.kimi-code/bin/kimi",
                "",
            ),
            (
                &none,
                "/Users/x/.local/bin/kimi",
                "/Users/x/.local/share/uv/tools/kimi-cli/bin/kimi",
                "uv tool upgrade kimi-cli",
            ),
            (
                &none,
                "/Users/x/.local/bin/aider",
                "/Users/x/.local/pipx/venvs/aider-chat/bin/aider",
                "pipx upgrade aider-chat",
            ),
            (
                &none,
                "/Users/x/.bun/bin/x",
                "/Users/x/.bun/install/global/node_modules/x-cli/bin.js",
                "bun add -g x-cli@latest",
            ),
        ] {
            let got = plan_for(publ, path, &source_of(real));
            assert_eq!(got.label, want, "plan at {real}");
            assert_eq!(got.argv.is_empty(), want.is_empty());
        }
        assert_eq!(
            plan_for(&claude, "/Users/x/.local/bin/claude", &Source::default()).argv,
            ["/Users/x/.local/bin/claude", "update"]
        );
    }
}
