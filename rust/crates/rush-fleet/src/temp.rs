//! An agent's temp work: where it is, how much disk it takes, emptying
//! it, and the sizes kept across restarts. Port of internal/fleet/temp.go,
//! diskusage.go, diskusage_darwin.go (getattrlistbulk) and
//! diskusage_background.go (paced walks, heavy folders reused).

use std::collections::HashMap;
use std::ffi::CString;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rush_core::adapter::{Registry, TempDir};
use rush_core::agent::Agent;
use rush_core::time::Ms;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::hosted::{is_temp_dir, temp_dir};

/// Where an agent's temp work is: what it cloned, built or downloaded as
/// scratch. A rush session has a folder rush gives it; the rest are its
/// agent's own (a background job's tmp folder, each session's scratch).
pub fn temp_dirs(reg: &Registry, a: &Agent) -> Vec<TempDir> {
    let mut out = Vec::new();
    let mut job = "";
    if a.rush {
        out.push(TempDir {
            path: temp_dir(&a.job.id),
            keep: true,
        });
    } else if !a.interactive && !a.past && !a.job.id.is_empty() {
        job = &a.job.id;
    }
    if let Some(ad) = reg.get(a.acct.kind.as_str())
        && let Some(s) = ad.scratcher()
    {
        out.extend(s.scratch(&a.acct, job, &a.job.session_id, &a.job.cwd));
    }
    out
}

/// The folder agent k's sessions' scratch folders are two below.
fn scratch_root(reg: &Registry, k: &str) -> String {
    reg.get(k)
        .and_then(|a| a.scratcher().map(|s| s.scratch_root()))
        .unwrap_or_default()
}

/// Whether d is one of rush's tmp folders or the agent's scratch: never
/// something a bad id could point elsewhere.
fn ours(reg: &Registry, a: &Agent, d: &str) -> bool {
    let root = scratch_root(reg, a.acct.kind.as_str());
    is_temp_dir(d)
        || !root.is_empty()
            && Path::new(d).parent().and_then(Path::parent) == Some(Path::new(&root))
}

/// Deletes an agent's temp work. It refuses while the agent has a process,
/// since whatever it's running may be using it.
pub fn clean_temp(reg: &Registry, a: &Agent) -> Result<(), String> {
    let r = (|| {
        if a.pid != 0 {
            return Err(format!(
                "{} is still running; stop it first",
                a.display_name
            ));
        }
        for d in temp_dirs(reg, a) {
            if !ours(reg, a, &d.path) {
                return Err(format!("won't delete {}", d.path));
            }
            if !d.keep {
                remove_all(&d.path)?;
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&d.path) else {
                continue;
            };
            for e in rd.flatten() {
                remove_all(&e.path().to_string_lossy())?;
            }
        }
        Ok(())
    })();
    agent_event(Some(a), "temp-cleaned", json!({"err": err_text(&r)}));
    r
}

/// Deletes one thing in one of a's temp folders, the rest kept. It refuses
/// while a runs, and anything not directly in one of them.
pub fn remove_temp_entry(reg: &Registry, a: &Agent, path: &str) -> Result<(), String> {
    let r = (|| {
        if a.pid != 0 {
            return Err(format!(
                "{} is still running; stop it first",
                a.display_name
            ));
        }
        let path = crate::worktree::clean(path);
        for d in temp_dirs(reg, a) {
            if !ours(reg, a, &d.path) {
                continue;
            }
            if Path::new(&path).parent() == Some(Path::new(&crate::worktree::clean(&d.path))) {
                return remove_all(&path);
            }
        }
        Err(format!(
            "won't delete {path}: it isn't in {}'s temp work",
            a.display_name
        ))
    })();
    agent_event(
        Some(a),
        "temp-removed",
        json!({"path": path, "err": err_text(&r)}),
    );
    r
}

