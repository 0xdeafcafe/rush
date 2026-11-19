//! Keeping the sign-ins rush holds for Codex accounts not in use from
//! lapsing. Codex refreshes the sign-in in its home once it's eight days
//! old, but one kept in the vault is never used, so by the time it's
//! switched to its refresh token may have expired. This refreshes it the
//! way Codex would, before then. Not in Go. Codex's own refresh is
//! codex-rs/core/src/auth.rs (try_refresh_token): a JSON POST of
//! {client_id, grant_type: "refresh_token", refresh_token, scope} to
//! auth.openai.com/oauth/token, answered with id_token and, when they
//! changed, access_token and refresh_token; last_refresh set to now.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use rush_core::adapter::{self, Account, Profile};
use rush_core::time::{Ms, format_rfc3339, now, parse_rfc3339};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::{vault_key, who_is};
use crate::json::{get, s};

/// Codex's token endpoint and its client id, as 0.170.0 has them (strings
/// of its binary).
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// How old a kept sign-in gets before it's refreshed: a day short of the
/// eight Codex waits, so it's never older than one Codex would refresh.
const STALE: Ms = 7 * 24 * 60 * 60_000;

/// How long after a failed try a sign-in is tried again: the periodic
/// read comes round every few minutes, and a network that's down or an
/// endpoint that refuses won't be better by then.
const RETRY: Ms = 60 * 60_000;

/// When each account was last tried and failed, by id.
static FAILED_AT: LazyLock<Mutex<HashMap<String, Ms>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The refresh tokens OpenAI has refused, by hash: asking with one again
/// only gets the same answer, so it waits until you sign in again.
static REFUSED: LazyLock<Mutex<HashSet<[u8; 32]>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn err(msg: impl Into<String>) -> adapter::Error {
    msg.into().into()
}

/// Refreshes the vault's sign-in for a when it's a week old or more, and
/// no profile is signed in as it.
pub(crate) fn keep_fresh(profiles: &[Profile], a: &Account) -> adapter::Result<bool> {
    keep_fresh_in(&rush_state::vault::vault(), TOKEN_URL, profiles, a)
}

pub(crate) fn keep_fresh_in(
    v: &rush_state::vault::Keys,
    url: &str,
    profiles: &[Profile],
    a: &Account,
) -> adapter::Result<bool> {
    if FAILED_AT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&a.id)
        .is_some_and(|t| now() - t < RETRY)
    {
        return Ok(false);
    }
    // Under the vault's lock, which a switch takes too, so none can put a
    // in a profile meanwhile.
    let _lock = v.lock()?;
    if !idle(profiles, &a.id) {
        return Ok(false);
    }
    let Ok(cur) = v.get(&vault_key(&a.id)) else {
        return Ok(false);
    };
    let Some(refresh) = due(&cur, now()) else {
        return Ok(false);
    };
    if rush_core::dryrun::would("renew sign-in", &a.key, "kept unused for a week") {
        return Ok(false);
    }
    let r = refreshed(url, &cur, &refresh, &a.id).and_then(|next| {
        v.put(&vault_key(&a.id), &next)
            .map_err(|e| err(format!("couldn't keep its new sign-in: {e}")))
    });
    let mut f = rush_events::Fields::new();
    f.insert("account".into(), a.key.clone().into());
    f.insert("why".into(), "kept unused for a week".into());
    if let Err(e) = &r {
        f.insert("err".into(), e.to_string().into());
        FAILED_AT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(a.id.clone(), now());
    }
    rush_events::log("signin-renewed", f);
    r.map(|()| true)
}

/// Whether no profile is signed in as id: each says whose its sign-in
/// is, or one that can't (its sign-in in the keyring, or unreadable)
/// might be id's. One with no auth.json at all is signed in as no one.
fn idle(profiles: &[Profile], id: &str) -> bool {
    profiles.iter().all(
        |p| match std::fs::read(std::path::Path::new(&p.dir).join("auth.json")) {
            Err(e) => e.kind() == std::io::ErrorKind::NotFound && !keyring(p),
            Ok(b) => who_is(&b).is_ok_and(|w| w.id != id),
        },
    )
}

/// Whether p's config keeps its sign-in somewhere other than auth.json.
fn keyring(p: &Profile) -> bool {
    std::fs::read_to_string(std::path::Path::new(&p.dir).join("config.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| {
            t.get("cli_auth_credentials_store")
                .and_then(|v| v.as_str().map(str::to_string))
        })
        .is_some_and(|s| s != "file")
}

