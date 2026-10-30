//! Where a plugin came from, and keeping it at a version.
//! Ported from internal/plugin/origin.go.
//!
//! A plugin a repository ships stays at the commit it was installed at:
//! in_projects offers an update only once a newer commit changes its
//! sources (its folder there and its manifest's builtFrom). In development
//! mode it doesn't wait for a commit: the broker installs it again from the
//! repository as it is whenever a file in its sources changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use rush_state::Time;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::gopath::{self, file_mode, mod_nanos, quote};
use crate::jsonshape::{self, Shape};
use crate::manifest::Manifest;
use crate::plugin::{Plugin, approvals, approve, digest, hex, load};
use crate::project::{Offer, install};
use crate::text::{go_q, printable};

/// Where an installed plugin came from, as rush saw it when it installed
/// it. A plugin put in root() by hand has none recorded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Origin {
    /// the repository it was installed from
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// that repository's origin, without any credentials
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    /// the repository's commit then
    #[serde(skip_serializing_if = "String::is_empty")]
    pub commit: String,
    /// its sources there had changes not yet committed
    #[serde(skip_serializing_if = "is_false")]
    pub uncommitted: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// what the install built, to tell if it changed since
    #[serde(skip_serializing_if = "String::is_empty")]
    pub digest: String,
    #[serde(rename = "installedAt", skip_serializing_if = "Time::is_zero_ref")]
    pub installed_at: Time,
    /// Its folder in the repository, then its builtFrom, as paths in the
    /// repository.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// git's hash of the sources at commit: what a newer commit must change
    /// to be offered as an update.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tree: String,
    /// Development mode: rebuilt as its sources change.
    #[serde(skip_serializing_if = "is_false")]
    pub dev: bool,
    /// A quick hash of the sources' files as installed (names, sizes,
    /// times), for development mode to see them change.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub files: String,
    /// Where its folder in root() points, when it's a link to a plugin
    /// being worked on. Read, not recorded.
    #[serde(skip)]
    pub link: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

trait ZeroRef {
    fn is_zero_ref(&self) -> bool;
}

impl ZeroRef for Time {
    fn is_zero_ref(&self) -> bool {
        self.is_zero()
    }
}

fn origins_path() -> PathBuf {
    crate::root().join("origins.json")
}

/// Keeps the UI's, the CLI's and the broker's writes in one process from
/// losing each other's.
static ORIGINS_MU: Mutex<()> = Mutex::new(());

const ORIGIN_SHAPE: Shape = Shape::Struct(
    "plugin.Origin",
    &[
        ("project", Shape::Any),
        ("remote", Shape::Any),
        ("commit", Shape::Any),
        ("uncommitted", Shape::Any),
        ("version", Shape::Any),
        ("digest", Shape::Any),
        ("installedAt", Shape::Any),
        ("sources", Shape::Any),
        ("tree", Shape::Any),
        ("dev", Shape::Any),
        ("files", Shape::Any),
    ],
);
const ORIGINS_SHAPE: Shape = Shape::Map(&ORIGIN_SHAPE);

/// Every recorded origin, by plugin name.
pub fn origins() -> BTreeMap<String, Origin> {
    std::fs::read(origins_path())
        .ok()
        .and_then(|b| jsonshape::decode(&b, &ORIGINS_SHAPE, false).ok())
        .unwrap_or_default()
}

fn update_origins(
    f: impl FnOnce(&mut BTreeMap<String, Origin>) -> Result<(), String>,
) -> Result<(), String> {
    let _g = ORIGINS_MU.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = origins();
    f(&mut all)?;
    crate::mkdir_all(&crate::root())?;
    let b = rush_state::json::to_vec_indent(&all).map_err(|e| e.to_string())?;
    crate::write_atomic(&origins_path(), &b)
}

/// Where the plugin named name, in dir, came from.
pub(crate) fn origin_of(name: &str, dir: &Path) -> Origin {
    let mut o = origins().remove(name).unwrap_or_default();
    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink())
        && let Ok(t) = std::fs::canonicalize(dir)
    {
        o.link = gopath::s(&t);
    }
    o
}

