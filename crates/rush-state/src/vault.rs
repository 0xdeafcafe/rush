//! The vault keeps the sign-ins of accounts not in use: in the login
//! keychain on macOS (through security(1), as photon/keychain does), in
//! files only you can read elsewhere. A provider's API key is kept there
//! too; the config says only which providers have one, so drawing never
//! reads the keychain.
//!
//! Ported from internal/state/vault.go:1-62 (Keys, Vault, Get, Put, Forget,
//! Lock), apikeys.go:1-39 (APIKey, PutAPIKey), logins.go:8-49 (LoginKeeper,
//! FoundLogin, Restored) and photon/keychain (Read, Write, Delete).

use crate::agents;
use crate::config::Login;
use std::fs;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// Where rush keeps sign-ins: the keychain on macOS, else files in Dir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub dir: PathBuf,
    /// The keychain service its items are under: on macOS, unless made
    /// over files.
    service: Option<String>,
}

/// The keychain service the normal rush home keeps its sign-ins under:
/// the one every rush used before each home had its own, so the normal
/// home's sign-ins carry on as they are.
pub const DEFAULT_SERVICE: &str = "rush-login";

/// The keychain service the rush home at home keeps its sign-ins under.
/// The normal home of the user whose home folder is user_home
/// (~/.config/rush, or ~/.config/agtop while that one is in use) keeps
/// [`DEFAULT_SERVICE`]; any other home (RUSH_HOME set elsewhere, a test
/// copy) has its own, `rush-login-` and 8 hex of a SHA-256 of its
/// canonical path, which starts empty. So a copy of the user's rush home
/// never reads, renews or hands out the user's real sign-ins. Not in Go,
/// where every home shared rush-login.
pub fn service_for(home: &Path, user_home: &Path) -> String {
    let home = canonical(home);
    let config = user_home.join(".config");
    if [config.join("rush"), config.join("agtop")]
        .iter()
        .any(|d| canonical(d) == home)
    {
        return DEFAULT_SERVICE.to_string();
    }
    let sum = Sha256::digest(home.as_os_str().as_encoded_bytes());
    let hex: String = sum[..4].iter().map(|b| format!("{b:02x}")).collect();
    format!("{DEFAULT_SERVICE}-{hex}")
}

/// p with its links resolved, or as it is when it isn't there yet.
fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// The home folder of the user this runs as, from the password file: the
/// keychain is that user's whatever HOME says, so a rush run with another
/// HOME isn't taken for the normal one.
fn account_home() -> Option<PathBuf> {
    // SAFETY: getpwuid returns static storage or null; read at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
        Some(PathBuf::from(dir.to_string_lossy().into_owned()))
    }
}

/// service_for's last answer and the home it was for: the vault is asked
/// for on every reading.
static LAST_SERVICE: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// The keychain service rush's folder keeps its sign-ins under.
fn service_of(dir: &Path) -> String {
    let mut last = LAST_SERVICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((d, s)) = last.as_ref()
        && d == dir
    {
        return s.clone();
    }
    // With no home to go by, it's taken for another home: never the
    // normal one's sign-ins by mistake.
    let s = match account_home() {
        Some(h) => service_for(dir, &h),
        None => service_for(dir, Path::new("/nonexistent")),
    };
    *last = Some((dir.to_path_buf(), s.clone()));
    s
}

/// Where rush keeps the sign-ins of the logins not in use: this rush
/// home's own (see [`service_for`]).
pub fn vault() -> Keys {
    let dir = crate::dir();
    let service = Some(service_of(&dir));
    Keys {
        dir: dir.join("logins"),
        service,
    }
}

impl Keys {
    /// A vault of files only you can read in dir, on any system.
    pub fn in_files(dir: PathBuf) -> Keys {
        Keys { dir, service: None }
    }

    /// The keychain service its sign-ins are under; None when they're
    /// files.
    pub fn service(&self) -> Option<&str> {
        self.service.as_deref()
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn get(&self, id: &str) -> io::Result<Vec<u8>> {
        if let Some(svc) = &self.service {
            return keychain::read(svc, id);
        }
        fs::read(self.file(id))
    }

    pub fn put(&self, id: &str, cred: &[u8]) -> io::Result<()> {
        if let Some(svc) = &self.service {
            return keychain::write(svc, id, cred);
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        crate::store::write_bytes(&self.file(id), cred)
    }

    pub fn forget(&self, id: &str) -> io::Result<()> {
        if let Some(svc) = &self.service {
            return keychain::delete(svc, id);
        }
        fs::remove_file(self.file(id))
    }

    /// Keeps two rushes from switching any agent's account at once; the
    /// lock is let go when what it returns is dropped.
    pub fn lock(&self) -> io::Result<fs::File> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.dir.join(".lock"))?;
        // SAFETY: flock on a descriptor f owns.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(f)
    }
}