/// Empties the tmp folder rush gave a session whose host is gone, once
/// nothing at its top has changed for idle. Only that folder. It says
/// whether it emptied it.
pub fn clean_stale_temp(a: &Agent, idle: Ms, now: Ms) -> Result<bool, String> {
    if !a.rush || a.pid != 0 || a.job.id.is_empty() {
        return Ok(false);
    }
    let dir = temp_dir(&a.job.id);
    if !is_temp_dir(&dir) {
        return Err(format!("won't delete {dir}"));
    }
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(false);
    };
    let ents: Vec<_> = rd.flatten().collect();
    if ents.is_empty() {
        return Ok(false);
    }
    for e in &ents {
        match e.metadata().ok().and_then(|m| m.modified().ok()) {
            Some(t) if now - ms_of(t) >= idle => {}
            _ => return Ok(false),
        }
    }
    let why = "its host is gone and it's idle";
    for e in &ents {
        if let Err(err) = remove_all(&e.path().to_string_lossy()) {
            agent_event(
                Some(a),
                "temp-cleaned",
                json!({"dir": dir, "why": why, "err": err}),
            );
            return Err(err);
        }
    }
    agent_event(
        Some(a),
        "temp-cleaned",
        json!({"dir": dir, "entries": ents.len(), "why": why}),
    );
    Ok(true)
}

pub(crate) fn ms_of(t: std::time::SystemTime) -> Ms {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as Ms,
        Err(e) => -(e.duration().as_millis() as Ms),
    }
}

fn remove_all(p: &str) -> Result<(), String> {
    let path = Path::new(p);
    let r = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => Err(e),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    };
    r.map_err(|e| e.to_string())
}

/// Logs something done to an agent's files or processes: with its
/// session's id when rush runs it. An empty err is left out.
pub(crate) fn agent_event(a: Option<&Agent>, kind: &str, mut f: serde_json::Value) {
    if f.get("err").is_some_and(|e| e == "") {
        f.as_object_mut().map(|m| m.remove("err"));
    }
    let Some(a) = a else {
        rush_sys::events::log(kind, f);
        return;
    };
    f["agent"] = json!(a.display_name);
    if a.rush && !a.job.id.is_empty() {
        // ponytail: the host module writes a session's own events.jsonl;
        // until it's wired in, its id goes with the event.
        f["session"] = json!(a.job.id);
        rush_sys::events::log(kind, f);
        return;
    }
    f["agentId"] = json!(a.job.id);
    rush_sys::events::log(kind, f);
}

pub(crate) fn err_text<T>(r: &Result<T, String>) -> String {
    r.as_ref().err().cloned().unwrap_or_default()
}

/// An agent's temp work as last measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TempSize {
    pub bytes: i64,
    #[serde(with = "rush_core::wire::time")]
    pub at: Ms,
    /// How long measuring it took, in ns as Go's duration: a folder of
    /// millions of files takes a core for most of a minute to walk.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub took: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// How much longer than a walk took it waits before the next: walks take
/// at most 1/TEMP_SHARE of a core.
const TEMP_SHARE: i64 = 50;

impl TempSize {
    /// How long a running agent's temp work goes before it's measured
    /// again: ten minutes, or longer for one whose last walk was long.
    fn every(&self) -> Ms {
        (self.took / 1_000_000 * TEMP_SHARE).clamp(10 * 60_000, 2 * 3_600_000)
    }
}

/// What each agent's temp work measured, across restarts, so the folders
/// are walked again only when an agent has done something since.
#[derive(Default)]
pub struct TempSizes {
    inner: Mutex<SizesIn>,
}

#[derive(Default)]
struct SizesIn {
    sizes: HashMap<String, TempSize>,
    dirty: bool,
    loaded: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct OnDisk {
    #[serde(default, rename = "sizes")]
    sizes: Option<HashMap<String, TempSize>>,
}

fn temp_path() -> std::path::PathBuf {
    rush_state::cache_path("temp.json")
}

impl TempSizes {
    /// None measured yet, not read from disk: load reads them.
    pub fn new() -> TempSizes {
        TempSizes::default()
    }

