//! pushlater holds an agent's git push until its branch has been quiet, so
//! a burst of pushes reaches the remote, and its CI, as one.
//!
//! The gate's hook rewrites a plain trailing `git push` in a session to
//! `rush push-later`, which writes a hold and starts a waiter. The waiter
//! sends the newest hold for the branch once QUIET passes with no newer
//! one, and tells the session when the push fails. Pushes to main, master
//! or the remote's default branch are never held, and RUSH_PUSH_NOW=1
//! turns it off.
//!
//! Ported from internal/pushlater/pushlater.go. Holds live in
//! `<rush's folder>/push-later/<key>.json`, written and read alike by Go
//! and Rust rush.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rush_core::time::Ms;
use serde::{Deserialize, Serialize};

use crate::rfc3339::{de_time_strict, null_default, ser_time};

/// QUIET is how long a branch goes without a new push before its held push
/// is sent.
pub const QUIET: Duration = Duration::from_secs(2 * 60);

/// QUIET as Go prints a duration, for the words `rush push-later` says.
pub const QUIET_WORDS: &str = "2m0s";

/// ERR_NONE is a --now with nothing held for the branch.
pub const ERR_NONE: &str = "no push is held for this branch";

/// off is whether pushes go at once: RUSH_PUSH_NOW is set, or this isn't a
/// rush session.
pub fn off() -> bool {
    let set = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    set("RUSH_PUSH_NOW") || !set("RUSH_SESSION")
}

/// Go's `\s`: ASCII space, tab, newline, form feed, carriage return.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r')
}

/// What a pushed word may be made of: not a space, nor a shell character
/// that would make the command more than a plain push.
fn is_word(c: char) -> bool {
    !is_space(c) && !matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')' | '$' | '`')
}

/// trailing_push finds a `git push` that ends the command: alone, or after
/// && or ; (or a new line) with nothing after it. It is Go's
/// `(^|&&|;|\n)(\s*)git\s+push((?:\s+[^\s;&|<>()$`]+)*)\s*$`, by hand:
/// where the `git` starts, and where the push's arguments are.
fn trailing_push(cmd: &str) -> Option<(usize, std::ops::Range<usize>)> {
    let b = cmd.as_bytes();
    for s in 0..=b.len() {
        // The alternatives in the regexp's order: ^, &&, ;, \n.
        let mut anchors = Vec::with_capacity(2);
        if s == 0 {
            anchors.push(0);
        }
        if b[s..].starts_with(b"&&") {
            anchors.push(2);
        } else if b[s..].starts_with(b";") || b[s..].starts_with(b"\n") {
            anchors.push(1);
        }
        for a in anchors {
            if let Some(m) = push_at(cmd, s + a) {
                return Some(m);
            }
        }
    }
    None
}

fn push_at(cmd: &str, from: usize) -> Option<(usize, std::ops::Range<usize>)> {
    let skip = |i: usize| i + cmd[i..].len() - cmd[i..].trim_start_matches(is_space).len();
    let git = skip(from);
    let rest = cmd[git..].strip_prefix("git")?;
    let after_git = cmd.len() - rest.len();
    let push = skip(after_git);
    if push == after_git {
        return None; // git\s+push wants a space between
    }
    let args = cmd[push..].strip_prefix("push")?;
    let start = cmd.len() - args.len();
    if !args.chars().all(|c| is_space(c) || is_word(c)) {
        return None;
    }
    if args.chars().next().is_some_and(|c| !is_space(c)) {
        return None; // `git pushx` isn't a push
    }
    let end = start + args.trim_end_matches(is_space).len();
    Some((git, start..end))
}

/// safe_flags are the flags a held push may carry; any other leaves the
/// command alone, as do refspecs with a colon or naming a protected branch.
const SAFE_FLAGS: &[&str] = &[
    "-u",
    "--set-upstream",
    "-q",
    "--quiet",
    "--no-verify",
    "-v",
    "--verbose",
];

