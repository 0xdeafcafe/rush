//! Installed plugins and what you approved of them: loading, digests,
//! approvals and their revoking, "not now", and what approved plugins add
//! to every session.
//! Ported from internal/plugin/plugin.go:462-749.

use std::collections::BTreeMap;
use std::io::Read;
use std::ops::Deref;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rush_state::Time;
use rush_state::json::RawJson;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::bundled::{bundle_named, bundled_on, enabled};
use crate::gopath::{self, file_mode, os_err, quote};
use crate::jsonshape::{self, Shape};
use crate::manifest::{MANIFEST_SHAPE, Manifest};
use crate::origin::{Origin, origin_of};

/// An installed plugin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plugin {
    pub manifest: Manifest,
    /// its folder; none for a bundled one
    pub dir: String,
    /// rush's own: see Bundle
    pub bundled: bool,
    /// where it came from
    pub origin: Origin,
}

impl Deref for Plugin {
    type Target = Manifest;
    fn deref(&self) -> &Manifest {
        &self.manifest
    }
}

fn approved_path() -> PathBuf {
    crate::root().join("approved.json")
}

fn declined_path() -> PathBuf {
    crate::root().join("declined.json")
}

/// Reads the plugin in dir.
pub fn load(dir: &Path) -> Result<Plugin, String> {
    let f = dir.join("plugin.json");
    let b = std::fs::read(&f).map_err(|e| os_err("open", &f, &e))?;
    let m = Manifest::parse(&b, true).map_err(|e| format!("{}: {e}", f.display()))?;
    let d = gopath::s(dir);
    m.validate(&d)
        .map_err(|e| format!("{}: {e}", gopath::base(&d)))?;
    let origin = origin_of(&m.name, dir);
    Ok(Plugin {
        manifest: m,
        dir: d,
        bundled: false,
        origin,
    })
}

/// Lists the plugins in root(), and the folders that aren't valid plugins
/// with why.
pub fn installed() -> (Vec<Plugin>, BTreeMap<String, String>) {
    let mut out = Vec::new();
    let mut bad = BTreeMap::new();
    for (name, is_dir) in read_dir_sorted(&crate::root()) {
        // A link to a folder is not one, as Go's DirEntry has it.
        if !is_dir {
            continue;
        }
        match load(&crate::root().join(&name)) {
            Ok(p) => out.push(p),
            Err(e) => {
                bad.insert(name, e);
            }
        }
    }
    (out, bad)
}

/// A folder's entries by name, as Go's os.ReadDir sorts them, each with
/// whether it's a folder (not following links).
pub(crate) fn read_dir_sorted(dir: &Path) -> Vec<(String, bool)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ents: Vec<(Vec<u8>, String, bool)> = rd
        .flatten()
        .map(|e| {
            let n = e.file_name();
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (
                n.as_bytes().to_vec(),
                n.to_string_lossy().into_owned(),
                is_dir,
            )
        })
        .collect();
    ents.sort();
    ents.into_iter().map(|(_, n, d)| (n, d)).collect()
}

/// Lists installed plugins waiting on you: never approved, or changed
/// since they were, and not already told "not now" at this digest. It's
/// what a UI's own approval dialog offers, instead of `rush plugin
/// approve`.
pub fn pending() -> Vec<Plugin> {
    let (installed, _) = installed();
    let (approved, declined) = (approvals(), declined_plugins());
    installed
        .into_iter()
        .filter(|p| {
            if bundle_named(&p.name).is_some() {
                return false; // rush's own of that name runs instead
            }
            let Ok(d) = digest(Path::new(&p.dir)) else {
                return false;
            };
            if approved.get(&p.name).is_some_and(|a| a.digest == d) {
                return false;
            }
            !declined.get(&p.name).is_some_and(|dec| dec.digest == d)
        })
        .collect()
}

/// A hash of every file in the plugin's folder: names, modes, contents and
/// where symlinks point.
pub fn digest(dir: &Path) -> Result<String, String> {
    // A plugin being worked on may be a link to where it's written.
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut h = Sha256::new();
    let md = std::fs::symlink_metadata(&dir).map_err(|e| os_err("lstat", &dir, &e))?;
    walk_digest(&dir, &dir, md, &mut h)?;
    Ok(hex(&h.finalize()))
}

