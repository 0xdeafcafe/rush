//! Plugins a project ships, in .rush/plugins/<name> with an install.sh
//! that builds it into the folder RUSH_PLUGIN_DIR names.
//! Ported from internal/plugin/project.go.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::bundled::bundle_named;
use crate::gopath;
use crate::manifest::{Manifest, name_ok};
use crate::origin::{record_origin, sources, tree};
use crate::plugin::{Plugin, installed, read_dir_sorted};
use crate::text::{clip, printable};

/// A plugin a repository ships.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Offer {
    pub name: String,
    pub description: String,
    pub version: String,
    /// its folder in the repository
    pub dir: String,
    /// the repository
    pub project: String,
    /// another version of it is installed
    pub update: bool,
    pub built_from: Vec<String>,
    /// For an update from the commit installed: that commit, the
    /// repository's now, and how many commits between changed the plugin.
    pub was: String,
    pub commit: String,
    pub commits: i64,
    /// What "not now" holds at: the sources' hash, else the version.
    pub(crate) mark: String,
}

fn offers_skipped_path() -> PathBuf {
    crate::root().join("offers-skipped.json")
}

fn read_skipped() -> BTreeMap<String, String> {
    std::fs::read(offers_skipped_path())
        .ok()
        .and_then(|b| rush_state::json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Lists what the repositories at roots ship that isn't installed, or is
/// newer than the commit installed (by version, for one installed before
/// rush kept the commit), and wasn't told "not now" at it.
pub fn in_projects(roots: &[String]) -> Vec<Offer> {
    let installed: HashMap<String, Plugin> = installed()
        .0
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();
    let skipped = read_skipped();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        let plugins = Path::new(root).join(".rush").join("plugins");
        for (entry, _) in read_dir_sorted(&plugins) {
            let dir = gopath::s(&plugins.join(&entry));
            let f = Path::new(&dir).join("plugin.json");
            let Ok(b) = std::fs::read(&f) else { continue };
            let Ok(m) = Manifest::parse(&b, false) else {
                continue;
            };
            if m.name != gopath::base(&dir) || !name_ok(&m.name) || seen.contains(&m.name) {
                continue;
            }
            if std::fs::metadata(Path::new(&dir).join("install.sh")).is_err() {
                continue;
            }
            if bundle_named(&m.name).is_some() {
                continue;
            }
            // A repository's words, drawn on rush's screen: what a terminal
            // would act on goes.
            let mut o = Offer {
                name: m.name.clone(),
                description: clip(&printable(&m.description), 300),
                version: clip(&printable(&m.version), 20),
                dir: dir.clone(),
                project: root.clone(),
                built_from: m.built_from.clone(),
                mark: m.version.clone(),
                ..Default::default()
            };
            if let Some(p) = installed.get(&m.name) {
                if p.origin.dev {
                    continue; // development mode keeps it as the files are
                } else if !p.origin.tree.is_empty() {
                    // Kept at the commit it was installed at, until a newer
                    // one changes it.
                    let rels = sources(root, &dir, &m.built_from);
                    let Some((head, n)) = p.origin.newer(root, &rels) else {
                        continue;
                    };
                    o.was = p.origin.commit.clone();
                    o.commit = head;
                    o.commits = n;
                    o.mark = tree(root, "HEAD", &rels);
                } else if p.version == m.version {
                    continue;
                }
                o.update = true;
            }
            if skipped.get(&m.name).is_some_and(|s| *s == o.mark) {
                continue;
            }
            seen.insert(m.name.clone());
            out.push(o);
        }
    }
    out
}

impl Offer {
    /// What install runs, to show before it does.
    pub fn install_script(&self) -> String {
        gopath::join(&[&self.dir, "install.sh"])
    }
}

/// Runs the offer's install.sh, as you and outside the sandbox, into
/// root()/<name>. What it built still waits on approval (pending).
pub fn install(o: &Offer) -> Result<(), String> {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg(o.install_script())
        .current_dir(&o.dir)
        .env("RUSH_PLUGIN_DIR", crate::root().join(&o.name))
        .env("RUSH_HOME", rush_state::dir());
    let (status, out) = crate::sys::combined_within(cmd, Duration::from_secs(300))
        .map_err(|e| format!("install.sh: {}", gopath::errno_text(&e)))?;
    if !status.success() {
        let text = String::from_utf8_lossy(&out);
        let lines: Vec<&str> = text.trim().split('\n').collect();
        let last = &lines[lines.len().saturating_sub(3)..];
        return Err(format!(
            "{}: {}: {}",
            gopath::base(&o.install_script()),
            exit_text(status),
            last.join(" · ")
        ));
    }
    record_origin(o)
}

/// How a program ended, as Go's exec error says it.
fn exit_text(st: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (st.code(), st.signal()) {
        (Some(c), _) => format!("exit status {c}"),
        (None, Some(libc::SIGKILL)) => "signal: killed".into(),
        (None, Some(s)) => format!("signal: {s}"),
        _ => "exit status -1".into(),
    }
}

/// Remembers "not now" for the offer as it is: a newer commit of it, or a
/// new version, is offered again.
pub fn skip(o: &Offer) -> Result<(), String> {
    let mut m = read_skipped();
    m.insert(o.name.clone(), o.mark.clone());
    crate::mkdir_all(&crate::root())?;
    let b = rush_state::json::to_vec_indent(&m).map_err(|e| e.to_string())?;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let p = offers_skipped_path();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&p)
        .map_err(|e| gopath::os_err("open", &p, &e))?;
    f.write_all(&b).map_err(|e| gopath::os_err("write", &p, &e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::origins;
    use crate::plugin::tests::Home;
    use crate::plugin::{approve, digest, load, pending};

    const INSTALL: &str = r#"set -eu
mkdir -p "$RUSH_PLUGIN_DIR"
printf '#!/bin/sh\n' > "$RUSH_PLUGIN_DIR/demo"
chmod +x "$RUSH_PLUGIN_DIR/demo"
cp plugin.json "$RUSH_PLUGIN_DIR/"
"#;

    fn tmp() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        let p = gopath::s(&std::fs::canonicalize(d.path()).unwrap());
        (d, p)
    }

    fn git_ok(repo: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-C",
                repo,
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn in_projects_offers_installs_and_skips() {
        let _h = Home::new();
        let (_r, repo) = tmp();
        let dir = Path::new(&repo).join(".rush/plugins/demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"demo","version":"1","command":["demo"]}"#,
        )
        .unwrap();
        assert!(
            in_projects(std::slice::from_ref(&repo)).is_empty(),
            "no install.sh, no offer"
        );
        std::fs::write(dir.join("install.sh"), INSTALL).unwrap();
        let got = in_projects(&[repo.clone(), repo.clone()]);
        assert_eq!(got.len(), 1, "one offer, once: {got:?}");
        assert!(got[0].name == "demo" && !got[0].update && got[0].project == repo);
        install(&got[0]).unwrap();
        assert!(
            in_projects(std::slice::from_ref(&repo)).is_empty(),
            "installed at that version"
        );
        assert_eq!(pending().len(), 1, "what it built waits on approval");
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"demo","version":"2","command":["demo"]}"#,
        )
        .unwrap();
        let got = in_projects(std::slice::from_ref(&repo));
        assert!(
            got.len() == 1 && got[0].update,
            "a new version is an update: {got:?}"
        );
        skip(&got[0]).unwrap();
        assert!(
            in_projects(std::slice::from_ref(&repo)).is_empty(),
            "not now at version 2"
        );
    }

    #[test]
    fn install_records_where_it_came_from() {
        let _h = Home::new();
        let (_r, repo) = tmp();
        let dir = Path::new(&repo).join(".rush/plugins/demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"demo","version":"1","command":["demo"]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("install.sh"), INSTALL).unwrap();
        git_ok(&repo, &["init", "-q"]);
        git_ok(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://someone:secret@github.com/acme/haven.git",
            ],
        );
        git_ok(&repo, &["add", "."]);
        git_ok(&repo, &["commit", "-qm", "x"]);
        install(&in_projects(std::slice::from_ref(&repo))[0]).unwrap();
        let p = load(&crate::root().join("demo")).unwrap();
        let o = &p.origin;
        assert!(
            o.project == repo
                && o.remote == "github.com/acme/haven"
                && o.commit.len() == 40
                && !o.uncommitted
                && o.version == "1",
            "origin: {o:?}"
        );
        let f = p.from();
        assert!(
            f.contains("github.com/acme/haven")
                && !f.contains("secret")
                && f.contains(&o.commit[..7]),
            "From: {f}"
        );
        let d = digest(Path::new(&p.dir)).unwrap();
        assert!(!o.changed_since_install(&d), "not changed yet");
        std::fs::write(Path::new(&p.dir).join("extra"), "x").unwrap();
        assert!(
            o.changed_since_install(&digest(Path::new(&p.dir)).unwrap()),
            "changed since install"
        );

        let by_hand = crate::root().join("other");
        std::fs::create_dir_all(&by_hand).unwrap();
        let f = Plugin {
            dir: gopath::s(&by_hand),
            origin: crate::origin::origin_of("other", &by_hand),
            ..Default::default()
        }
        .from();
        assert!(f.contains("by hand"), "by hand: {f}");
        let link = crate::root().join("linked");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let f = Plugin {
            dir: gopath::s(&link),
            origin: crate::origin::origin_of("linked", &link),
            ..Default::default()
        }
        .from();
        assert!(f.starts_with("Linked to"), "link: {f}");
    }

    /// A repository shipping the plugin demo, built from its folder and
    /// lib/, committed.
    fn git_repo() -> (tempfile::TempDir, String, PathBuf) {
        let (r, repo) = tmp();
        let dir = Path::new(&repo).join(".rush/plugins/demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(Path::new(&repo).join("lib")).unwrap();
        std::fs::write(Path::new(&repo).join("lib/code"), "1").unwrap();
        std::fs::write(Path::new(&repo).join("README"), "x").unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"demo","version":"1","command":["demo"],"builtFrom":["../../../lib"]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("install.sh"),
            r#"set -eu
mkdir -p "$RUSH_PLUGIN_DIR"
printf '#!/bin/sh\n' > "$RUSH_PLUGIN_DIR/demo"
cat ../../../lib/code >> "$RUSH_PLUGIN_DIR/demo"
chmod +x "$RUSH_PLUGIN_DIR/demo"
cp plugin.json "$RUSH_PLUGIN_DIR/"
"#,
        )
        .unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        git_ok(&repo, &["add", "."]);
        git_ok(&repo, &["commit", "-qm", "one"]);
        (r, repo, dir)
    }

    #[test]
    fn locked_until_a_newer_commit_changes_it() {
        let _h = Home::new();
        let (_r, repo, _) = git_repo();
        let roots = std::slice::from_ref(&repo);
        install(&in_projects(roots)[0]).unwrap();
        let o = origins().remove("demo").unwrap();
        assert!(
            !o.tree.is_empty() && o.sources.len() == 2 && o.sources[1] == "lib",
            "origin: {o:?}"
        );
        let w = |p: &str, s: &str| std::fs::write(Path::new(&repo).join(p), s).unwrap();
        // A commit that doesn't touch the plugin's sources isn't an update.
        w("README", "y");
        git_ok(&repo, &["commit", "-qam", "readme"]);
        assert!(in_projects(roots).is_empty(), "its files didn't change");
        // Nor is an edit not yet committed.
        w("lib/code", "2");
        assert!(in_projects(roots).is_empty(), "not committed");
        // A commit to what it's built from is, with the version unchanged.
        git_ok(&repo, &["commit", "-qam", "two"]);
        let got = in_projects(roots);
        assert!(
            got.len() == 1
                && got[0].update
                && got[0].was == origins()["demo"].commit
                && got[0].commits == 1,
            "an update: {got:?}"
        );
        // A checkout behind the commit installed doesn't offer to go back.
        let old = git_ok(&repo, &["rev-parse", "HEAD~1"]);
        install(&got[0]).unwrap();
        git_ok(&repo, &["checkout", "-q", &old]);
        assert!(in_projects(roots).is_empty(), "behind what's installed");
        git_ok(&repo, &["checkout", "-q", "main"]);
        // "Not now" holds until a newer commit changes it again.
        w("lib/code", "3");
        git_ok(&repo, &["commit", "-qam", "three"]);
        let got = in_projects(roots);
        assert_eq!(got.len(), 1, "three: {got:?}");
        skip(&got[0]).unwrap();
        assert!(in_projects(roots).is_empty(), "not now");
        w("lib/code", "4");
        git_ok(&repo, &["commit", "-qam", "four"]);
        let got = in_projects(roots);
        assert!(
            got.len() == 1 && got[0].commits == 2,
            "four, two commits on from the one installed: {got:?}"
        );
    }

    #[test]
    fn development_mode_rebuilds_as_files_change() {
        use crate::origin::{dev_changed, rebuild, set_dev};
        let _h = Home::new();
        let (_r, repo, dir) = git_repo();
        let roots = std::slice::from_ref(&repo);
        assert!(
            set_dev("demo", true).is_err(),
            "not installed from a repository yet"
        );
        install(&in_projects(roots)[0]).unwrap();
        approve(&load(&crate::root().join("demo")).unwrap()).unwrap();
        assert!(dev_changed().is_empty(), "not in development mode");
        set_dev("demo", true).unwrap();
        assert!(
            dev_changed().contains_key("demo"),
            "turning it on builds it from the files as they are"
        );
        assert_eq!(rebuild("demo"), Ok(true), "rebuilt and approved");
        assert!(dev_changed().is_empty(), "built as the files are");
        assert!(
            in_projects(roots).is_empty(),
            "development mode isn't offered updates"
        );
        // An edit, not committed, is built and runs without asking. The
        // edit has a new size, so the quick hash sees it whatever the
        // clock's grain.
        std::fs::write(Path::new(&repo).join("lib/code"), "edited").unwrap();
        assert!(dev_changed().contains_key("demo"), "a file changed");
        assert_eq!(rebuild("demo"), Ok(true), "rebuilt");
        let built = std::fs::read_to_string(crate::root().join("demo/demo")).unwrap();
        assert!(built.contains("edited"), "built from the edit: {built:?}");
        assert!(pending().is_empty(), "approved as it asks for no more");
        let p = load(&crate::root().join("demo")).unwrap();
        assert!(p.from().contains("development mode"), "From: {}", p.from());
        // One that asks for more waits on you.
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"demo","version":"1","command":["demo"],"builtFrom":["../../../lib"],"network":["example.com:443"]}"#,
        )
        .unwrap();
        assert_eq!(rebuild("demo"), Ok(false), "asks for more");
        assert_eq!(pending().len(), 1, "waits on approval");
        set_dev("demo", false).unwrap();
        assert!(rebuild("demo").is_err(), "off: no rebuilding");
    }
}