/// Runs git in root, "" when it fails.
pub(crate) fn git(root: &str, args: &[&str]) -> String {
    let mut c = Command::new("git");
    c.arg("-C").arg(root).args(args);
    match crate::sys::output_within(c, Duration::from_secs(10)) {
        Some(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => String::new(),
    }
}

/// What the plugin in dir, in the repository at root, is built from, as
/// paths in the repository: dir, then builtFrom. One that leaves the
/// repository is left out.
pub(crate) fn sources(root: &str, dir: &str, built_from: &[String]) -> Vec<String> {
    let Ok(rel) = gopath::rel(root, dir) else {
        return Vec::new();
    };
    let mut out = vec![rel.clone()];
    for b in built_from {
        let p = gopath::clean(&gopath::join(&[&rel, b]));
        if p == ".." || p.starts_with("../") || gopath::is_abs(&p) {
            continue;
        }
        out.push(p);
    }
    out
}

/// git's hash of rels at rev, together: "" when one isn't there.
pub(crate) fn tree(root: &str, rev: &str, rels: &[String]) -> String {
    let specs: Vec<String> = rels.iter().map(|r| format!("{rev}:{r}")).collect();
    let mut args = vec!["rev-parse"];
    args.extend(specs.iter().map(String::as_str));
    git(root, &args)
}

/// A quick hash of the files under rels in root: names, sizes, modes and
/// times, not contents, so looking every few seconds stays cheap. Folders
/// git and package managers fill are passed over.
pub(crate) fn files_hash(root: &str, rels: &[String]) -> String {
    let mut h = Sha256::new();
    let mut n = 0usize;
    let root_p = Path::new(root);
    for rel in rels {
        // Past the limit, each walk after stops at its first entry.
        let base = PathBuf::from(gopath::join(&[root, rel]));
        walk_files(root_p, &base, &base, &mut n, &mut h);
    }
    hex(&h.finalize())
}

/// Hashes path and what's under it; false once past 50,000 entries, when
/// Go's walk stops altogether.
fn walk_files(root: &Path, base: &Path, path: &Path, n: &mut usize, h: &mut Sha256) -> bool {
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return true;
    };
    let is_dir = md.file_type().is_dir();
    if is_dir && path != base {
        let name = path.file_name().map(|s| s.to_string_lossy());
        if matches!(name.as_deref(), Some(".git" | "node_modules")) {
            return true;
        }
    }
    *n += 1;
    if *n > 50000 {
        return false;
    }
    use std::os::unix::ffi::OsStrExt;
    let rel = gopath::rel(&gopath::s(root), &gopath::s(path)).unwrap_or_default();
    h.update(
        format!(
            "{} {:o} {} {}\n",
            quote(rel.as_bytes()),
            file_mode(&md),
            md.len(),
            mod_nanos(&md)
        )
        .as_bytes(),
    );
    if is_dir {
        let mut names: Vec<std::ffi::OsString> = match std::fs::read_dir(path) {
            Ok(rd) => rd.flatten().map(|e| e.file_name()).collect(),
            Err(_) => return true,
        };
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for name in names {
            if !walk_files(root, base, &path.join(name), n, h) {
                return false;
            }
        }
    }
    true
}

/// Notes that the offer was just installed, from its repository as it is
/// now. Development mode, if it was on, stays on.
pub(crate) fn record_origin(o: &Offer) -> Result<(), String> {
    let rels = sources(&o.project, &o.dir, &o.built_from);
    let d = digest(&crate::root().join(&o.name)).unwrap_or_default();
    let mut st_args = vec!["status", "--porcelain", "--"];
    st_args.extend(rels.iter().map(String::as_str));
    let status = git(&o.project, &st_args);
    let mut rec = Origin {
        project: o.project.clone(),
        remote: clean_remote(&git(&o.project, &["remote", "get-url", "origin"])),
        commit: git(&o.project, &["rev-parse", "HEAD"]),
        uncommitted: !status.is_empty(),
        version: o.version.clone(),
        digest: d,
        installed_at: Time::now(),
        tree: tree(&o.project, "HEAD", &rels),
        files: files_hash(&o.project, &rels),
        sources: rels,
        ..Default::default()
    };
    update_origins(|all| {
        rec.dev = all.get(&o.name).is_some_and(|x| x.dev);
        all.insert(o.name.clone(), rec);
        Ok(())
    })
}