    /// What an agent's temp work measured last.
    pub fn bytes(&self, key: &str) -> i64 {
        self.lock().sizes.get(key).map_or(0, |s| s.bytes)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SizesIn> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the last measurements, once, under any set since.
    pub fn load(&self) {
        if self.lock().loaded {
            return;
        }
        let disk: OnDisk = std::fs::read(temp_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut t = self.lock();
        for (k, v) in disk.sizes.unwrap_or_default() {
            t.sizes.entry(k).or_insert(v);
        }
        t.loaded = true;
    }

    /// The agents whose temp work should be measured: never measured,
    /// busy since, or running and not measured for a while. A stopped one
    /// isn't walked again until it does something.
    pub fn due<'a>(&self, agents: &'a [Agent], now: Ms) -> Vec<&'a Agent> {
        let t = self.lock();
        agents
            .iter()
            .filter(|a| match t.sizes.get(&a.key) {
                None => true,
                Some(e) => {
                    a.pid != 0 && now - e.at > e.every() || a.job.updated_at > e.at && a.pid == 0
                }
            })
            .collect()
    }

    /// Records measurements; saved by the next save.
    pub fn set(&self, m: HashMap<String, TempSize>) {
        let mut t = self.lock();
        t.sizes.extend(m);
        t.dirty = true;
    }

    /// Writes the measurements if they changed.
    pub fn save(&self) {
        let mut t = self.lock();
        if !t.dirty || !t.loaded {
            return; // unread, it would write over what's there
        }
        let disk = OnDisk {
            sizes: Some(t.sizes.clone()),
        };
        if let Ok(b) = serde_json::to_vec(&disk) {
            let _ = rush_state::write_bytes(&temp_path(), &b);
        }
        t.dirty = false;
    }
}

// ---- How much disk a folder takes (diskusage*.go) ----

/// How much disk the folders take, counted as du does (allocated blocks),
/// without following links.
pub fn disk_usage(dirs: &[TempDir]) -> i64 {
    usage(dirs, &mut None)
}

/// Automatic disk meters are housekeeping, not foreground work: one
/// budget across temp and worktree scans, so concurrent meters can't
/// saturate a core. Two milliseconds of walking earns eighteen of rest.
static BACKGROUND: Mutex<()> = Mutex::new(());

pub(crate) struct Pacer {
    last: Instant,
    work: Duration,
}

impl Pacer {
    fn yield_now(p: &mut Option<Pacer>) {
        let Some(p) = p else { return };
        let now = Instant::now();
        p.work += now - p.last;
        let mut now = now;
        if p.work >= Duration::from_millis(2) {
            // An unusually slow call need not impose an unbounded sleep.
            std::thread::sleep((p.work * 9).min(Duration::from_millis(100)));
            p.work = Duration::ZERO;
            now = Instant::now();
        }
        p.last = now;
    }
}

/// disk_usage at a background pace, sharing one budget.
pub fn disk_usage_background(dirs: &[TempDir]) -> i64 {
    let _g = BACKGROUND.lock().unwrap_or_else(|e| e.into_inner());
    // Time between walks does not consume the work budget.
    let mut p = Some(Pacer {
        last: Instant::now(),
        work: Duration::ZERO,
    });
    let n = usage(dirs, &mut p);
    Pacer::yield_now(&mut p);
    n
}

fn usage(dirs: &[TempDir], pace: &mut Option<Pacer>) -> i64 {
    let mut n = 0;
    for d in dirs {
        Pacer::yield_now(pace);
        let Ok(m) = std::fs::symlink_metadata(&d.path) else {
            continue;
        };
        n += blocks(&m);
        if m.is_dir() {
            n += dir_usage(&d.path, pace);
        }
    }
    n
}

fn blocks(m: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    m.blocks() as i64 * 512
}

/// What's inside a folder, each entry looked at through the folder's own
/// descriptor (statUsage).
fn stat_usage(dir: &str, pace: &mut Option<Pacer>) -> i64 {
    Pacer::yield_now(pace);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        Pacer::yield_now(pace);
        let Ok(m) = e.metadata() else { continue }; // DirEntry::metadata doesn't follow links
        n += blocks(&m);
        if m.is_dir() {
            n += stat_usage(&format!("{dir}/{}", e.file_name().to_string_lossy()), pace);
        }
    }
    n
}

