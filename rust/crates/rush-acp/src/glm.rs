//! Z.ai's GLM models through ZCode, Z.ai's own coding agent, as a rush
//! adapter. ZCode doesn't speak ACP itself: its machine interface is its
//! own protocol (zcode app-server --stdio). rush runs it through
//! zcode-acp-server (npm zcode-acp), which drives that same ZCode runtime
//! and speaks ACP, and reads the GLM Coding Plan's limits as ZCode does.
//! It stays out of sight until both are installed. Port of
//! internal/adapters/glm/glm.go:1-99 and quota.go:1-190.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rush_core::adapter::{
    self, Account, Adapter, Conn, Driver, Feature, Features, Keyed, Kind, Level, Made, Profile,
    Programmer, QuotaSource, Sink, StartOptions, Support,
};
use rush_core::time::{Ms, now};
use rush_core::usage::{Quota, Source, Window};
use serde::Deserialize;

use crate::agent::{Agent, installed};
use crate::rpc::Error;
use crate::util::{env, home_dir, path_str};

/// GLM's kind.
pub const KIND: &str = "glm";

/// The Coding Plan's limits, as ZCode itself reads them: api.z.ai for
/// Z.ai's keys, open.bigmodel.cn for BigModel's. Asking spends nothing.
pub const HOST_INTL: &str = "https://api.z.ai";
pub const HOST_CN: &str = "https://open.bigmodel.cn";
pub const QUOTA_PATH: &str = "/api/monitor/usage/quota/limit";

/// The CLI inside ZCode's desktop app, which the bridge finds on its own.
pub const APP_BUNDLE: &str = "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs";

/// GLM through ZCode.
pub struct Glm {
    /// ZCode over ACP, through the bridge.
    cli: Agent,
    host_intl: String,
    host_cn: String,
    app_bundle: String,
}

impl Default for Glm {
    fn default() -> Self {
        Glm::new()
    }
}

impl Glm {
    pub fn new() -> Glm {
        Glm {
            cli: Agent {
                id: Kind::from(KIND),
                title: "GLM".into(),
                command: "zcode-acp-server".into(),
                more: HashMap::from([(Feature::Quota, Support::YES.with("the GLM Coding Plan's"))]),
                ..Agent::default()
            },
            host_intl: HOST_INTL.into(),
            host_cn: HOST_CN.into(),
            app_bundle: APP_BUNDLE.into(),
        }
    }

    /// A GLM that reads limits from these hosts, and looks for ZCode's app
    /// at app: for tests.
    pub fn at(host_intl: &str, host_cn: &str, app: &str) -> Glm {
        Glm {
            host_intl: host_intl.into(),
            host_cn: host_cn.into(),
            app_bundle: app.into(),
            ..Glm::new()
        }
    }

    /// Whether the bridge has a ZCode to run, where it looks for one:
    /// ZCODE_BIN, zcode (its installer puts it in ~/.local/bin), or the
    /// desktop app.
    fn has_zcode(&self) -> bool {
        let p = env("ZCODE_BIN");
        if !p.is_empty() {
            return std::fs::metadata(p).is_ok();
        }
        if adapter::find("zcode", &[]).is_some() {
            return true;
        }
        std::fs::metadata(&self.app_bundle).is_ok()
    }
}

/// ZCode's data folder: ZCODE_HOME, else ~/.zcode.
fn home() -> String {
    let d = env("ZCODE_HOME");
    if !d.is_empty() {
        return d;
    }
    let h = home_dir();
    if h.is_empty() {
        return String::new();
    }
    path_str(&Path::new(&h).join(".zcode"))
}

impl Adapter for Glm {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "ZCode".into()
    }
    /// The bridge's over ACP (ZCode's modes: plan, build, edit, yolo,
    /// auto), and the Coding Plan's limits. It's been tried only against
    /// recorded fixtures.
    fn features(&self) -> Features {
        self.cli.all_features()
    }
    fn level(&self) -> Level {
        self.cli.tried
    }
    /// ZCODE_HOME, or ~/.zcode, when the bridge is installed and has a
    /// ZCode to drive.
    fn profiles(&self) -> Vec<Profile> {
        if !installed(&self.cli.id) || !self.has_zcode() {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(KIND),
            name: "GLM".into(),
            dir: home(),
        }]
    }
    rush_core::has!(
        driver: Driver,
        programmer: Programmer,
        made: Made,
        keyed: Keyed,
        quota_source: QuotaSource,
    );
}

impl Made for Glm {
    fn maker(&self) -> String {
        "GLM".into()
    }
}

impl Keyed for Glm {
    /// Where Z.ai's API key is read from.
    fn key_env(&self) -> String {
        "ZAI_API_KEY".into()
    }
}

impl Programmer for Glm {
    /// The ACP bridge, what rush runs.
    fn program(&self) -> (String, Vec<String>) {
        ("zcode-acp-server".into(), Vec::new())
    }
}

