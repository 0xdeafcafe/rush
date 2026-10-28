//! Finds out which of the agents' programs rush runs have a newer version
//! out, and updates them.
//!
//! It runs each program's --version, asks Homebrew or the package's
//! registry for the newest, and remembers the newest for [`EVERY`], as
//! update does for rush itself. Every function here waits on programs or
//! the network: never call one on the UI thread.
//!
//! Ported from internal/harnessup/harnessup.go:1-214. Go asks the agent
//! registry which harnesses are installed and where; here the caller says,
//! with a [`Harness`] each (the registry is the adapters' module).

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::json;

mod latest;
mod plan;
mod version;

pub use latest::latest_of;
pub use plan::{Published, Source, Update, plan_for, source_of};
pub use version::{newer, parse};

use crate::events;

/// How long a newest version is kept before it's asked for again.
pub const EVERY: Duration = Duration::from_secs(4 * 3600);

/// What looking for newer harnesses is called where rush shows what waits
/// on the network.
pub const JOB: &str = "Harness update check";

/// How a harness stands against its newest version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Either version couldn't be found.
    Unknown,
    UpToDate,
    Available,
}

/// One harness on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub kind: String,
    pub name: String,
    /// The real program, not rush's stand-in for it.
    pub path: PathBuf,
    pub installed: String,
    pub latest: String,
    /// Empty argv: no way known to update it.
    pub update: Update,
}

impl Status {
    /// Whether a newer version is out.
    pub fn state(&self) -> State {
        if self.installed.is_empty() || self.latest.is_empty() {
            State::Unknown
        } else if newer(&self.latest, &self.installed) {
            State::Available
        } else {
            State::UpToDate
        }
    }
}

/// An installed harness to check: its agent kind and name, its real
/// program (see [`real_program`]), and where it says it's published.
#[derive(Clone, Debug, Default)]
pub struct Harness {
    pub kind: String,
    pub name: String,
    pub path: PathBuf,
    pub published: Published,
}

/// One kept newest version, as harnessup.json has it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Entry {
    #[serde(default)]
    latest: String,
    #[serde(default, with = "go_time")]
    at: Option<SystemTime>,
}

type Cache = BTreeMap<String, Entry>;

/// Go's time.Time in JSON: RFC 3339; its zero value when unset.
mod go_time {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::SystemTime;

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&crate::timefmt::rfc3339_nano(*t)),
            None => s.serialize_str("0001-01-01T00:00:00Z"),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        let s = String::deserialize(d)?;
        Ok(crate::timefmt::parse(&s))
    }
}

/// Where the newest versions are kept: harnessup.json in rush's state
/// folder.
pub fn cache_path(state_dir: &Path) -> PathBuf {
    state_dir.join("harnessup.json")
}