/// Folders that are huge, rarely change, and cost thousands of calls a
/// walk: a background walk reuses one's last total while its own modified
/// time holds and the total is under an hour old.
const HEAVY: &[&str] = &["node_modules", ".git", "target", ".venv", "venv", "Pods"];
const HEAVY_TTL: Duration = Duration::from_secs(3600);

/// A heavy folder's modified time, when its total was counted, and it.
type Heavy = HashMap<String, (i128, Instant, i64)>;

static HEAVY_CACHE: Mutex<Option<Heavy>> = Mutex::new(None);

fn mtime_nanos(dir: &str) -> Option<i128> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(dir).ok()?;
    Some(m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128)
}

/// Whether dir is a heavy folder whose last total still holds; the stamp
/// is its modified time, to store the new total under.
fn heavy_hit(dir: &str, pace: &Option<Pacer>) -> (i128, Option<i64>) {
    let name = dir.rsplit('/').next().unwrap_or(dir);
    if pace.is_none() || !HEAVY.contains(&name) {
        return (0, None);
    }
    let Some(stamp) = mtime_nanos(dir) else {
        return (0, None);
    };
    let c = HEAVY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(m, at, total)) = c.as_ref().and_then(|c| c.get(dir))
        && m == stamp
        && at.elapsed() < HEAVY_TTL
    {
        return (stamp, Some(total));
    }
    (stamp, None)
}

fn heavy_store(dir: &str, stamp: i128, total: i64) {
    if stamp == 0 {
        return;
    }
    let mut c = HEAVY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let c = c.get_or_insert_with(HashMap::new);
    if c.len() > 4096 {
        c.retain(|_, (_, at, _)| at.elapsed() < HEAVY_TTL);
    }
    c.insert(dir.to_string(), (stamp, Instant::now(), total));
}

/// What's inside a folder, read with getattrlistbulk where the file
/// system has it: each call hands back a folder's worth of entries' names,
/// types and allocated sizes.
fn dir_usage(dir: &str, pace: &mut Option<Pacer>) -> i64 {
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; 128 << 10];
        if let Some(n) = bulk::usage(libc::AT_FDCWD, dir, dir, &mut buf, pace) {
            return n;
        }
    }
    stat_usage(dir, pace) // a file system without it
}

#[cfg(target_os = "macos")]
mod bulk {
    use super::*;

    #[repr(C)]
    struct AttrList {
        bitmapcount: u16,
        reserved: u16,
        commonattr: u32,
        volattr: u32,
        dirattr: u32,
        fileattr: u32,
        forkattr: u32,
    }

    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const ATTR_CMN_NAME: u32 = 0x1;
    const ATTR_CMN_OBJTYPE: u32 = 0x8;
    const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
    const ATTR_DIR_ALLOCSIZE: u32 = 0x8;
    const ATTR_FILE_ALLOCSIZE: u32 = 0x4;
    const VDIR: u32 = 2;

    unsafe extern "C" {
        fn getattrlistbulk(
            dirfd: libc::c_int,
            attr_list: *mut AttrList,
            attr_buf: *mut libc::c_void,
            attr_buf_size: libc::size_t,
            options: u64,
        ) -> libc::c_int;
    }

