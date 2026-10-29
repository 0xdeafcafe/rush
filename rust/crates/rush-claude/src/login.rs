//! Claude Code's sign-ins: who a config folder is signed in as, the
//! sign-in itself (in the login keychain on macOS, .credentials.json
//! elsewhere), and rush's vault of the logins not in use. Port of
//! internal/adapters/claude/claude/login.go.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use rush_state::Keys;
use rush_state::config::Login;
use rush_state::json::RawJson;
use rush_state::vault::keychain;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::account::Account;
use crate::files::write_file_atomic;
use crate::json::nd;
use crate::oauth::{self, OauthError};

/// rush's vault, with what it does with Claude Code's sign-ins.
pub struct Vault(pub Keys);

/// rush's vault, where the logins not in use keep their sign-ins.
pub fn the_vault() -> Vault {
    Vault(rush_state::vault())
}

/// Who a config folder is signed in as, from its state file, with the
/// sign-in itself; None when it isn't signed in.
pub fn signed(a: &Account) -> Option<(Login, Vec<u8>)> {
    let prof = read_profile(&a.state_path())?;
    let cred = read_creds(a).ok()?;
    if !usable(&cred) {
        return None;
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct P {
        #[serde(rename = "accountUuid", deserialize_with = "nd")]
        uuid: String,
        #[serde(rename = "emailAddress", deserialize_with = "nd")]
        email: String,
        #[serde(rename = "organizationName", deserialize_with = "nd")]
        org: String,
    }
    let p: P = serde_json::from_str(prof.get()).ok()?;
    if p.uuid.is_empty() {
        return None;
    }
    Some((
        Login {
            id: p.uuid,
            email: p.email,
            org: p.org,
            profile: Some(prof),
            ..Login::default()
        },
        cred,
    ))
}

/// The uuid of the account a config folder is signed in as, from its
/// state file alone.
pub fn signed_in_as(a: &Account) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct P {
        #[serde(rename = "accountUuid", deserialize_with = "nd")]
        uuid: String,
    }
    read_profile(&a.state_path())
        .and_then(|p| serde_json::from_str::<P>(p.get()).ok())
        .map(|p| p.uuid)
        .unwrap_or_default()
}

impl Vault {
    pub fn get(&self, id: &str) -> io::Result<Vec<u8>> {
        self.0.get(id)
    }
    pub fn put(&self, id: &str, cred: &[u8]) -> io::Result<()> {
        self.0.put(id, cred)
    }
    pub fn forget(&self, id: &str) -> io::Result<()> {
        self.0.forget(id)
    }
    pub fn lock(&self) -> io::Result<fs::File> {
        self.0.lock()
    }

    /// Saves the sign-in a folder holds now into the vault, when it has
    /// changed: Claude Code replaces its tokens as it refreshes them, and
    /// only the newest works. It's kept as whoever it belongs to, which
    /// owner reports when that isn't the account the folder names (a Claude
    /// Code started before a switch put its own back); when Anthropic
    /// can't be asked, the folder is taken at its word. None: it isn't
    /// signed in.
    pub fn keep(&self, a: &Account) -> io::Result<Option<(Login, String)>> {
        let Some((l, cred)) = signed(a) else {
            return Ok(None);
        };
        if self.get(&l.id).is_ok_and(|old| old == cred) {
            return Ok(Some((l, String::new())));
        }
        if let Ok(id) = oauth::owner(&cred)
            && id != l.id
        {
            if self.get(&id).is_ok_and(|old| old == cred) {
                return Ok(Some((l, id)));
            }
            self.put(&id, &cred)?;
            return Ok(Some((l, id)));
        }
        self.put(&l.id, &cred)?;
        Ok(Some((l, String::new())))
    }