/// Every given harness's status. The newest versions are asked for only
/// when none was kept in the last [`EVERY`] (or force). find is how rush
/// finds programs (brew, for one).
pub fn check(
    state_dir: &Path,
    harnesses: &[Harness],
    force: bool,
    find: &(dyn Fn(&str) -> Option<PathBuf> + Sync),
) -> Vec<Status> {
    let c: Cache = std::fs::read(cache_path(state_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let rows: Vec<(&Harness, Status)> = harnesses
        .iter()
        .filter(|h| !h.path.as_os_str().is_empty() && seen.insert(h.path.clone()))
        .map(|h| {
            (
                h,
                Status {
                    kind: h.kind.clone(),
                    name: h.name.clone(),
                    path: h.path.clone(),
                    ..Status::default()
                },
            )
        })
        .collect();
    let c = Mutex::new(c);
    let online = Mutex::new(true);
    let out: Vec<Status> = std::thread::scope(|sc| {
        let handles: Vec<_> = rows
            .into_iter()
            .map(|(h, mut r)| {
                let (c, online) = (&c, &online);
                sc.spawn(move || {
                    let real = std::fs::canonicalize(&r.path).unwrap_or_default();
                    let src = source_of(&real.to_string_lossy());
                    r.update = plan_for(&h.published, &r.path.to_string_lossy(), &src);
                    r.installed = version_of(&r.path);
                    let fresh = {
                        let c = c.lock().unwrap();
                        let e = c.get(&r.kind);
                        !force
                            && e.is_some_and(|e| {
                                !e.latest.is_empty()
                                    // One kept "in the future" (the clock went back)
                                    // is fresh, as Go's time.Since is negative.
                                    && e.at.is_some_and(|at| at.elapsed().map_or(true, |d| d < EVERY))
                            })
                    };
                    if !fresh {
                        if !events::net_run(JOB) {
                            *online.lock().unwrap() = false;
                        } else {
                            let l = latest_of(&h.published, &src, find);
                            events::net_done(JOB);
                            events::log(
                                "harness-checked",
                                json!({"harness": r.name, "installed": r.installed, "latest": l, "forced": force}),
                            );
                            if !l.is_empty() {
                                c.lock().unwrap().insert(
                                    r.kind.clone(),
                                    Entry {
                                        latest: l,
                                        at: Some(SystemTime::now()),
                                    },
                                );
                            }
                        }
                    }
                    r.latest = c.lock().unwrap().get(&r.kind).map(|e| e.latest.clone()).unwrap_or_default();
                    r
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    if *online.lock().unwrap()
        && let Ok(b) = serde_json::to_vec(&*c.lock().unwrap())
    {
        let _ = make_private_dir(state_dir);
        let _ = write_private(&cache_path(state_dir), &b);
    }
    out
}

fn make_private_dir(d: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(d)
}

fn write_private(p: &Path, b: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(p)?
        .write_all(b)
}

/// Where an agent's program is, past rush's stand-ins: found is where it
/// was found on PATH; when that's rush's shim folder, the program of that
/// name on path_without_shims is the real one. None when there's none.
pub fn real_program(
    found: &Path,
    program: &str,
    shim_dir: &Path,
    path_without_shims: &OsStr,
) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if found.as_os_str().is_empty() {
        return None;
    }
    let clean = |p: &Path| -> PathBuf { p.components().collect() };
    if found.parent().map(clean) != Some(clean(shim_dir)) {
        return Some(found.to_path_buf());
    }
    std::env::split_paths(path_without_shims).find_map(|d| {
        let c = d.join(program);
        let m = std::fs::metadata(&c).ok()?;
        (!m.is_dir() && m.permissions().mode() & 0o111 != 0).then_some(c)
    })
}

/// What program path says its version is, or empty.
pub fn version_of(path: &Path) -> String {
    match crate::cmd::output_within(
        Command::new(path)
            .arg("--version")
            .env("NO_COLOR", "1")
            .env("CI", "1"),
        Duration::from_secs(8),
    ) {
        Ok(o) => parse(&String::from_utf8_lossy(&o.out)),
        Err(_) => String::new(),
    }
}

/// Updates the harness s describes, and is how it went: an error carries
/// the last of what the updater said. find is how rush finds programs, for
/// an updater named without its path.
pub fn run(s: &Status, find: &dyn Fn(&str) -> Option<PathBuf>) -> Result<(), String> {
    events::log(
        "harness-update",
        json!({"harness": s.name, "from": s.installed, "to": s.latest, "how": s.update.label}),
    );
    let res = run_update(s, find);
    let mut f = json!({"harness": s.name});
    if let Err(e) = &res {
        f["err"] = json!(e);
    }
    events::log("harness-updated", f);
    res
}

fn run_update(s: &Status, find: &dyn Fn(&str) -> Option<PathBuf>) -> Result<(), String> {
    if s.update.argv.is_empty() {
        return Err(format!(
            "rush doesn't know how {} was installed: update it the way you installed it",
            s.name
        ));
    }
    let mut argv = s.update.argv.clone();
    if !Path::new(&argv[0]).is_absolute() {
        let Some(p) = find(&argv[0]) else {
            return Err(format!("{} isn't on your PATH", argv[0]));
        };
        argv[0] = p.to_string_lossy().into_owned();
    }
    let res = crate::cmd::combined_within(
        Command::new(&argv[0]).args(&argv[1..]),
        Duration::from_secs(600),
    );
    let (out, err) = match res {
        Ok(o) if o.ok() => return Ok(()),
        Ok(o) => {
            let e = o.err_text();
            (o.out, e)
        }
        Err(e) => (Vec::new(), e.to_string()),
    };
    let text = String::from_utf8_lossy(&out);
    let tail = text.trim();
    let tail = tail.rsplit_once('\n').map_or(tail, |(_, t)| t);
    Err(format!(
        "{}: {}",
        s.update.label,
        if tail.is_empty() { &err } else { tail }
    ))
}

#[cfg(test)]
mod tests;