fn api_key_id(p: &str) -> String {
    format!("apikey-{p}")
}

/// Provider p's API key: the one rush keeps, else the one in its variable.
/// It reads the keychain: never on the UI thread.
pub fn api_key(p: &str) -> String {
    if let Ok(b) = vault().get(&api_key_id(p))
        && !b.is_empty()
    {
        return String::from_utf8_lossy(&b).into_owned();
    }
    let e = agents::key_env(p);
    if e.is_empty() {
        return String::new();
    }
    std::env::var(e).unwrap_or_default()
}

/// Keeps key as provider p's, or forgets p's when it's "". It writes the
/// keychain: never on the UI thread. Config::mark_api_key says so after.
pub fn put_api_key(p: &str, key: &str) -> io::Result<()> {
    if key.is_empty() {
        let _ = vault().forget(&api_key_id(p));
        return Ok(());
    }
    vault().put(&api_key_id(p), key.as_bytes())
}

/// A login signed in somewhere rush looks, and the name it would take: its
/// folder's, for one found in an older folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundLogin {
    pub login: Login,
    pub name: String,
}

/// What finding logins put right: an agent started before a switch had put
/// its account's sign-in back in the home.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Restored {
    /// The id of the account it had put back.
    pub was: String,
    /// The one switched to again.
    pub now: String,
    /// Why it couldn't be switched again, if it couldn't.
    pub err: Option<String>,
}

/// What a look for logins found (LoginKeeper.FindLogins's results): the
/// logins signed in where rush looks, a switch put right, whether every
/// older folder was taken in whole (so the config can forget them), and
/// why the agent's home's sign-in couldn't be kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundLogins {
    pub found: Vec<FoundLogin>,
    pub restored: Option<Restored>,
    pub imported: bool,
    pub failed: Option<String>,
}

/// The logins agent's side of its logins: it keeps each login's sign-in,
/// reads each one's plan, and signs its home in as one. Its methods read
/// the disk, the keychain and some the network: never on the UI thread.
/// After Go's internal/state/logins.go:8-31 (LoginKeeper).
pub trait LoginKeeper: Send + Sync {
    /// Keeps the sign-in the agent's home holds now, and takes in the
    /// folders an older rush was given.
    fn find_logins(&self, cfg: &crate::Config) -> FoundLogins;
    /// l's plan, shared through path like every account's.
    fn refresh_login(
        &self,
        path: &str,
        cfg: &crate::Config,
        l: &Login,
        offline: bool,
    ) -> rush_core::usage::Reading;
    /// Refreshes l's sign-in, unless an agent running on it will.
    fn renew_login(&self, cfg: &crate::Config, l: &Login) -> Result<(), String>;
    /// The login new sessions run as, in a home of its own; "" runs them
    /// in the agent's home as whoever it's signed in as.
    fn using_login(&self, cfg: &crate::Config) -> String;
    /// Makes l the login new sessions run as.
    fn use_login(&self, cfg: &crate::Config, l: &Login) -> io::Result<()>;
    /// Readies the sign-in new sessions now run on (after use_login) for
    /// several to start on at once: refreshed first if it has run out or
    /// is about to. Whether it refreshed. Rust-only.
    fn ready_login(&self, _cfg: &crate::Config) -> Result<bool, String> {
        Ok(false)
    }
    /// Keeps the sign-in just made in p as its login's.
    fn adopt_login(&self, p: &rush_core::adapter::Profile) -> io::Result<Login>;
    /// Drops what rush keeps of the login with id.
    fn forget_login(&self, id: &str) -> io::Result<()>;
}

/// Generic passwords in the macOS login keychain, through security(1).
pub mod keychain {
    use super::*;

    const SECURITY: &str = "/usr/bin/security";