    /// Signs root in as to: whatever root holds now is kept first, so
    /// switching back finds it as it was. A Claude Code already running
    /// keeps the account it started with until it restarts; when it next
    /// refreshes its sign-in it sees the stored one changed and takes that
    /// instead of writing its own back.
    pub fn use_login(&self, root: &Account, to: &Login) -> io::Result<()> {
        let _lock = self.lock()?;
        let cred = match self.get(&to.id) {
            Ok(c) if usable(&c) => c,
            _ => {
                return Err(io::Error::other(format!(
                    "rush has no sign-in for {}; sign in to it again",
                    to.name
                )));
            }
        };
        let Some(prof) = &to.profile else {
            return Err(io::Error::other(format!(
                "rush doesn't know who {} is; sign in to it again",
                to.name
            )));
        };
        if let Ok(id) = oauth::owner(&cred)
            && id != to.id
        {
            // Kept under the wrong name by a rush from before it checked.
            let _ = self.forget(&to.id);
            return Err(io::Error::other(format!(
                "rush's sign-in for {} was another account's; sign in to it again",
                to.name
            )));
        }
        self.keep(root)
            .map_err(|e| io::Error::other(format!("couldn't keep the account in use: {e}")))?;
        let mut cred = cred;
        if let Ok(now) = read_creds(root) {
            cred = with_mcp_logins(&cred, &now);
        }
        write_creds(root, &cred)?;
        write_profile(&root.state_path(), prof)
    }

    /// Takes the sign-in a fresh folder was just signed in with (the one
    /// `claude auth login` ran in) into the vault, then removes the folder
    /// and its sign-in: the vault's copy is the only one.
    pub fn adopt(&self, scratch: &Account) -> io::Result<Login> {
        let Some((l, cred)) = signed(scratch) else {
            return Err(io::Error::other("not signed in"));
        };
        self.put(&l.id, &cred)?;
        let _ = delete_creds(scratch);
        let _ = fs::remove_dir_all(&scratch.config_dir);
        Ok(l)
    }
}

/// Says in a folder's state file that it's signed in as l, for a sign-in
/// that is l's already.
pub fn name(a: &Account, l: &Login) -> io::Result<()> {
    match &l.profile {
        Some(p) => write_profile(&a.state_path(), p),
        None => Ok(()),
    }
}

/// cred carrying the MCP servers' logins now holds: Claude Code keeps them
/// in the same item as the account's sign-in, but they're yours, not the
/// account's, so a switch leaves them as they are rather than dropping
/// them or bringing back the other account's older copies.
pub fn with_mcp_logins(cred: &[u8], now: &[u8]) -> Vec<u8> {
    if !usable(now) {
        return cred.to_vec(); // nothing sound to go by: leave cred as it was
    }
    let Ok(from) = serde_json::from_slice::<BTreeMap<String, Box<RawValue>>>(now) else {
        return cred.to_vec();
    };
    let Ok(mut to) = serde_json::from_slice::<BTreeMap<String, Box<RawValue>>>(cred) else {
        return cred.to_vec();
    };
    match from.get("mcpOAuth") {
        Some(v) if !v.get().is_empty() && v.get() != "null" => {
            to.insert("mcpOAuth".into(), v.clone());
        }
        _ => {
            to.remove("mcpOAuth"); // signed out of them all
        }
    }
    rush_state::json::to_vec(&to).unwrap_or_else(|_| cred.to_vec())
}

/// Whether a stored sign-in has what Claude Code needs to carry on with
/// it: a refresh token.
pub fn usable(cred: &[u8]) -> bool {
    oauth::refresh_token(cred).is_some_and(|r| !r.is_empty())
}

/// Who a state file says it's signed in as: its oauthAccount, as written.
pub(crate) fn read_profile(state_path: &std::path::Path) -> Option<RawJson> {
    let b = fs::read(state_path).ok()?;
    #[derive(Deserialize)]
    struct F {
        #[serde(rename = "oauthAccount")]
        oauth: Option<Box<RawValue>>,
    }
    let f: F = serde_json::from_slice(&b).ok()?;
    let v = f.oauth?;
    if v.get() == "null" {
        return None;
    }
    Some(RawJson(v))
}