/// rewrite is command with its trailing `git push ...` run as
/// `<exe> push-later ...`, when it is one a hold may stand in for.
pub fn rewrite(command: &str, exe: &str) -> Option<String> {
    let (git, args_at) = trailing_push(command)?;
    let tail = &command[args_at.clone()];
    let args: Vec<&str> = tail.split_whitespace().collect();
    let flags = args.iter().filter(|a| a.starts_with('-')).count();
    if args.len() > 2 + flags {
        return None; // more than a remote and a branch
    }
    for a in &args {
        if a.starts_with('-') && !SAFE_FLAGS.contains(a) {
            return None;
        }
        if a.contains(':') || protected(a) {
            return None;
        }
    }
    Some(format!(
        "{}{} push-later{}",
        &command[..git],
        quote(exe),
        tail
    ))
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// protected is whether branch is one a push is never held for.
pub fn protected(branch: &str) -> bool {
    branch == "main" || branch == "master"
}

/// A Hold is a push waiting for its branch to go quiet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    #[serde(default, deserialize_with = "null_default")]
    pub dir: String,
    #[serde(default, deserialize_with = "null_default")]
    pub branch: String,
    #[serde(default, deserialize_with = "null_default")]
    pub args: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub session: String,
    #[serde(
        default,
        serialize_with = "ser_time",
        deserialize_with = "de_time_strict"
    )]
    pub at: Ms,
}

/// key names the hold for dir's branch: the first 8 bytes of
/// sha256(dir NUL branch), in hex.
pub fn key(dir: &str, branch: &str) -> String {
    let mut msg = Vec::with_capacity(dir.len() + branch.len() + 1);
    msg.extend_from_slice(dir.as_bytes());
    msg.push(0);
    msg.extend_from_slice(branch.as_bytes());
    sha256(&msg)[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// how long a hold made at `at` still waits, at `now`.
pub fn due(at: Ms, now: Ms) -> Duration {
    let left = at
        .saturating_add(QUIET.as_millis() as Ms)
        .saturating_sub(now);
    Duration::from_millis(left.max(0) as u64)
}

/// rush's folder, as Go's state.Dir (internal/state/state.go:27) finds it:
/// RUSH_HOME, else ~/.config/rush, or ~/.config/agtop when only that one
/// has a config.json.
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RUSH_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let rush = home.join(".config").join("rush");
    let agtop = home.join(".config").join("agtop");
    if !rush.join("config.json").exists() && agtop.join("config.json").exists() {
        return agtop;
    }
    rush
}

/// Holds is the folder holds are kept in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holds {
    dir: PathBuf,
}

impl Default for Holds {
    /// `push-later` in rush's folder.
    fn default() -> Self {
        Holds::at(state_dir().join("push-later"))
    }
}

impl Holds {
    /// Holds kept in dir.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Holds { dir: dir.into() }
    }

    /// the folder itself; the waiter's log goes here too (`<key>.log`).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    fn pid_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.pid"))
    }

    /// write records h, replacing any older hold for its branch, and gives
    /// its key.
    pub fn write(&self, h: &Hold) -> io::Result<String> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let key = key(&h.dir, &h.branch);
        let b = serde_json::to_vec(h)?;
        let path = self.path(&key);
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &b)?;
        fs::rename(&tmp, &path)?;
        Ok(key)
    }

    /// read is the hold under key.
    pub fn read(&self, key: &str) -> io::Result<Hold> {
        let b = fs::read(self.path(key))?;
        Ok(serde_json::from_slice(&b)?)
    }

    /// take removes and returns the hold under key, for a push sent at
    /// once.
    pub fn take(&self, key: &str) -> io::Result<Hold> {
        let h = self.read(key)?;
        fs::remove_file(self.path(key))?;
        Ok(h)
    }

    /// lock claims the waiter for key: false when a live one has it.
    pub fn lock(&self, key: &str, alive: impl Fn(i32) -> bool) -> bool {
        let p = self.pid_path(key);
        let me = std::process::id() as i32;
        if let Ok(b) = fs::read(&p)
            && let Ok(pid) = serde_json::from_slice::<i32>(&b)
            && pid != me
            && alive(pid)
        {
            return false;
        }
        write_private(&p, me.to_string().as_bytes()).is_ok()
    }

    /// unlock frees key's waiter lock, if this process holds it.
    pub fn unlock(&self, key: &str) {
        let p = self.pid_path(key);
        if let Ok(b) = fs::read(&p)
            && serde_json::from_slice::<i32>(&b).ok() == Some(std::process::id() as i32)
        {
            let _ = fs::remove_file(p);
        }
    }
}

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

