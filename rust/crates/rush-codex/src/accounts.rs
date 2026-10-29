//! Codex keeps its sign-in in its home's auth.json: a ChatGPT sign-in's
//! tokens, or an API key. An account rush keeps is a copy of that file in
//! the vault, and switching puts one in place of the other. Port of
//! internal/adapters/codex/accounts.go:1-219.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use rush_core::adapter::{self, Account, Kind, Profile, SignInDone};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::KIND;
use crate::json::{get, s};
use crate::quota::{hex, random};

/// Where the vault keeps an account's auth.json.
fn vault_key(id: &str) -> String {
    format!("codex:{id}")
}

fn err(msg: impl Into<String>) -> adapter::Error {
    msg.into().into()
}

/// The account an auth.json signs in as. It never returns a secret: an
/// API key's account is named by a hash of it.
pub(crate) fn who_is(b: &[u8]) -> adapter::Result<Account> {
    let f: Value = serde_json::from_slice(b)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| err("codex: its auth.json isn't readable"))?;
    let mut a = Account {
        kind: Kind::from(KIND),
        ..Account::default()
    };
    let tokens = get(&f, "tokens");
    let key = s(&f, "OPENAI_API_KEY");
    match tokens
        .map(|t| s(t, "account_id"))
        .filter(|id| !id.is_empty())
    {
        Some(id) => {
            a.id = id.to_string();
            let claims = jwt_claims(s(tokens.expect("has tokens"), "id_token"));
            a.email = s(&claims, "email").to_string();
            a.plan = get(&claims, "https://api.openai.com/auth")
                .map(|x| s(x, "chatgpt_plan_type").to_string())
                .unwrap_or_default();
        }
        None if !key.is_empty() => {
            let sum = Sha256::digest(key.as_bytes());
            a.id = format!("apikey-{}", hex(&sum[..4]));
            (a.name, a.plan) = ("API key".into(), "API key".into());
        }
        None => return Err(err("codex: not signed in (codex login)")),
    }
    a.key = format!("codex:{}", a.id);
    Ok(a)
}

/// Who an id token names; it isn't checked, only read.
fn jwt_claims(tok: &str) -> Value {
    let parts: Vec<&str> = tok.split('.').collect();
    if parts.len() < 2 {
        return Value::Null;
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

fn auth_path(p: &Profile) -> PathBuf {
    Path::new(&p.dir).join("auth.json")
}

/// Who p's auth.json signs in as.
pub(crate) fn current(p: &Profile) -> adapter::Result<Account> {
    let b = std::fs::read(auth_path(p)).map_err(|_| {
        err("codex: no auth.json; not signed in, or its sign-in is in the keyring, which rush can't switch")
    })?;
    who_is(&b)
}

fn first_of<'a>(vs: &[&'a str]) -> &'a str {
    vs.iter().find(|v| !v.is_empty()).copied().unwrap_or("")
}

/// Puts a's auth.json in p, keeping p's own first: Codex refreshes its
/// tokens as it goes, and only the newest work.
pub(crate) fn switch(p: &Profile, a: &Account) -> adapter::Result<()> {
    let r = switch_in(&rush_state::vault::vault(), p, a);
    switched("codex", &p.dir, a, r.as_ref().err());
    r
}

fn switch_in(v: &rush_state::vault::Keys, p: &Profile, a: &Account) -> adapter::Result<()> {
    let _lock = v.lock()?;
    let who = first_of(&[&a.name, &a.email, &a.id]);
    let next = v.get(&vault_key(&a.id)).map_err(|_| {
        err(format!(
            "rush has no sign-in for {who}; sign in to it again"
        ))
    })?;
    if who_is(&next).map(|w| w.id).ok().as_deref() != Some(a.id.as_str()) {
        return Err(err(format!(
            "rush's sign-in for {who} isn't that account's; sign in to it again"
        )));
    }
    if let Ok(cur) = std::fs::read(auth_path(p))
        && let Ok(w) = who_is(&cur)
        && v.get(&vault_key(&w.id)).ok().as_deref() != Some(cur.as_slice())
    {
        v.put(&vault_key(&w.id), &cur)
            .map_err(|e| err(format!("couldn't keep the account in use: {e}")))?;
    }
    write_atomic(&auth_path(p), &next)?;
    Ok(())
}

/// Logs an account switch, and how it went.
fn switched(agent: &str, dir: &str, a: &Account, e: Option<&adapter::Error>) {
    let mut f = rush_events::Fields::new();
    f.insert("agent".into(), agent.into());
    f.insert("dir".into(), dir.into());
    f.insert("to".into(), a.id.clone().into());
    if let Some(e) = e {
        f.insert("err".into(), e.to_string().into());
    }
    rush_events::log("account-switched", f);
}