    fn not_found() -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, "keychain: no such item")
    }

    type Items = std::collections::BTreeMap<(String, String), Vec<u8>>;

    /// In a test (rush_core::sandbox), the keychain is this process's map
    /// of (service, account) to secret, empty at the start: a test never
    /// reaches the real one. Never anything outside a test.
    fn fake() -> Option<std::sync::MutexGuard<'static, Items>> {
        static FAKE: Mutex<Items> = Mutex::new(Items::new());
        rush_core::sandbox::active().then(|| FAKE.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The fake's item for service and account; an empty account is any.
    fn fake_find<'a>(
        f: &'a Items,
        service: &str,
        account: &str,
    ) -> Option<(&'a (String, String), &'a Vec<u8>)> {
        f.iter()
            .find(|((s, a), _)| s == service && (account.is_empty() || a == account))
    }

    /// service's secret for account; an empty account is any.
    pub fn read(service: &str, account: &str) -> io::Result<Vec<u8>> {
        if let Some(f) = fake() {
            return fake_find(&f, service, account)
                .map(|(_, v)| v.clone())
                .ok_or_else(not_found);
        }
        let mut args = vec!["find-generic-password", "-s", service, "-w"];
        if !account.is_empty() {
            args.extend(["-a", account]);
        }
        let out = Command::new(SECURITY)
            .args(&args)
            .stderr(Stdio::null())
            .output()
            .map_err(|_| not_found())?;
        if !out.status.success() {
            return Err(not_found());
        }
        let mut b = out.stdout;
        while b.last() == Some(&b'\n') {
            b.pop();
        }
        // A secret that isn't one line of text is printed as hex; -g says
        // whether it was.
        if !b.is_empty()
            && let Some(dec) = from_hex(&b)
            && printed_hex(&args)
        {
            return Ok(dec);
        }
        Ok(b)
    }

    /// Whether security prints the secret args find as hex: -g writes it
    /// to stderr as password: 0x… when it does.
    fn printed_hex(args: &[&str]) -> bool {
        let mut a: Vec<&str> = args.iter().copied().filter(|a| *a != "-w").collect();
        a.push("-g");
        match Command::new(SECURITY)
            .args(&a)
            .stdout(Stdio::null())
            .output()
        {
            Ok(o) => o.status.success() && o.stderr.starts_with(b"password: 0x"),
            Err(_) => false,
        }
    }

    /// Whether service keeps an item for account, found without reading
    /// its secret.
    pub fn has(service: &str, account: &str) -> bool {
        if let Some(f) = fake() {
            return f.contains_key(&(service.to_string(), account.to_string()));
        }
        Command::new(SECURITY)
            .args(["find-generic-password", "-s", service, "-a", account])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Goes through security's own prompt rather than its arguments where
    /// it fits, so the secret doesn't show in the process list; a longer
    /// one goes as an argument, which only your own user can read.
    /// security's output echoes the secret back, so it's never passed on.
    pub fn write(service: &str, account: &str, secret: &[u8]) -> io::Result<()> {
        if let Some(mut f) = fake() {
            f.insert((service.to_string(), account.to_string()), secret.to_vec());
            return Ok(());
        }
        let hex = to_hex(secret);
        let line = format!(
            "add-generic-password -U -a {} -s {} -X {}\n",
            go_quote(account),
            go_quote(service),
            go_quote(&hex)
        );
        let out = if line.len() < 4000 {
            let mut child = Command::new(SECURITY)
                .arg("-i")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(line.as_bytes());
            }
            child.wait_with_output()
        } else {
            Command::new(SECURITY)
                .args([
                    "add-generic-password",
                    "-U",
                    "-a",
                    account,
                    "-s",
                    service,
                    "-X",
                    &hex,
                ])
                .output()
        };
        let ok = match out {
            Ok(o) => {
                o.status.success()
                    && !o.stdout.windows(15).any(|w| w == b"unknown command")
                    && !o.stderr.windows(15).any(|w| w == b"unknown command")
            }
            Err(_) => false,
        };
        if !ok {
            return Err(io::Error::other(format!(
                "couldn't save a sign-in to the keychain ({service})"
            )));
        }
        Ok(())
    }

    /// Removes service's item for account; an empty account is any.
    pub fn delete(service: &str, account: &str) -> io::Result<()> {
        if let Some(mut f) = fake() {
            let k = fake_find(&f, service, account)
                .map(|(k, _)| k.clone())
                .ok_or_else(not_found)?;
            f.remove(&k);
            return Ok(());
        }
        let mut args = vec!["delete-generic-password", "-s", service];
        if !account.is_empty() {
            args.extend(["-a", account]);
        }
        let ok = Command::new(SECURITY)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok { Ok(()) } else { Err(not_found()) }
    }

    fn to_hex(b: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(b.len() * 2);
        for c in b {
            s.push(HEX[(c >> 4) as usize] as char);
            s.push(HEX[(c & 0xf) as usize] as char);
        }
        s
    }

    fn from_hex(b: &[u8]) -> Option<Vec<u8>> {
        if !b.len().is_multiple_of(2) {
            return None;
        }
        let val = |c: u8| -> Option<u8> {
            match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            }
        };
        b.chunks(2)
            .map(|p| Some(val(p[0])? << 4 | val(p[1])?))
            .collect()
    }

    /// Go's %q for the plain strings a service, account or hex are.
    fn go_quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                    out.push_str(&format!("\\x{:02x}", c as u32))
                }
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hex_and_quotes() {
            assert_eq!(to_hex(b"{\n}"), "7b0a7d");
            assert_eq!(from_hex(b"7b0A7d").unwrap(), b"{\n}");
            assert!(from_hex(b"7g").is_none());
            assert_eq!(go_quote("a\"b"), "\"a\\\"b\"");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Off macOS the vault is files only you can read, and its lock is
    // let go when dropped.
    #[test]
    fn files_vault() {
        let d = tempfile::tempdir().unwrap();
        let v = Keys::in_files(d.path().join("logins"));
        assert!(v.get("a").is_err());
        v.put("a", b"secret").unwrap();
        assert_eq!(v.get("a").unwrap(), b"secret");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(d.path().join("logins/a.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let l = v.lock().unwrap();
        drop(l);
        let _again = v.lock().unwrap();
        v.forget("a").unwrap();
        assert!(v.get("a").is_err());
    }

    // The normal home keeps the old service, so its sign-ins carry on;
    // any other home, test copies included, has one of its own.
    #[test]
    fn each_home_has_its_own_service() {
        let t = tempfile::tempdir().unwrap();
        let user = t.path().join("me");
        let config = user.join(".config");
        fs::create_dir_all(config.join("agtop")).unwrap();
        fs::create_dir_all(t.path().join("copy")).unwrap();
        std::os::unix::fs::symlink(config.join("agtop"), t.path().join("link")).unwrap();
        for (home, default) in [
            (config.join("rush"), true),
            (config.join("agtop"), true),
            (t.path().join("link"), true),
            (t.path().join("copy"), false),
            (t.path().join("other"), false),
        ] {
            let s = service_for(&home, &user);
            assert_eq!(s == DEFAULT_SERVICE, default, "{}: {s}", home.display());
            if !default {
                assert!(
                    s.starts_with("rush-login-") && s.len() == "rush-login-".len() + 8,
                    "{s}"
                );
            }
        }
        assert_ne!(
            service_for(&t.path().join("copy"), &user),
            service_for(&t.path().join("other"), &user)
        );
        assert_eq!(
            service_for(&t.path().join("copy"), &user),
            service_for(&t.path().join("copy/../copy"), &user),
            "named by its canonical path"
        );
    }

    // Two rush homes never see each other's sign-ins, and a home other
    // than the normal one doesn't read the normal one's (the fake
    // keychain stands in for the real one in a test).
    #[test]
    fn homes_keep_their_own_sign_ins() {
        let t = tempfile::tempdir().unwrap();
        keychain::write(DEFAULT_SERVICE, "acct-1", b"the user's own").unwrap();
        crate::set_thread_dir(Some(t.path().join("a")));
        let a = vault();
        assert!(a.get("acct-1").is_err(), "read the normal home's sign-in");
        a.put("acct-1", b"a's").unwrap();
        crate::set_thread_dir(Some(t.path().join("b")));
        let b = vault();
        assert_ne!(a, b);
        assert!(b.get("acct-1").is_err(), "b saw a's sign-in");
        b.put("acct-1", b"b's").unwrap();
        b.forget("acct-1").unwrap();
        crate::set_thread_dir(None);
        assert_eq!(a.get("acct-1").unwrap(), b"a's");
        assert_eq!(
            keychain::read(DEFAULT_SERVICE, "acct-1").unwrap(),
            b"the user's own",
            "the normal home's sign-in was changed"
        );
        {
            assert!(a.service().unwrap().starts_with("rush-login-"));
            assert_ne!(a.service(), b.service());
        }
    }
}