fn write_private(path: &Path, b: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(b)
}

/// PushFn sends a held push; Err is why it failed.
pub type PushFn<'a> = Box<dyn FnMut(&Hold) -> Result<(), String> + 'a>;

/// FailFn is told a held push that failed, and why.
pub type FailFn<'a> = Box<dyn FnMut(&Hold, &str) + 'a>;

/// A Waiter sends a held push once its branch is quiet. Its clock, sleep
/// and push are fields so tests can drive it.
pub struct Waiter<'a> {
    pub holds: Holds,
    pub now: Box<dyn FnMut() -> Ms + 'a>,
    pub sleep: Box<dyn FnMut(Duration) + 'a>,
    pub push: PushFn<'a>,
    pub fail: Option<FailFn<'a>>,
}

impl Waiter<'_> {
    /// run waits on the hold under key until it is due, sends it, and keeps
    /// on while a newer hold came during the push. It returns once none is
    /// left.
    pub fn run(&mut self, key: &str) {
        loop {
            let Ok(h) = self.holds.read(key) else {
                return;
            };
            let d = due(h.at, (self.now)());
            if !d.is_zero() {
                (self.sleep)(d);
                continue;
            }
            let res = (self.push)(&h);
            if let Ok(now) = self.holds.read(key)
                && now.at != h.at
            {
                continue; // a newer push came while this one went: it carries this one
            }
            let _ = fs::remove_file(self.holds.path(key));
            if let (Err(err), Some(fail)) = (res, self.fail.as_mut()) {
                fail(&h, &err);
            }
            return;
        }
    }
}