/// Saves p's auth.json in the vault as whoever it signs in as, when it
/// has changed.
pub fn keep(p: &Profile) -> adapter::Result<Account> {
    let b = std::fs::read(auth_path(p))?;
    let who = who_is(&b)?;
    let v = rush_state::vault::vault();
    if v.get(&vault_key(&who.id)).ok().as_deref() == Some(b.as_slice()) {
        return Ok(who);
    }
    v.put(&vault_key(&who.id), &b)?;
    Ok(who)
}

/// `codex login` in a home of its own, which keeps its sign-in in a file
/// there; done takes it into the vault and removes the home.
pub(crate) fn sign_in() -> adapter::Result<(Command, SignInDone)> {
    let home = rush_state::dir().join(format!("signin-codex-{}", hex(&random::<4>())));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&home)?;
    write_private(
        &home.join("config.toml"),
        b"cli_auth_credentials_store = \"file\"\n",
    )?;
    let mut cmd = Command::new(crate::rpc::program());
    cmd.arg("login").env("CODEX_HOME", &home);
    let done: SignInDone = Box::new(move || {
        let out = (|| {
            let b = std::fs::read(home.join("auth.json")).map_err(|_| err("not signed in"))?;
            let who = who_is(&b)?;
            rush_state::vault::vault().put(&vault_key(&who.id), &b)?;
            Ok(who)
        })();
        let _ = std::fs::remove_dir_all(&home);
        out
    });
    Ok((cmd, done))
}

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

fn write_private(path: &Path, b: &[u8]) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(b)
}

/// Drops rush's copy of a's auth.json.
pub(crate) fn forget(a: &Account) -> adapter::Result<()> {
    rush_state::vault::vault().forget(&vault_key(&a.id))?;
    Ok(())
}

/// Writes b to path through a file beside it, only you can read.
fn write_atomic(path: &Path, b: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".auth-{}.json", hex(&random::<6>())));
    let r = write_private(&tmp, b).and_then(|_| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(v: Value) -> Vec<u8> {
        serde_json::to_vec(&v).unwrap()
    }

    // accounts_test.go:TestWhoIs: an auth.json says who it signs in as,
    // from its id token, and an API key's account is named without the
    // key.
    #[test]
    fn who_is_signed_in() {
        let claims = serde_json::json!({"email": "me@example.com", "https://api.openai.com/auth": {"chatgpt_plan_type": "plus"}});
        let tok = format!(
            "e30.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        );
        let a = who_is(&auth(serde_json::json!({"auth_mode": "chatgpt", "tokens": {"id_token": tok, "account_id": "acct-1", "refresh_token": "secret"}}))).unwrap();
        assert_eq!(
            (
                a.id.as_str(),
                a.email.as_str(),
                a.plan.as_str(),
                a.key.as_str()
            ),
            ("acct-1", "me@example.com", "plus", "codex:acct-1")
        );
        let a = who_is(&auth(
            serde_json::json!({"auth_mode": "apikey", "OPENAI_API_KEY": "sk-secret"}),
        ))
        .unwrap();
        assert!(
            a.id.starts_with("apikey-") && !(a.id.clone() + &a.name).contains("secret"),
            "{a:?}"
        );
        assert!(who_is(br#"{"auth_mode":"chatgpt"}"#).is_err());
        assert!(who_is(b"not json").is_err());
    }

    // Switching keeps the sign-in in use, and puts the other in its place,
    // in a vault of files.
    #[test]
    fn switches_through_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let v = rush_state::vault::Keys::in_files(dir.path().join("vault"));
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let p = Profile {
            dir: home.to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let one = auth(serde_json::json!({"tokens": {"account_id": "one"}}));
        let two = auth(serde_json::json!({"tokens": {"account_id": "two"}}));
        std::fs::write(home.join("auth.json"), &one).unwrap();
        let to = Account {
            id: "two".into(),
            name: "Two".into(),
            ..Account::default()
        };
        let e = switch_in(&v, &p, &to).unwrap_err();
        assert_eq!(
            e.to_string(),
            "rush has no sign-in for Two; sign in to it again"
        );
        v.put("codex:two", &one).unwrap();
        let e = switch_in(&v, &p, &to).unwrap_err();
        assert!(e.to_string().contains("isn't that account's"), "{e}");
        v.put("codex:two", &two).unwrap();
        switch_in(&v, &p, &to).unwrap();
        assert_eq!(std::fs::read(home.join("auth.json")).unwrap(), two);
        assert_eq!(v.get("codex:one").unwrap(), one, "the one in use is kept");
        assert_eq!(current(&p).unwrap().id, "two");
    }
}