/// The refresh token of a ChatGPT sign-in due a refresh at t: one last
/// refreshed a week or more before, or never said when. An API key, or a
/// sign-in without a refresh token, is never due.
fn due(auth: &[u8], t: Ms) -> Option<String> {
    let f: Value = serde_json::from_slice(auth).ok()?;
    let refresh = get(&f, "tokens").map(|t| s(t, "refresh_token"))?;
    if refresh.is_empty() {
        return None;
    }
    let last = parse_rfc3339(s(&f, "last_refresh"));
    (last == 0 || t - last >= STALE).then(|| refresh.to_string())
}

/// auth with the tokens its refresh token is exchanged for at url and
/// last_refresh now, everything else as it was. The new sign-in must
/// still be id's.
fn refreshed(url: &str, auth: &[u8], refresh: &str, id: &str) -> adapter::Result<Vec<u8>> {
    let sum: [u8; 32] = Sha256::digest(refresh.as_bytes()).into();
    let refused = || REFUSED.lock().unwrap_or_else(|e| e.into_inner());
    if refused().contains(&sum) {
        return Err(err(lapsed()));
    }
    let ask = json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "scope": "openid profile email",
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let mut res = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(ask.to_string().as_bytes())
        .map_err(|_| err("refresh request failed"))?;
    let status = res.status().as_u16();
    let body = res
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_vec()
        .unwrap_or_default();
    let got: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if status == 400 || status == 401 {
        if refused_answer(&got) {
            refused().insert(sum);
            return Err(err(lapsed()));
        }
        return Err(err(format!("refresh returned {status}")));
    }
    if status != 200 {
        return Err(err(format!("refresh returned {status}")));
    }
    let id_token = s(&got, "id_token");
    if id_token.is_empty() {
        return Err(err("refresh answered without a token"));
    }
    let mut f: Value =
        serde_json::from_slice(auth).map_err(|_| err("its sign-in isn't readable"))?;
    let tokens = f
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| err("its sign-in has no tokens"))?;
    tokens.insert("id_token".into(), id_token.into());
    for k in ["access_token", "refresh_token"] {
        let t = s(&got, k);
        if !t.is_empty() {
            tokens.insert(k.into(), t.into());
        }
    }
    f["last_refresh"] = format_rfc3339(now()).into();
    let next = serde_json::to_vec_pretty(&f).map_err(|e| err(e.to_string()))?;
    if who_is(&next).map(|w| w.id).ok().as_deref() != Some(id) {
        return Err(err("its refreshed sign-in is another account's"));
    }
    Ok(next)
}

fn lapsed() -> String {
    "its sign-in has lapsed; sign in to it again".into()
}