/// sha256 of msg (FIPS 180-4), for key: small enough not to want a crate.
fn sha256(msg: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut data = msg.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&((msg.len() as u64) * 8).to_be_bytes());
    for block in data.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, c) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*c);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn rewrite_only_a_plain_trailing_push() {
        for (cmd, want) in [
            ("git push", "'rush' push-later"),
            (
                "git push -u origin feat/x",
                "'rush' push-later -u origin feat/x",
            ),
            (
                "git add a && git commit -m x && git push",
                "git add a && git commit -m x && 'rush' push-later",
            ),
            (
                "cd repo; git push origin HEAD",
                "cd repo; 'rush' push-later origin HEAD",
            ),
            ("git push   ", "'rush' push-later"),
            ("a &&  git  push -q  ", "a &&  'rush' push-later -q"),
            ("git push --force", ""),
            ("git push --force-with-lease", ""),
            ("git push origin --delete feat/x", ""),
            ("git push --tags", ""),
            ("git push origin main", ""),
            ("git push origin HEAD:main", ""),
            ("git push origin a b c", ""),
            ("git push && gh pr create", ""),
            ("git push | tail -2", ""),
            ("git push 2>&1", ""),
            ("echo git push", ""),
            ("&&&& git push", "&&&& 'rush' push-later"),
            (" ; git push", " ; 'rush' push-later"),
            ("git pushx", ""),
            ("gitpush", ""),
        ] {
            let got = rewrite(cmd, "rush");
            assert_eq!(
                got.as_deref().unwrap_or(""),
                want,
                "rewrite({cmd:?}) = {got:?}"
            );
        }
        assert_eq!(
            rewrite("git push", "/a b/it's").unwrap(),
            r"'/a b/it'\''s' push-later"
        );
    }

    #[test]
    fn due_counts_down_the_quiet() {
        let at = 1_792_756_800_000; // 2026-10-23T12:00:00Z
        assert_eq!(due(at, at + 30_000), QUIET - Duration::from_secs(30));
        assert_eq!(due(at, at + QUIET.as_millis() as Ms + 1000), Duration::ZERO);
        assert_eq!(due(Ms::MAX, Ms::MIN), Duration::from_millis(Ms::MAX as u64));
        assert_eq!(due(Ms::MIN, Ms::MAX), Duration::ZERO);
    }

    // The same key as Go's hex(sha256(dir NUL branch)[:8]).
    #[test]
    fn key_is_gos() {
        assert_eq!(
            sha256(b"abc")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // printf '/r\0feat/x' | shasum -a 256 | cut -c1-16
        assert_eq!(key("/r", "feat/x"), "51c9fe823b03267f");
    }

    // A hold Go wrote reads here, and one written here reads as Go writes
    // it.
    #[test]
    fn holds_read_gos_files() {
        let tmp = TempDir::new();
        let holds = Holds::at(tmp.path());
        let k = key("/r", "feat/x");
        fs::create_dir_all(tmp.path()).unwrap();
        fs::write(
            tmp.path().join(format!("{k}.json")),
            r#"{"dir":"/r","branch":"feat/x","args":null,"session":"","at":"2026-10-23T14:00:00.123456789+02:00"}"#,
        )
        .unwrap();
        let h = holds.read(&k).unwrap();
        assert_eq!(h.at, 1_792_756_800_123);
        assert!(h.args.is_empty());
        let k2 = holds.write(&h).unwrap();
        assert_eq!(k2, k);
        assert_eq!(
            fs::read_to_string(tmp.path().join(format!("{k}.json"))).unwrap(),
            r#"{"dir":"/r","branch":"feat/x","args":[],"session":"","at":"2026-10-23T12:00:00.123Z"}"#
        );
        assert_eq!(holds.take(&k).unwrap(), h);
        // One whose time can't be read isn't a hold, as in Go: the waiter
        // leaves it rather than pushing at once.
        fs::write(
            tmp.path().join(format!("{k}.json")),
            r#"{"dir":"/r","branch":"feat/x","at":"soon"}"#,
        )
        .unwrap();
        assert!(holds.read(&k).is_err(), "read a hold with no time");
        fs::remove_file(tmp.path().join(format!("{k}.json"))).unwrap();
        assert!(holds.read(&k).is_err());
    }

    #[test]
    fn lock_keeps_out_a_second_live_waiter() {
        let tmp = TempDir::new();
        let holds = Holds::at(tmp.path());
        fs::create_dir_all(tmp.path()).unwrap();
        assert!(holds.lock("k", |_| true));
        assert!(holds.lock("k", |_| true), "this process holds it already");
        fs::write(tmp.path().join("k.pid"), "1").unwrap();
        assert!(!holds.lock("k", |_| true), "a live one has it");
        assert!(holds.lock("k", |_| false), "a dead one's is taken");
        holds.unlock("k");
        assert!(!tmp.path().join("k.pid").exists());
    }

    // A burst of holds sends one push, the newest, once the branch is
    // quiet; a failure is told once.
    #[test]
    fn waiter_sends_the_newest_once_quiet() {
        let tmp = TempDir::new();
        let holds = Holds::at(tmp.path());
        let clock = Cell::new(1_792_756_800_000);
        let hold = |args: &str, at| Hold {
            dir: "/r".into(),
            branch: "feat/x".into(),
            args: vec![args.into()],
            at,
            ..Default::default()
        };
        let key = holds.write(&hold("first", clock.get())).unwrap();
        let pushed = RefCell::new(Vec::new());
        let slept = RefCell::new(Vec::new());
        let failed = RefCell::new(Vec::new());
        let held = Cell::new(false);
        let quiet = QUIET.as_millis() as Ms;
        let mut w = Waiter {
            holds: holds.clone(),
            now: Box::new(|| clock.get()),
            sleep: Box::new(|d| {
                slept.borrow_mut().push(d);
                clock.set(clock.get() + d.as_millis() as Ms);
                if !held.get() {
                    // a second push lands a minute into the wait
                    held.set(true);
                    holds
                        .write(&hold("second", clock.get() - quiet + 60_000))
                        .unwrap();
                }
            }),
            push: Box::new(|h| {
                pushed.borrow_mut().push(h.args.clone());
                Err("rejected".into())
            }),
            fail: Some(Box::new(|_, err| failed.borrow_mut().push(err.to_string()))),
        };
        w.run(&key);
        drop(w);
        let pushed = pushed.into_inner();
        assert!(
            pushed.len() == 1 && pushed[0][0] == "second",
            "pushed {pushed:?}, want only the newest"
        );
        assert_eq!(slept.borrow().len(), 2, "slept {:?}", slept.borrow());
        assert_eq!(failed.into_inner(), ["rejected"]);
        assert!(holds.read(&key).is_err(), "the hold outlived its push");
    }

    /// A folder of its own under the system's temp folder, gone after.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "rush-gh-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&p);
            TempDir(p.join("push-later"))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = fs::remove_dir_all(parent);
            }
        }
    }
}