    /// One folder and those under it; None when its file system can't
    /// answer getattrlistbulk. buf is reused all the way down.
    pub(super) fn usage(
        parent: libc::c_int,
        name: &str,
        dir: &str,
        buf: &mut [u8],
        pace: &mut Option<Pacer>,
    ) -> Option<i64> {
        Pacer::yield_now(pace);
        let (stamp, hit) = heavy_hit(dir, pace);
        if let Some(t) = hit {
            return Some(t);
        }
        let Ok(c) = CString::new(name) else {
            return Some(0);
        };
        // Opening by name from the parent's descriptor skips resolving the
        // whole path again.
        // SAFETY: a valid descriptor and NUL-terminated name.
        let fd = unsafe {
            libc::openat(
                parent,
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Some(0);
        }
        let mut attrs = AttrList {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE,
            volattr: 0,
            dirattr: ATTR_DIR_ALLOCSIZE,
            fileattr: ATTR_FILE_ALLOCSIZE,
            forkattr: 0,
        };
        let mut n = 0i64;
        let mut subs = Vec::new();
        loop {
            Pacer::yield_now(pace);
            // SAFETY: buf outlives the call and its length is given.
            let r =
                unsafe { getattrlistbulk(fd, &mut attrs, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if r < 0 {
                let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if e == libc::EINTR {
                    continue;
                }
                // SAFETY: fd is ours.
                unsafe { libc::close(fd) };
                if e == libc::ENOTSUP || e == libc::EINVAL {
                    return None;
                }
                return Some(n);
            }
            if r == 0 {
                break;
            }
            let mut p: &[u8] = buf;
            for _ in 0..r {
                let Some((size, sub, next)) = entry(p) else {
                    break;
                };
                n += size;
                if let Some(s) = sub {
                    subs.push(s);
                }
                p = next;
            }
        }
        for s in subs {
            n += usage(fd, &s, &format!("{dir}/{s}"), buf, pace).unwrap_or(0);
        }
        // SAFETY: fd is ours.
        unsafe { libc::close(fd) };
        heavy_store(dir, stamp, n);
        Some(n)
    }

    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    /// One entry getattrlistbulk packed: its allocated size, its name when
    /// it's a folder, and what follows it.
    pub(super) fn entry(p: &[u8]) -> Option<(i64, Option<String>, &[u8])> {
        if p.len() < 24 {
            return None;
        }
        let l = u32_at(p, 0) as usize;
        if l < 24 || l > p.len() {
            return None;
        }
        let (ent, next) = p.split_at(l);
        let common = u32_at(ent, 4);
        let dir_set = u32_at(ent, 12);
        let file_set = u32_at(ent, 16);
        let mut o = 24;
        let mut name: &[u8] = &[];
        if common & ATTR_CMN_NAME != 0 {
            if o + 8 > l {
                return Some((0, None, next));
            }
            let off = u32_at(ent, o) as i32 as isize;
            let nl = u32_at(ent, o + 4) as usize;
            let s = o as isize + off;
            if nl > 0 && s >= 0 && s as usize + nl <= l {
                name = &ent[s as usize..s as usize + nl - 1]; // less its NUL
            }
            o += 8;
        }
        let mut typ = 0;
        if common & ATTR_CMN_OBJTYPE != 0 && o + 4 <= l {
            typ = u32_at(ent, o);
            o += 4;
        }
        let mut size = 0i64;
        if dir_set & ATTR_DIR_ALLOCSIZE != 0 && o + 8 <= l {
            size = i64::from_le_bytes(ent[o..o + 8].try_into().unwrap());
            o += 8;
        }
        if file_set & ATTR_FILE_ALLOCSIZE != 0 && o + 8 <= l {
            size = i64::from_le_bytes(ent[o..o + 8].try_into().unwrap());
        }
        let sub =
            (typ == VDIR && !name.is_empty()).then(|| String::from_utf8_lossy(name).into_owned());
        Some((size, sub, next))
    }
}

#[cfg(test)]
pub(crate) fn stat_usage_for_test(dir: &str) -> i64 {
    stat_usage(dir, &mut None)
}
