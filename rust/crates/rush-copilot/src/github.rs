//! Reaching GitHub for Copilot: the token (the environment's, else gh's
//! for the account rush runs Copilot on), what GitHub says of your
//! Copilot, where its agents API is, and repositories' names.
//!
//! Ported from Go's internal/adapters/copilot/github.go:1-199.

use std::collections::HashMap;
use std::io::Read;
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use rush_core::adapter::Result;

use crate::{Inner, err, lock};

/// How long gh's token is kept before it's asked for again.
const TOKEN_FOR: Duration = Duration::from_secs(10 * 60);

/// What GitHub says about your Copilot: your plan, your limits, and where
/// its API is.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct User {
    #[serde(rename = "copilot_plan")]
    pub plan: String,
    #[serde(rename = "quota_reset_date_utc")]
    pub resets_at: String,
    pub endpoints: Endpoints,
    #[serde(rename = "quota_snapshots")]
    pub quotas: HashMap<String, Snapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Endpoints {
    pub api: String,
}

/// One of your Copilot's limits.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub entitlement: f64,
    pub remaining: f64,
    #[serde(rename = "percent_remaining")]
    pub percent: f64,
    pub unlimited: bool,
    #[serde(rename = "overage_permitted")]
    pub overage: bool,
}

/// What GitHub's API is asked of: api.github.com, or a test's own.
pub const GITHUB: &str = "https://api.github.com";

impl Inner {
    /// The GitHub token Copilot is reached with: the one in the
    /// environment, else gh's for the account rush uses for Copilot.
    pub(crate) fn token(&self) -> Result<String> {
        for k in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
            if let Some(v) = (self.getenv)(k) {
                return Ok(v);
            }
        }
        self.token_for(&self.chosen())
    }

    /// gh's token for login (its active account when empty), asked for at
    /// most every ten minutes.
    pub(crate) fn token_for(&self, login: &str) -> Result<String> {
        let mut tokens = lock(&self.tokens);
        if let Some((t, at)) = tokens.get(login)
            && at.elapsed() < TOKEN_FOR
        {
            return Ok(t.clone());
        }
        let mut args = vec!["auth", "token", "-h", "github.com"];
        if !login.is_empty() {
            args.extend(["-u", login]);
        }
        let out = Command::new(self.gh()).args(&args).output();
        let tok = match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ if !login.is_empty() => {
                return Err(err(format!(
                    "copilot: gh isn't signed in as {login} (gh auth login)"
                )));
            }
            _ => return Err(err("copilot: not signed in to GitHub (gh auth login)")),
        };
        tokens.insert(login.to_string(), (tok.clone(), Instant::now()));
        Ok(tok)
    }

    /// Drops the tokens asked for, so the next ask is fresh.
    pub(crate) fn forget_tokens(&self) {
        lock(&self.tokens).clear();
    }

    /// gh, wherever it's installed.
    pub(crate) fn gh(&self) -> String {
        if let Some(g) = &self.gh {
            return g.clone();
        }
        rush_core::adapter::find("gh", &[]).unwrap_or_else(|| "gh".into())
    }

    /// url's JSON, with the GitHub token.
    pub(crate) fn get<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let tok = self.token()?;
        self.get_with(url, &tok)
    }

    /// url's JSON, with tok.
    pub(crate) fn get_with<T: DeserializeOwned>(&self, url: &str, tok: &str) -> Result<T> {
        let mut b = Vec::new();
        self.fetch_with(url, tok, Duration::from_secs(20))?
            .take(16 << 20)
            .read_to_end(&mut b)?;
        Ok(serde_json::from_slice(&b)?)
    }

    /// url's body, with the GitHub token.
    pub(crate) fn fetch(&self, url: &str, timeout: Duration) -> Result<Box<dyn Read + Send>> {
        let tok = self.token()?;
        self.fetch_with(url, &tok, timeout)
    }

    /// url's body, with tok.
    pub(crate) fn fetch_with(
        &self,
        url: &str,
        tok: &str,
        timeout: Duration,
    ) -> Result<Box<dyn Read + Send>> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        let res = agent
            .get(url)
            .header("Authorization", &format!("Bearer {tok}"))
            .header("Accept", "application/json")
            .header("Copilot-Integration-Id", "copilot-4-cli")
            .header("User-Agent", "rush")
            .call()?;
        if res.status().as_u16() != 200 {
            let path = url
                .split_once("://")
                .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
                .unwrap_or("/");
            let path = path.split('?').next().unwrap_or(path);
            return Err(err(format!(
                "copilot: {path} answered {}",
                res.status().as_u16()
            )));
        }
        Ok(Box::new(res.into_body().into_reader()))
    }

    /// Asks GitHub about your Copilot.
    pub(crate) fn read_user(&self) -> Result<User> {
        self.get(&format!("{}/copilot_internal/user", self.github))
    }

    /// Asks GitHub about login's Copilot.
    pub(crate) fn read_user_as(&self, login: &str) -> Result<User> {
        let tok = self.token_for(login)?;
        self.get_with(&format!("{}/copilot_internal/user", self.github), &tok)
    }

    /// Where your Copilot's agents API is, remembered once known.
    pub(crate) fn api_base(&self) -> Result<String> {
        let mut base = lock(&self.base);
        if !base.is_empty() {
            return Ok(base.clone());
        }
        let u = self.read_user()?;
        if u.endpoints.api.is_empty() {
            return Err(err("copilot: GitHub gave no API for your Copilot"));
        }
        *base = u.endpoints.api.trim_end_matches('/').to_string();
        Ok(base.clone())
    }

    /// A repository's owner/name, by its id.
    pub(crate) fn repo_name(&self, id: i64) -> String {
        if let Some(n) = lock(&self.repos).get(&id) {
            return n.clone();
        }
        #[derive(Deserialize)]
        struct Repo {
            #[serde(default)]
            full_name: String,
        }
        let Ok(r) = self.get::<Repo>(&format!("{}/repositories/{id}", self.github)) else {
            return String::new();
        };
        lock(&self.repos).insert(id, r.full_name.clone());
        r.full_name
    }
}