impl Origin {
    /// Whether the repository at root has a newer commit of what the
    /// installed plugin was built from: its sources differ from those
    /// installed, and the commit installed is behind root's. A checkout
    /// behind it, or on a branch apart, isn't offered, so worktrees on
    /// older branches don't offer to undo an update. It's the commit
    /// there and how many commits since changed the sources.
    pub(crate) fn newer(&self, root: &str, rels: &[String]) -> Option<(String, i64)> {
        if self.tree.is_empty() || self.commit.is_empty() {
            return None;
        }
        let t = tree(root, "HEAD", rels);
        if t.is_empty() || t == self.tree {
            return None;
        }
        let mut c = Command::new("git");
        c.args([
            "-C",
            root,
            "merge-base",
            "--is-ancestor",
            &self.commit,
            "HEAD",
        ]);
        if !crate::sys::output_within(c, Duration::from_secs(60))
            .is_some_and(|o| o.status.success())
        {
            return None;
        }
        let range = format!("{}..HEAD", self.commit);
        let mut args = vec!["rev-list", "--count", &range, "--"];
        args.extend(rels.iter().map(String::as_str));
        let n = git(root, &args).parse().unwrap_or(0);
        Some((git(root, &["rev-parse", "HEAD"]), n))
    }

    /// Whether the files, whose digest is d, aren't what the install
    /// built.
    pub fn changed_since_install(&self, d: &str) -> bool {
        !self.digest.is_empty() && !d.is_empty() && self.link.is_empty() && d != self.digest
    }
}

/// Turns development mode on or off for an installed plugin a repository
/// ships. On, the broker installs it again from the repository as it is
/// now, without waiting for a commit, and again whenever its files change.
pub fn set_dev(name: &str, on: bool) -> Result<(), String> {
    update_origins(|all| {
        let Some(o) = all.get_mut(name).filter(|o| !o.project.is_empty()) else {
            return Err(format!(
                "{name} wasn't installed from a repository: development mode rebuilds it from one"
            ));
        };
        o.dev = on;
        if on {
            o.files.clear(); // build it from the files as they are now
        }
        Ok(())
    })
}

/// The plugins in development mode whose sources' files changed since
/// they were last installed, with the hash they have now.
pub fn dev_changed() -> BTreeMap<String, String> {
    origins()
        .into_iter()
        .filter(|(_, o)| o.dev && !o.project.is_empty())
        .filter_map(|(name, o)| {
            let h = files_hash(&o.project, &o.sources);
            (h != o.files).then_some((name, h))
        })
        .collect()
}

/// Installs a plugin in development mode again from its repository as it
/// is, and approves what it built if it asks for nothing more than was
/// approved: a change to what it may do still waits on you. It says
/// whether it approved it.
pub fn rebuild(name: &str) -> Result<bool, String> {
    let Some(o) = origins()
        .remove(name)
        .filter(|o| o.dev && !o.sources.is_empty())
    else {
        return Err(format!("{name} isn't in development mode"));
    };
    let dir = gopath::join(&[&o.project, &o.sources[0]]);
    let f = Path::new(&dir).join("plugin.json");
    let b = std::fs::read(&f).map_err(|e| gopath::os_err("open", &f, &e))?;
    let m = Manifest::parse(&b, false).map_err(|e| format!("plugin.json: {e}"))?;
    if m.name != name {
        return Err(format!("{dir}: plugin.json names {}", go_q(&m.name)));
    }
    install(&Offer {
        name: name.to_string(),
        version: m.version.clone(),
        dir: dir.clone(),
        project: o.project.clone(),
        built_from: m.built_from.clone(),
        ..Default::default()
    })?;
    let p = load(&crate::root().join(name))?;
    match approvals().get(name) {
        Some(a) if same_asks(&a.manifest, &p.manifest) => approve(&p).map(|_| true),
        _ => Ok(false),
    }
}

