//! The vault keeps the sign-ins of accounts not in use: in the login
//! keychain on macOS (through security(1), as photon/keychain does), in
//! files only you can read elsewhere. A provider's API key is kept there
//! too; the config says only which providers have one, so drawing never
//! reads the keychain.
//!
//! Ported from internal/state/vault.go:1-62 (Keys, Vault, Get, Put, Forget,
//! Lock), apikeys.go:1-39 (APIKey, PutAPIKey), logins.go:36-45 (FoundLogin,
//! Restored) and photon/keychain (Read, Write, Delete).

use crate::agents;
use crate::config::Login;
use std::fs;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Where rush keeps sign-ins: the keychain on macOS, else files in Dir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub dir: PathBuf,
    /// Whether it's the keychain: on macOS, unless made over files.
    keychain: bool,
}

const VAULT_SERVICE: &str = "rush-login";

/// Where rush keeps the sign-ins of the logins not in use.
pub fn vault() -> Keys {
    Keys {
        dir: crate::dir().join("logins"),
        keychain: cfg!(target_os = "macos"),
    }
}

impl Keys {
    /// A vault of files only you can read in dir, on any system.
    pub fn in_files(dir: PathBuf) -> Keys {
        Keys {
            dir,
            keychain: false,
        }
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn get(&self, id: &str) -> io::Result<Vec<u8>> {
        if self.keychain {
            return keychain::read(VAULT_SERVICE, id);
        }
        fs::read(self.file(id))
    }

    pub fn put(&self, id: &str, cred: &[u8]) -> io::Result<()> {
        if self.keychain {
            return keychain::write(VAULT_SERVICE, id, cred);
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        crate::store::write_bytes(&self.file(id), cred)
    }

    pub fn forget(&self, id: &str) -> io::Result<()> {
        if self.keychain {
            return keychain::delete(VAULT_SERVICE, id);
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

/// Generic passwords in the macOS login keychain, through security(1).
pub mod keychain {
    use super::*;

    const SECURITY: &str = "/usr/bin/security";

    fn not_found() -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, "keychain: no such item")
    }

    /// service's secret for account; an empty account is any.
    pub fn read(service: &str, account: &str) -> io::Result<Vec<u8>> {
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
}