/// Whether an error answer says the refresh token is no good: OpenAI's
/// invalid_grant, or Codex's refresh_token_expired, _reused and
/// _invalidated, as a string or an object's code.
fn refused_answer(v: &Value) -> bool {
    let e = v.get("error");
    let code = match e {
        Some(Value::String(c)) => c.as_str(),
        Some(o @ Value::Object(_)) => s(o, "code"),
        _ => "",
    };
    code == "invalid_grant" || code.starts_with("refresh_token_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A server answering every request with status and body; each
    /// request's body is sent back on the channel.
    fn server(status: u16, body: String) -> (String, std::sync::mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/oauth/token", l.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { return };
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut b = vec![0; len];
                let _ = r.read_exact(&mut b);
                let _ = tx.send(String::from_utf8_lossy(&b).into_owned());
                let _ = write!(
                    c,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (url, rx)
    }

    fn id_token(account: &str) -> String {
        let claims = json!({"email": "me@example.com", "https://api.openai.com/auth": {"chatgpt_account_id": account}});
        format!(
            "e30.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        )
    }

    fn auth(account: &str, refresh: &str, last: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "OPENAI_API_KEY": null,
            "tokens": {"id_token": id_token(account), "access_token": "old-access", "refresh_token": refresh, "account_id": account},
            "last_refresh": last,
        }))
        .unwrap()
    }

    fn account(id: &str) -> Account {
        Account {
            id: id.into(),
            key: format!("codex:{id}"),
            ..Account::default()
        }
    }

    #[test]
    fn due_once_a_week_old() {
        let t = parse_rfc3339("2026-11-19T12:00:00Z");
        for (last, refresh, want) in [
            ("2026-11-18T12:00:00.123456Z", "r", false),
            ("2026-11-12T12:00:01Z", "r", false),
            ("2026-11-12T12:00:00Z", "r", true),
            ("2026-10-01T00:00:00Z", "r", true),
            ("", "r", true),
            ("2026-10-01T00:00:00Z", "", false),
        ] {
            assert_eq!(
                due(&auth("a", refresh, last), t).is_some(),
                want,
                "{last} {refresh:?}"
            );
        }
        assert_eq!(due(br#"{"OPENAI_API_KEY":"sk-x"}"#, t), None);
        assert_eq!(due(b"not json", t), None);
    }

    #[test]
    fn refreshes_a_stale_kept_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let v = rush_state::vault::Keys::in_files(dir.path().join("vault"));
        v.put(
            "codex:fresh-1",
            &auth("fresh-1", "old-refresh", "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        let answer = json!({"id_token": id_token("fresh-1"), "access_token": "new-access", "refresh_token": "new-refresh"});
        let (url, asked) = server(200, answer.to_string());
        assert!(keep_fresh_in(&v, &url, &[], &account("fresh-1")).unwrap());
        let sent: Value = serde_json::from_str(&asked.recv().unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"client_id": CLIENT_ID, "grant_type": "refresh_token", "refresh_token": "old-refresh", "scope": "openid profile email"})
        );
        let kept: Value = serde_json::from_slice(&v.get("codex:fresh-1").unwrap()).unwrap();
        assert_eq!(kept["tokens"]["refresh_token"], "new-refresh");
        assert_eq!(kept["tokens"]["access_token"], "new-access");
        assert_eq!(kept["tokens"]["account_id"], "fresh-1");
        assert!(now() - parse_rfc3339(kept["last_refresh"].as_str().unwrap()) < 60_000);
        // Fresh now: not asked again.
        assert!(!keep_fresh_in(&v, &url, &[], &account("fresh-1")).unwrap());
    }

    #[test]
    fn a_refused_refresh_leaves_the_sign_in_and_waits() {
        let dir = tempfile::tempdir().unwrap();
        let v = rush_state::vault::Keys::in_files(dir.path().join("vault"));
        let old = auth("fresh-2", "dead-refresh", "2026-01-01T00:00:00Z");
        v.put("codex:fresh-2", &old).unwrap();
        let (url, asked) = server(
            401,
            json!({"error": {"code": "refresh_token_expired"}}).to_string(),
        );
        let e = keep_fresh_in(&v, &url, &[], &account("fresh-2")).unwrap_err();
        assert_eq!(e.to_string(), lapsed());
        assert_eq!(v.get("codex:fresh-2").unwrap(), old);
        asked.recv().unwrap();
        // Within the hour it isn't tried again.
        assert!(!keep_fresh_in(&v, &url, &[], &account("fresh-2")).unwrap());
        assert!(asked.try_recv().is_err());
    }

    #[test]
    fn refused_answers() {
        for (v, want) in [
            (json!({"error": "invalid_grant"}), true),
            (json!({"error": {"code": "refresh_token_reused"}}), true),
            (
                json!({"error": {"code": "refresh_token_invalidated"}}),
                true,
            ),
            (json!({"error": "invalid_request"}), false),
            (json!({}), false),
        ] {
            assert_eq!(refused_answer(&v), want, "{v}");
        }
    }

    // A kept sign-in is left alone while a profile runs on it, or while a
    // profile can't say whose its sign-in is; the server is never asked.
    #[test]
    fn leaves_one_in_use_alone() {
        let dir = tempfile::tempdir().unwrap();
        let v = rush_state::vault::Keys::in_files(dir.path().join("vault"));
        v.put("codex:fresh-4", &auth("fresh-4", "r", "")).unwrap();
        let home = |name: &str, files: &[(&str, &[u8])]| {
            let h = dir.path().join(name);
            std::fs::create_dir_all(&h).unwrap();
            for (f, b) in files {
                std::fs::write(h.join(f), b).unwrap();
            }
            Profile {
                dir: h.to_string_lossy().into_owned(),
                ..Profile::default()
            }
        };
        let mine = auth("fresh-4", "r", "");
        let other = auth("someone", "r", "");
        let in_use = home("in-use", &[("auth.json", &mine)]);
        let elsewhere = home("other", &[("auth.json", &other)]);
        let empty = home("empty", &[]);
        let keyring = home(
            "keyring",
            &[("config.toml", b"cli_auth_credentials_store = \"keyring\"\n")],
        );
        let bad = home("bad", &[("auth.json", b"not json")]);
        for (profiles, want) in [
            (vec![elsewhere.clone(), empty.clone()], true),
            (vec![elsewhere.clone(), in_use], false),
            (vec![keyring], false),
            (vec![bad], false),
        ] {
            assert_eq!(idle(&profiles, "fresh-4"), want, "{profiles:?}");
        }
        let (url, asked) = server(500, String::new());
        assert!(!keep_fresh_in(&v, &url, &[home("in-use", &[])], &account("fresh-4")).unwrap());
        assert!(
            asked.try_recv().is_err(),
            "the one in use is never refreshed"
        );
    }
}