impl Driver for Glm {
    /// Runs ZCode over ACP through the bridge, in the profile's home.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        if !o.profile.dir.is_empty() && o.profile.dir != home() {
            o.env.push(format!("ZCODE_HOME={}", o.profile.dir));
        }
        Ok(Box::new(self.cli.run(o, sink, false)?))
    }
}

/// One of ZCode's model providers in v2/config.json.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Provider {
    enabled: bool,
    options: ProviderOptions,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProviderOptions {
    #[serde(rename = "apiKey")]
    api_key: String,
    #[serde(rename = "baseURL")]
    base_url: String,
}

/// The key and base URL of the provider ZCode uses: the one ZCODE_PROVIDER
/// pins, else the first enabled one with a key.
fn credentials(dir: &str) -> (String, String) {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Cfg {
        provider: HashMap<String, Provider>,
    }
    let Some(cfg) = std::fs::read(Path::new(dir).join("v2").join("config.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Cfg>(&b).ok())
    else {
        return (String::new(), String::new());
    };
    let mut ids: Vec<&String> = cfg.provider.keys().collect();
    ids.sort();
    let pinned = env("ZCODE_PROVIDER");
    for id in ids {
        if !pinned.is_empty() && *id != pinned {
            continue;
        }
        let p = &cfg.provider[id];
        if p.enabled && !p.options.api_key.is_empty() {
            return (p.options.api_key.clone(), p.options.base_url.clone());
        }
    }
    (String::new(), String::new())
}

/// The quota endpoint's envelope.
#[derive(Deserialize, Default)]
#[serde(default)]
struct QuotaResponse {
    code: i64,
    msg: String,
    success: bool,
    data: QuotaData,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QuotaData {
    level: String,
    limits: Vec<Limit>,
}

/// One of the plan's limits. Some count (usage is the allowance,
/// currentValue what's used), some only say a percentage used.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Limit {
    /// TOKENS_LIMIT, TIME_LIMIT (MCP calls)
    #[serde(rename = "type")]
    kind: String,
    number: Option<i64>,
    usage: Option<f64>,
    #[serde(rename = "currentValue")]
    current_value: Option<f64>,
    remaining: Option<f64>,
    percentage: Option<f64>,
    /// ms
    #[serde(rename = "nextResetTime")]
    next_reset_time: Option<i64>,
}

fn no_key() -> Error {
    Error::other("no GLM API key in ZCode: sign in to ZCode with a Coding Plan key")
}

impl QuotaSource for Glm {
    /// The Coding Plan's limits for the key ZCode is signed in with.
    fn quota(&self, p: &Profile, _: &Account) -> adapter::Result<Quota> {
        let dir = if p.dir.is_empty() {
            home()
        } else {
            p.dir.clone()
        };
        let (key, base) = credentials(&dir);
        if key.is_empty() {
            return Err(Box::new(no_key()));
        }
        let host = if base.contains("z.ai") {
            &self.host_intl
        } else {
            &self.host_cn
        };
        let res = crate::http::get(&format!("{host}{QUOTA_PATH}"), &key).map_err(Error::other)?;
        if res.code == 429 {
            return Err(Box::new(Error::other("GLM quota: rate-limited")));
        }
        let r: QuotaResponse = serde_json::from_slice(&res.body)
            .map_err(|_| Error::other(format!("GLM quota: {}", res.status)))?;
        if !r.success {
            if r.code == 401 || r.code == 1001 {
                return Err(Box::new(Error::other("GLM refused ZCode's API key")));
            }
            return Err(Box::new(Error::other(format!("GLM quota: {}", r.msg))));
        }
        Ok(quota_of(&r, now()))
    }
}

/// The plan's limits as rush's windows.
fn quota_of(r: &QuotaResponse, now: Ms) -> Quota {
    Quota {
        plan: r.data.level.clone(),
        fetched_at: now,
        source: Source::Fetched,
        windows: r.data.limits.iter().filter_map(window_of).collect(),
        ..Quota::default()
    }
}

fn window_of(l: &Limit) -> Option<Window> {
    let w = |id: &str, label: &str, name: &str, hours: u64| Window {
        id: id.into(),
        label: label.into(),
        name: name.into(),
        span: Duration::from_secs(hours * 3600),
        ..Window::default()
    };
    let mut win = match (l.kind.as_str(), l.number) {
        ("TOKENS_LIMIT", Some(5)) => w("token_5h", "5h", "5-hour", 5),
        ("TOKENS_LIMIT", Some(7)) => w("token_week", "7d", "weekly", 7 * 24),
        ("TOKENS_LIMIT", n) => {
            let mut x = w("token", "tokens", "tokens", 0);
            if let Some(n) = n {
                x.id = format!("token_{n}");
            }
            x
        }
        ("TIME_LIMIT" | "MCP_LIMIT", _) => w("mcp", "MCP", "MCP calls", 30 * 24),
        (t, _) => w(&t.to_lowercase(), t, t, 0),
    };
    // The most precise figure there is: what's left of the parts, what's
    // used of the allowance, then the percentage used.
    let total = match (l.remaining, l.current_value, l.usage) {
        (Some(r), Some(c), _) => r + c,
        (_, _, Some(u)) => u,
        _ => 0.0,
    };
    if total > 0.0 && l.current_value.is_some() {
        let c = l.current_value.unwrap_or_default();
        win.used = c;
        win.limit = total;
        win.percent = 100.0 * c / total;
    } else if total > 0.0 && l.remaining.is_some() {
        let r = l.remaining.unwrap_or_default();
        win.used = total - r;
        win.limit = total;
        win.percent = 100.0 * (total - r) / total;
    } else {
        win.percent = l.percentage?;
    }
    win.percent = win.percent.clamp(0.0, 100.0);
    if let Some(t) = l.next_reset_time
        && t > 0
    {
        win.resets_at = t;
    }
    Some(win)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake;
    use std::sync::Mutex;

    const CONFIG: &str = r#"{"provider":{"anthropic-custom":{"enabled":false,"options":{"apiKey":"sk-disabled","baseURL":"https://example.com"}},"bigmodel":{"enabled":true,"options":{"apiKey":"","baseURL":"https://open.bigmodel.cn/api/anthropic"}},"zai":{"enabled":true,"options":{"apiKey":"zai-key.abc","baseURL":"https://api.z.ai/api/anthropic"},"models":{"glm-5.3":{}}}}}"#;
    const QUOTA: &str = r#"{"code":200,"msg":"操作成功","data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":42,"nextResetTime":1790524800000,"usageDetails":[]},{"type":"TOKENS_LIMIT","unit":6,"number":7,"percentage":11,"nextResetTime":1790956800000,"usageDetails":[]},{"type":"TIME_LIMIT","unit":5,"number":1,"usage":1000,"currentValue":250,"remaining":750,"percentage":25,"nextResetTime":1792080000000,"usageDetails":[{"modelCode":"search-prime","usage":200},{"modelCode":"web-reader","usage":50}]}],"level":"pro"},"success":true}"#;
    const REFUSED: &str = r#"{"code":1001,"msg":"Authorization Token非法","success":false}"#;

    // glm_test.go: TestQuota
    #[test]
    fn quota() {
        if !env("ZCODE_PROVIDER").is_empty() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("v2")).unwrap();
        std::fs::write(d.path().join("v2/config.json"), CONFIG).unwrap();
        let seen = Arc::new(Mutex::new((String::new(), String::new())));
        let answer = Arc::new(Mutex::new(QUOTA));
        let (s2, a2) = (seen.clone(), answer.clone());
        let srv = fake::serve(move |r| {
            *s2.lock().unwrap() = (r.auth.clone(), r.path.clone());
            (200, a2.lock().unwrap().to_string())
        });
        let g = Glm::at(&srv.url, "http://cn.invalid", "/nowhere");
        let p = Profile {
            kind: Kind::from(KIND),
            dir: path_str(d.path()),
            ..Profile::default()
        };
        let q = g.quota(&p, &Account::default()).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            ("Bearer zai-key.abc".to_string(), QUOTA_PATH.to_string())
        );
        assert_eq!((q.plan.as_str(), q.windows.len()), ("pro", 3));
        let five = q.window("token_5h").unwrap();
        let week = q.window("token_week").unwrap();
        let mcp = q.window("mcp").unwrap();
        assert_eq!(
            (five.percent, five.label.as_str(), five.resets_at),
            (42.0, "5h", 1790524800000)
        );
        assert_eq!((week.percent, week.label.as_str()), (11.0, "7d"));
        assert_eq!((mcp.used, mcp.limit, mcp.percent), (250.0, 1000.0, 25.0));
        assert_eq!(q.tightest("").unwrap().id, "token_5h");

        *answer.lock().unwrap() = REFUSED;
        let err = g.quota(&p, &Account::default()).unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
        let empty = tempfile::tempdir().unwrap();
        let err = g
            .quota(
                &Profile {
                    dir: path_str(empty.path()),
                    ..Profile::default()
                },
                &Account::default(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), no_key().to_string());
    }

    // glm_test.go: TestDormantUntilInstalled, through a registry with a
    // PATH of its own: the bridge is what's looked for.
    #[test]
    fn installed_once_the_bridge_is() {
        use std::os::unix::fs::PermissionsExt;
        let (bin, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let reg =
            rush_core::adapter::Registry::with_env(&path_str(bin.path()), &path_str(home.path()));
        reg.register(Glm::at(HOST_INTL, HOST_CN, "/nowhere"));
        assert_eq!(reg.label(KIND), "GLM (ZCode)");
        assert!(!reg.installed(KIND), "GLM shows without the bridge");
        let p = bin.path().join("zcode-acp-server");
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        reg.recheck();
        assert!(
            reg.installed(KIND),
            "the bridge is there but GLM isn't installed"
        );
    }

    #[test]
    fn names_itself_zcode_by_glm() {
        let g = Glm::new();
        assert_eq!((g.name().as_str(), g.maker().as_str()), ("ZCode", "GLM"));
        assert_eq!(g.key_env(), "ZAI_API_KEY");
        assert!(g.features().contains_key(&Feature::Quota));
    }
}