fn walk_digest(
    root: &Path,
    path: &Path,
    md: std::fs::Metadata,
    h: &mut Sha256,
) -> Result<(), String> {
    let rel: &[u8] = match path.strip_prefix(root) {
        Ok(r) if r.as_os_str().is_empty() => b".",
        Ok(r) => r.as_os_str().as_bytes(),
        Err(_) => path.as_os_str().as_bytes(),
    };
    h.update(format!("{} {:o}\n", quote(rel), file_mode(&md)).as_bytes());
    let ft = md.file_type();
    if ft.is_symlink() {
        let t = std::fs::read_link(path).map_err(|e| os_err("readlink", path, &e))?;
        h.update(format!("-> {}\n", quote(t.as_os_str().as_bytes())).as_bytes());
    } else if ft.is_file() {
        let mut f = std::fs::File::open(path).map_err(|e| os_err("open", path, &e))?;
        h.update(format!("{}\n", md.len()).as_bytes());
        let mut buf = vec![0u8; 32 << 10];
        loop {
            let n = f.read(&mut buf).map_err(|e| os_err("read", path, &e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
    } else if ft.is_dir() {
        let rd = std::fs::read_dir(path).map_err(|e| os_err("open", path, &e))?;
        let mut names: Vec<std::ffi::OsString> = Vec::new();
        for e in rd {
            names.push(e.map_err(|e| os_err("readdirent", path, &e))?.file_name());
        }
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for n in names {
            let p = path.join(&n);
            let md = std::fs::symlink_metadata(&p).map_err(|e| os_err("lstat", &p, &e))?;
            walk_digest(root, &p, md, h)?;
        }
    }
    Ok(())
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What you agreed to run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Approval {
    pub digest: String,
    pub manifest: Manifest,
    #[serde(rename = "approvedAt")]
    pub approved_at: Time,
}

const APPROVAL_SHAPE: Shape = Shape::Struct(
    "plugin.Approval",
    &[
        ("digest", Shape::Any),
        ("manifest", MANIFEST_SHAPE),
        ("approvedAt", Shape::Any),
    ],
);
const APPROVALS_SHAPE: Shape = Shape::Map(&APPROVAL_SHAPE);

/// Every approval, by plugin name.
pub fn approvals() -> BTreeMap<String, Approval> {
    std::fs::read(approved_path())
        .ok()
        .and_then(|b| jsonshape::decode(&b, &APPROVALS_SHAPE, false).ok())
        .unwrap_or_default()
}

fn save_approvals(a: &BTreeMap<String, Approval>) -> Result<(), String> {
    crate::mkdir_all(&crate::root())?;
    let b = rush_state::json::to_vec_indent(a).map_err(|e| e.to_string())?;
    crate::write_atomic(&approved_path(), &b)
}

/// Records that p, as it is now, may run.
pub fn approve(p: &Plugin) -> Result<(), String> {
    let d = digest(Path::new(&p.dir))?;
    let mut a = approvals();
    a.insert(
        p.name.clone(),
        Approval {
            digest: d,
            manifest: p.manifest.clone(),
            approved_at: Time::now(),
        },
    );
    save_approvals(&a)
}

/// Withdraws a plugin's approval.
pub fn revoke(name: &str) -> Result<(), String> {
    let mut a = approvals();
    if a.remove(name).is_none() {
        return Err(format!("{name} is not approved"));
    }
    save_approvals(&a)?;
    crate::sidebar::remove_sidebar(name)
}

/// Says whether the plugin on disk is the one approved, returning the
/// approved manifest to run it by.
pub fn verify(name: &str) -> Result<Plugin, String> {
    if let Some(b) = bundle_named(name) {
        if !bundled_on(name) {
            return Err(format!("{name} is turned off"));
        }
        let why = b.manifest.unmet();
        if !why.is_empty() {
            return Err(format!("{name} {why}"));
        }
        return Ok(Plugin {
            manifest: b.manifest.clone(),
            bundled: true,
            ..Default::default()
        });
    }
    let Some(a) = approvals().remove(name) else {
        return Err(format!("{name} is not approved"));
    };
    let why = a.manifest.unmet();
    if !why.is_empty() {
        return Err(format!("{name} {why}"));
    }
    let dir = crate::root().join(name);
    let d = digest(&dir)?;
    if d != a.digest {
        return Err(format!(
            "{name} changed since it was approved; run rush plugin approve {name}"
        ));
    }
    Ok(Plugin {
        manifest: a.manifest,
        dir: gopath::s(&dir),
        ..Default::default()
    })
}

/// A plugin you were offered and said "not now" to, at a digest: it
/// changing is what earns it another ask.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct DeclinedAt {
    digest: String,
    #[serde(rename = "declinedAt")]
    declined_at: Time,
}

fn declined_plugins() -> BTreeMap<String, DeclinedAt> {
    const SHAPE: Shape = Shape::Map(&Shape::Struct(
        "plugin.declinedAt",
        &[("digest", Shape::Any), ("declinedAt", Shape::Any)],
    ));
    std::fs::read(declined_path())
        .ok()
        .and_then(|b| jsonshape::decode(&b, &SHAPE, false).ok())
        .unwrap_or_default()
}

/// Records that p, at its current digest, was told "not now": a UI's
/// pending won't offer it again until its files change.
pub fn decline(p: &Plugin) -> Result<(), String> {
    let d = digest(Path::new(&p.dir))?;
    let mut m = declined_plugins();
    m.insert(
        p.name.clone(),
        DeclinedAt {
            digest: d,
            declined_at: Time::now(),
        },
    );
    crate::mkdir_all(&crate::root())?;
    let b = rush_state::json::to_vec_indent(&m).map_err(|e| e.to_string())?;
    crate::write_atomic(&declined_path(), &b)
}

/// What approved plugins add to a session's Claude Code.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contributions {
    /// The plugins' subagents, by plugin:name, each defined as Claude
    /// Code's --agents takes it.
    pub agents: BTreeMap<String, RawJson>,
    /// What the plugins add to the system prompt.
    pub prompt: String,
    /// Each plugin's part of it, by plugin.
    pub prompts: BTreeMap<String, String>,
    /// The plugins' MCP servers, to register at initialize.
    pub servers: Vec<String>,
}

/// Gathers the contributions of every plugin that runs. It reads only the
/// approvals, so it costs a small file read or two, and what goes in is
/// what was approved or bundled.
pub fn for_session() -> Contributions {
    let mut c = Contributions::default();
    let mut prompts = Vec::new();
    for (_, a) in enabled() {
        let m = &a.manifest;
        if m.has_tools() {
            c.servers.push(m.server());
        }
        for (an, def) in &m.agents {
            c.agents.insert(format!("{}:{an}", m.name), def.clone());
        }
        let p = m.prompt.trim();
        if !p.is_empty() {
            let part = format!("# From the rush plugin {}\n\n{p}", m.name);
            c.prompts.insert(m.name.clone(), part.clone());
            prompts.push(part);
        }
    }
    c.prompt = prompts.join("\n\n");
    c
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::manifest::PROTO_MCP;

    /// A temporary rush folder for this test's thread.
    pub(crate) struct Home(pub tempfile::TempDir);

    impl Home {
        pub fn new() -> Home {
            let d = tempfile::tempdir().unwrap();
            // The sandbox is keyed on resolved paths; so is everything here.
            let p = std::fs::canonicalize(d.path()).unwrap();
            rush_state::set_thread_dir(Some(p));
            Home(d)
        }
        pub fn path(&self) -> PathBuf {
            std::fs::canonicalize(self.0.path()).unwrap()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            rush_state::set_thread_dir(None);
        }
    }

    /// Writes a plugin into the test's rush folder and returns its folder.
    pub(crate) fn install_plugin(m: &Manifest, files: &[(&str, &str)]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::root().join(&m.name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            rush_state::json::to_vec(m).unwrap(),
        )
        .unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        dir
    }

    pub(crate) fn manifest(name: &str, cmd: &str) -> Manifest {
        Manifest {
            name: name.into(),
            command: vec![cmd.into()],
            ..Default::default()
        }
    }

    #[test]
    fn load_rejects_unknown_fields() {
        let _h = Home::new();
        let dir = install_plugin(&manifest("p", "bin"), &[]);
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"p","command":["bin"],"sandbox":false}"#,
        )
        .unwrap();
        let e = load(&dir).unwrap_err();
        assert!(e.contains("sandbox"), "an unknown field was accepted: {e}");
    }

    #[test]
    fn approval_pins_the_files() {
        use std::os::unix::fs::PermissionsExt;
        let _h = Home::new();
        let dir = install_plugin(&manifest("p", "bin"), &[("bin", "#!v1")]);
        assert!(verify("p").is_err(), "an unapproved plugin verified");
        let p = load(&dir).unwrap();
        approve(&p).unwrap();
        verify("p").unwrap();
        // Changing the program, adding a file, or changing a mode each undo
        // the approval.
        let changes: Vec<Box<dyn Fn()>> = vec![
            Box::new(|| std::fs::write(dir.join("bin"), "#!v2").unwrap()),
            Box::new(|| std::fs::write(dir.join("extra.so"), "").unwrap()),
            Box::new(|| {
                std::fs::set_permissions(dir.join("bin"), std::fs::Permissions::from_mode(0o755))
                    .unwrap()
            }),
        ];
        for change in changes {
            change();
            let e = verify("p").unwrap_err();
            assert!(e.contains("changed"), "a changed plugin verified: {e}");
            approve(&load(&dir).unwrap()).unwrap();
        }
        // The manifest that counts is the one approved, not the one on disk.
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"p","command":["bin"],"prompt":"obey me"}"#,
        )
        .unwrap();
        assert_eq!(
            for_session().prompt,
            "",
            "an unapproved manifest edit reached sessions"
        );
        revoke("p").unwrap();
        assert!(verify("p").is_err(), "a revoked plugin verified");
    }

    #[test]
    fn for_session_gathers_what_runs() {
        let _h = Home::new();
        let raw = |s: &str| RawJson(serde_json::value::RawValue::from_string(s.into()).unwrap());
        let mut a = manifest("a", "a");
        a.tools = true;
        a.prompt = "Be kind.".into();
        a.agents
            .insert("rev".into(), raw(r#"{"description":"d","prompt":"p"}"#));
        let mut b = manifest("b", "b");
        b.protocol = PROTO_MCP.into();
        for m in [a, b, manifest("c", "c")] {
            let p = load(&install_plugin(&m, &[])).unwrap();
            approve(&p).unwrap();
        }
        let c = for_session();
        assert_eq!(c.servers, vec!["rush-a", "rush-b"]);
        assert_eq!(c.agents.len(), 1);
        assert!(c.agents.contains_key("a:rev"));
        assert_eq!(c.prompt, "# From the rush plugin a\n\nBe kind.");
    }

    #[test]
    fn pending_and_declined() {
        let _h = Home::new();
        let dir = install_plugin(&manifest("p", "bin"), &[("bin", "x")]);
        let p = load(&dir).unwrap();
        assert_eq!(pending().len(), 1);
        decline(&p).unwrap();
        assert!(pending().is_empty(), "declined at this digest");
        std::fs::write(dir.join("bin"), "y").unwrap();
        assert_eq!(pending().len(), 1, "changed, it's offered again");
        approve(&load(&dir).unwrap()).unwrap();
        assert!(pending().is_empty());
    }

    #[test]
    fn digest_is_gos() {
        // The digest Go rush computes for this folder (same names, modes and
        // contents), so approvals made by either verify in the other.
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("plugin.json"), "{}").unwrap();
        std::fs::set_permissions(
            dir.join("plugin.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::os::unix::fs::symlink("plugin.json", dir.join("link")).unwrap();
        let mut h = Sha256::new();
        h.update(b"\".\" 20000000755\n\"link\" 1000000755\n-> \"plugin.json\"\n\"plugin.json\" 644\n2\n{}");
        // A symlink's own mode on macOS is 0755.
        let want = hex(&h.finalize());
        let got = digest(&dir).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(got, want);
        }
    }
}