/// Puts prof in the state file as who it's signed in as, leaving
/// everything else. The usage Claude Code cached was the other account's,
/// so it goes.
pub(crate) fn write_profile(state_path: &std::path::Path, prof: &RawJson) -> io::Result<()> {
    let mut all: BTreeMap<String, Box<RawValue>> = BTreeMap::new();
    match fs::read(state_path) {
        Ok(b) if !b.is_empty() => {
            all = serde_json::from_slice(&b).map_err(|e| {
                io::Error::other(format!("{} isn't readable: {e}", state_path.display()))
            })?;
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    all.insert("oauthAccount".into(), prof.0.clone());
    all.remove("cachedUsageUtilization");
    write_file_atomic(state_path, &indent(&all)?, 0o600)
}

/// v as Go's jsonx.MarshalIndent writes it: two spaces a level. Values
/// kept as written (Claude Code's own, in its state file) are indented too
/// but keep their keys' order and their numbers as written, as Go's do:
/// only the outer map's keys are in order.
pub(crate) fn indent<T: serde::Serialize>(v: &T) -> io::Result<Vec<u8>> {
    Ok(reindent(
        &rush_state::json::to_vec(v).map_err(io::Error::other)?,
    ))
}

/// JSON text b indented two spaces a level, its whitespace dropped and
/// nothing else changed: an empty object or array stays `{}` / `[]`.
fn reindent(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len() + b.len() / 4);
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    let line = |out: &mut Vec<u8>, depth: usize| {
        out.push(b'\n');
        out.resize(out.len() + 2 * depth, b' ');
    };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        if in_str {
            out.push(c);
            match c {
                _ if esc => esc = false,
                b'\\' => esc = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => {}
            b'"' => {
                in_str = true;
                out.push(c);
            }
            b'{' | b'[' => {
                out.push(c);
                let close = if c == b'{' { b'}' } else { b']' };
                let mut j = i;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if b.get(j) == Some(&close) {
                    out.push(close);
                    i = j + 1;
                } else {
                    depth += 1;
                    line(&mut out, depth);
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                line(&mut out, depth);
                out.push(c);
            }
            b',' => {
                out.push(c);
                line(&mut out, depth);
            }
            b':' => out.extend_from_slice(b": "),
            _ => out.push(c),
        }
    }
    out
}

/// Keeps sign-ins in .credentials.json files on any system: for tests,
/// which mustn't touch the keychain.
static CREDS_IN_FILES: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn creds_in_files() {
    CREDS_IN_FILES.store(true, Ordering::Relaxed);
}

fn in_keychain() -> bool {
    cfg!(target_os = "macos") && !cfg!(test) && !CREDS_IN_FILES.load(Ordering::Relaxed)
}

/// Your user name, the account name programs keep their own keychain
/// items under (photon/keychain.User).
fn user() -> String {
    std::env::var("USER").unwrap_or_default()
}

/// The sign-in Claude Code keeps for a config folder: in the login
/// keychain on macOS, in .credentials.json elsewhere.
pub fn read_creds(a: &Account) -> Result<Vec<u8>, OauthError> {
    if in_keychain() {
        let svc = a.keychain_service();
        if let Ok(c) = keychain::read(&svc, &user()) {
            return Ok(c);
        }
        if let Ok(c) = keychain::read(&svc, "") {
            return Ok(c);
        }
        return Err(OauthError::NotSignedIn);
    }
    fs::read(a.config_dir.join(".credentials.json")).map_err(|_| OauthError::NotSignedIn)
}

pub fn write_creds(a: &Account, cred: &[u8]) -> io::Result<()> {
    if in_keychain() {
        return keychain::write(&a.keychain_service(), &user(), cred);
    }
    write_file_atomic(&a.config_dir.join(".credentials.json"), cred, 0o600)
}

pub fn delete_creds(a: &Account) -> io::Result<()> {
    if in_keychain() {
        return keychain::delete(&a.keychain_service(), &user());
    }
    fs::remove_file(a.config_dir.join(".credentials.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // login_test.go (TestSwitchKeepsMCPLogins): a switch keeps the MCP
    // servers' logins the folder holds now, not the other account's older
    // ones.
    #[test]
    fn switch_keeps_mcp_logins() {
        let now =
            br#"{"claudeAiOauth":{"refreshToken":"a"},"mcpOAuth":{"posthog":{"accessToken":"p"}}}"#;
        let to = br#"{"claudeAiOauth":{"refreshToken":"b"},"mcpOAuth":{"old":{}}}"#;
        let s = |b: Vec<u8>| String::from_utf8(b).unwrap();
        assert_eq!(
            s(with_mcp_logins(to, now)),
            r#"{"claudeAiOauth":{"refreshToken":"b"},"mcpOAuth":{"posthog":{"accessToken":"p"}}}"#
        );
        assert_eq!(
            s(with_mcp_logins(to, br#"{"claudeAiOauth":{}}"#)),
            s(to.to_vec()),
            "with no sign-in now to go by"
        );
        assert_eq!(
            s(with_mcp_logins(
                to,
                br#"{"claudeAiOauth":{"refreshToken":"a"}}"#
            )),
            r#"{"claudeAiOauth":{"refreshToken":"b"}}"#,
            "signed out of every MCP server now"
        );
    }

    // Go's MarshalIndent keeps a raw value's keys and numbers as written.
    #[test]
    fn indent_keeps_raw_values_as_written() {
        let mut all: BTreeMap<String, Box<RawValue>> = BTreeMap::new();
        let raw = |s: &str| RawValue::from_string(s.into()).unwrap();
        all.insert(
            "z".into(),
            raw(r#"{"b": 1.50, "a": [ ], "c": {"s": "x, {y}: \\\"z"}}"#),
        );
        all.insert("a".into(), raw("[1,{}]"));
        let got = String::from_utf8(indent(&all).unwrap()).unwrap();
        assert_eq!(
            got,
            "{\n  \"a\": [\n    1,\n    {}\n  ],\n  \"z\": {\n    \"b\": 1.50,\n    \"a\": [],\n    \"c\": {\n      \"s\": \"x, {y}: \\\\\\\"z\"\n    }\n  }\n}"
        );
    }

    #[test]
    fn who_a_folder_is_signed_in_as() {
        creds_in_files();
        let dir = tempfile::tempdir().unwrap();
        let a = Account::at("", dir.path().join("cfg"));
        fs::create_dir_all(&a.config_dir).unwrap();
        assert_eq!(signed_in_as(&a), "");
        fs::write(
            a.state_path(),
            r#"{"numStartups":3,"oauthAccount":{"accountUuid":"u-1","emailAddress":"me@x","organizationName":"Org"},"cachedUsageUtilization":{}}"#,
        )
        .unwrap();
        assert_eq!(signed_in_as(&a), "u-1");
        assert!(signed(&a).is_none(), "no sign-in beside it");
        write_creds(&a, br#"{"claudeAiOauth":{"refreshToken":"r"}}"#).unwrap();
        let (l, _) = signed(&a).unwrap();
        assert_eq!(
            (l.id.as_str(), l.email.as_str(), l.org.as_str()),
            ("u-1", "me@x", "Org")
        );

        // Naming another login keeps the rest of the state file, less the
        // usage cached for the one before.
        let other = Login {
            id: "u-2".into(),
            profile: Some(RawJson(
                RawValue::from_string(r#"{"accountUuid":"u-2"}"#.into()).unwrap(),
            )),
            ..Login::default()
        };
        name(&a, &other).unwrap();
        assert_eq!(signed_in_as(&a), "u-2");
        let st: serde_json::Value =
            serde_json::from_slice(&fs::read(a.state_path()).unwrap()).unwrap();
        assert_eq!(st["numStartups"], 3);
        assert!(st.get("cachedUsageUtilization").is_none());

        let vault = Vault(Keys::in_files(dir.path().join("vault")));
        let l = vault.adopt(&a).unwrap();
        assert_eq!(l.id, "u-2");
        assert!(vault.get("u-2").is_ok_and(|c| usable(&c)));
        assert!(!a.config_dir.exists(), "the vault's copy is the only one");
    }
}
