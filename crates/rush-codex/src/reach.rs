//! What a Codex thread rush starts reads and may write beside Claude Code's
//! sessions on the same project: Claude Code's instruction files, and the
//! folders a commit and rush's own commands write. Rust-only: Go's adapter
//! left Codex to its own AGENTS.md and sandbox.

use std::path::Path;
use std::process::Command;

/// The instruction file Codex reads in a folder with no AGENTS.md: Claude
/// Code's, so a project written for either reads the same to both.
pub(crate) const CLAUDE_DOC: &str = "CLAUDE.md";

/// The names Codex reads in a folder with no AGENTS.md: config.toml's own
/// (in codex_home), then CLAUDE.md.
pub(crate) fn doc_fallbacks(codex_home: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_to_string(Path::new(codex_home).join("config.toml"))
        .ok()
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .and_then(|t| t.get("project_doc_fallback_filenames").cloned())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if !names.iter().any(|n| n == CLAUDE_DOC) {
        names.push(CLAUDE_DOC.into());
    }
    names
}

/// The user's own instructions to every project, from Claude Code's
/// CLAUDE.md in claude_home, for a Codex whose home (codex_home) has no
/// AGENTS.md of its own; "" when it has one, or there's none to read.
pub(crate) fn user_doc(codex_home: &str, claude_home: &str) -> String {
    let own = ["AGENTS.override.md", "AGENTS.md"]
        .iter()
        .any(|n| Path::new(codex_home).join(n).is_file());
    if own || claude_home.is_empty() {
        return String::new();
    }
    let path = Path::new(claude_home).join(CLAUDE_DOC);
    match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => format!(
            "The user's own instructions for every project, from {}:\n\n{}",
            path.display(),
            t.trim()
        ),
        _ => String::new(),
    }
}

/// Claude Code's home: CLAUDE_CONFIG_DIR, else ~/.claude.
pub(crate) fn claude_home() -> String {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return d.to_string_lossy().into_owned();
    }
    match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        Some(h) => Path::new(&h).join(".claude").to_string_lossy().into_owned(),
        None => String::new(),
    }
}

/// What a thread in dir may write besides dir in Codex's workspace sandbox:
/// the repository's git folders, which Codex keeps read-only even inside a
/// writable root and which a worktree's live outside it, so a commit needs
/// them; and rush's own folder (rush_home), which rush feed, memory, watch
/// and the rest write.
pub(crate) fn writable_roots(dir: &str, rush_home: &str) -> Vec<String> {
    let mut roots = git_dirs(dir);
    if !rush_home.is_empty() {
        roots.push(rush_home.to_string());
    }
    roots.dedup();
    roots
}

/// The repository's shared git folder and the worktree's own, absolute;
/// none outside a repository.
fn git_dirs(dir: &str) -> Vec<String> {
    if dir.is_empty() {
        return Vec::new();
    }
    let Ok(out) = Command::new("git")
        .args([
            "-C",
            dir,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut dirs: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    dirs.dedup();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    // A worktree's commit writes its own git folder and the shared one;
    // a plain checkout's, its .git; outside a repository, only rush's.
    #[test]
    fn roots_for_a_commit() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let main = root.join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&main, &["worktree", "add", "-q", "../wt", "-b", "wt"]);
        let g = main.join(".git");
        let s = |p: &Path| p.to_string_lossy().into_owned();
        for (dir, want) in [
            (
                root.join("wt"),
                vec![s(&g), s(&g.join("worktrees").join("wt")), "/rush".into()],
            ),
            (main.clone(), vec![s(&g), "/rush".into()]),
            (root.clone(), vec!["/rush".into()]),
        ] {
            assert_eq!(writable_roots(&s(&dir), "/rush"), want, "{dir:?}");
        }
        assert!(writable_roots("", "").is_empty());
    }

    #[test]
    fn claude_md_read_where_agents_md_isnt() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_string_lossy().into_owned();
        assert_eq!(doc_fallbacks(&home), vec!["CLAUDE.md"]);
        std::fs::write(
            d.path().join("config.toml"),
            "project_doc_fallback_filenames = [\"README.agents.md\"]\n",
        )
        .unwrap();
        assert_eq!(doc_fallbacks(&home), vec!["README.agents.md", "CLAUDE.md"]);
        std::fs::write(
            d.path().join("config.toml"),
            "project_doc_fallback_filenames = [\"CLAUDE.md\"]\n",
        )
        .unwrap();
        assert_eq!(doc_fallbacks(&home), vec!["CLAUDE.md"]);
    }

    // Claude Code's user CLAUDE.md goes to a Codex with no AGENTS.md of
    // its own; one with its own reads that.
    #[test]
    fn user_doc_unless_codex_has_its_own() {
        let codex = tempfile::tempdir().unwrap();
        let claude = tempfile::tempdir().unwrap();
        let (cx, cl) = (
            codex.path().to_string_lossy().into_owned(),
            claude.path().to_string_lossy().into_owned(),
        );
        assert_eq!(user_doc(&cx, &cl), "");
        std::fs::write(claude.path().join("CLAUDE.md"), "Be brief.\n").unwrap();
        let got = user_doc(&cx, &cl);
        assert!(got.ends_with(":\n\nBe brief."), "{got}");
        assert!(got.contains("CLAUDE.md"), "{got}");
        std::fs::write(codex.path().join("AGENTS.md"), "Mine.").unwrap();
        assert_eq!(user_doc(&cx, &cl), "");
        assert_eq!(user_doc(&cx, ""), "");
    }
}