/// Whether two manifests ask for the same: only their description and
/// version may differ.
fn same_asks(a: &Manifest, b: &Manifest) -> bool {
    let strip = |m: &Manifest| {
        let mut m = m.clone();
        m.description.clear();
        m.version.clear();
        rush_state::json::to_vec(&m).ok()
    };
    match (strip(a), strip(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// A git remote as a place, "github.com/owner/repo", with any user or
/// token in it dropped.
pub(crate) fn clean_remote(r: &str) -> String {
    let r = r.strip_suffix(".git").unwrap_or(r);
    if let Some((host, path)) = url_host_path(r)
        && !host.is_empty()
    {
        return format!("{host}{path}");
    }
    // scp-like: git@github.com:owner/repo
    let r = match r.rfind('@') {
        Some(at) => &r[at + 1..],
        None => r,
    };
    r.replacen(':', "/", 1)
}

/// The host and path of a URL with a scheme and an authority, as Go's
/// url.Parse finds them; None for anything else.
fn url_host_path(r: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = r.split_once("://")?;
    let mut sc = scheme.bytes();
    if !sc.next()?.is_ascii_alphabetic()
        || !sc.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
    {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = match auth.rfind('@') {
        Some(at) => &auth[at + 1..],
        None => auth,
    };
    Some((host, path))
}

impl Plugin {
    /// Where p came from, in a line: "From ~/Source/haven
    /// (github.com/acme/haven) at commit 1a2b3c4, on 25 Oct 2026".
    pub fn from(&self) -> String {
        let o = &self.origin;
        if self.bundled {
            return "Comes with rush".into();
        }
        if !o.link.is_empty() {
            return format!("Linked to {}, where it's being worked on", home(&o.link));
        }
        if o.project.is_empty() {
            return format!(
                "Put in {} by hand: rush doesn't know where it came from",
                home(&self.dir)
            );
        }
        let mut s = format!("From {}", home(&o.project));
        if !o.remote.is_empty() {
            s += &format!(" ({})", printable(&o.remote));
        }
        if o.dev {
            return s + ", in development mode: rebuilt as its files change";
        }
        if !o.commit.is_empty() {
            s += &format!(" at commit {}", short_commit(&o.commit));
            if o.uncommitted {
                s += " plus changes not yet committed";
            }
        }
        if !o.installed_at.is_zero() {
            s += &format!(", on {}", day(o.installed_at));
        }
        s
    }
}

/// A time as Go's "2 Jan 2006" writes it, in the zone it was kept in.
fn day(t: Time) -> String {
    let f = t.format(); // 2006-01-02T…
    let (y, m, d) = (&f[0..4], &f[5..7], &f[8..10]);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mi: usize = m.parse().unwrap_or(1);
    let di: u32 = d.parse().unwrap_or(1);
    format!("{di} {} {y}", MONTHS[(mi.clamp(1, 12)) - 1])
}

/// A commit as people read it: its first seven characters.
pub fn short_commit(commit: &str) -> &str {
    let end = commit
        .char_indices()
        .nth(7)
        .map_or(commit.len(), |(i, _)| i);
    &commit[..end]
}

/// p with your home folder as ~.
pub(crate) fn home(p: &str) -> String {
    let h = std::env::var("HOME").unwrap_or_default();
    if !h.is_empty() {
        if p == h {
            return "~".into();
        }
        if let Some(rest) = p.strip_prefix(&h)
            && rest.starts_with('/')
        {
            return format!("~{rest}");
        }
    }
    p.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_remote_as_go() {
        for (i, w) in [
            ("git@github.com:acme/haven.git", "github.com/acme/haven"),
            ("https://tok@github.com/acme/haven", "github.com/acme/haven"),
            (
                "ssh://git@gitlab.com/acme/haven.git",
                "gitlab.com/acme/haven",
            ),
            ("", ""),
        ] {
            assert_eq!(clean_remote(i), w, "{i}");
        }
    }

    #[test]
    fn day_and_commit() {
        let t = Time::parse("2026-10-25T23:30:00+01:00").unwrap();
        assert_eq!(day(t), "25 Oct 2026");
        assert_eq!(short_commit("1a2b3c4d5e"), "1a2b3c4");
        assert_eq!(short_commit("1a2"), "1a2");
    }

    #[test]
    fn sources_stay_in_the_repository() {
        let s = sources(
            "/r",
            "/r/.rush/plugins/demo",
            &["../../../lib".into(), "../../../../x".into()],
        );
        assert_eq!(s, vec![".rush/plugins/demo", "lib"]);
    }
}
