//! DeepSeek's own coding agent, DeepSeek Harness (dsh, npm
//! @deepseek-ai/dsh), as a rush adapter. rush runs it over ACP with its
//! shipped "acp" profile (dsh --profile acp), and reads the balance of the
//! DeepSeek API key it uses. It stays out of sight until dsh is installed.
//!
//! Ported from Go's internal/adapters/deepseek/deepseek.go:1-88 and
//! balance.go:1-179. A session runs through rush-acp's [`Agent`] (Go's
//! `cli`): [`COMMAND`] with [`ARGS`], readied by [`DeepSeek::acp_options`].

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use rush_acp::Agent;
use rush_core::adapter::{
    Account, Adapter, Conn, Driver, Feature, Features, Keyed, Kind, Level, Made, Profile,
    Programmer, QuotaSource, Registry, Result, Sink, StartOptions, Support,
};
use rush_core::usage::{Quota, Source};

use crate::{Getenv, process_env};

/// DeepSeek's kind.
pub const KIND: &str = "deepseek";

/// dsh, which npm puts on PATH.
pub const COMMAND: &str = "dsh";

/// What makes dsh speak ACP: its acp profile, which sets itself up on
/// first use. SDK and ACP are dsh profiles rather than programs of their
/// own.
pub const ARGS: [&str; 2] = ["--profile", "acp"];

/// dsh's home under the home folder.
pub const HOME: &str = ".dsh";

/// DeepSeek's balance endpoint. Asking it spends nothing.
pub const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

/// A dsh with no DeepSeek API key rush can find.
pub const NO_KEY: &str = "no DeepSeek API key: set one with dsh, or DEEPSEEK_API_KEY";

/// DeepSeek Harness.
#[derive(Clone)]
pub struct DeepSeek {
    reg: Registry,
    /// dsh over ACP (Go's cli).
    cli: Agent,
    getenv: Getenv,
    balance_url: String,
}

impl DeepSeek {
    /// dsh as rush runs it, knowing what's installed from reg.
    pub fn new(reg: Registry) -> DeepSeek {
        DeepSeek {
            reg,
            cli: cli(),
            getenv: process_env(),
            balance_url: BALANCE_URL.to_string(),
        }
    }

    /// With the environment read through getenv, and the balance asked of
    /// url: for tests.
    pub fn with(reg: Registry, getenv: Getenv, url: &str) -> DeepSeek {
        DeepSeek {
            reg,
            cli: cli(),
            getenv,
            balance_url: url.to_string(),
        }
    }

    /// dsh's home: DSH_HOME, else ~/.dsh.
    pub fn home(&self) -> String {
        if let Some(d) = (self.getenv)("DSH_HOME") {
            return d;
        }
        crate::home_dir(&self.getenv)
            .map(|h| Path::new(&h).join(HOME).to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// o as dsh starts over ACP, in the profile's home (Go's Start). dsh
    /// has no modes, so a mode asked for is left alone rather than failing
    /// the start.
    pub fn acp_options(&self, mut o: StartOptions) -> StartOptions {
        o.mode.clear();
        if !o.profile.dir.is_empty() && o.profile.dir != self.home() {
            o.env.push(format!("DSH_HOME={}", o.profile.dir));
        }
        o
    }
}

/// dsh over ACP (Go's cli).
fn cli() -> Agent {
    Agent {
        id: Kind::from(KIND),
        title: "DeepSeek".into(),
        command: COMMAND.into(),
        args: ARGS.iter().map(|a| a.to_string()).collect(),
        home: HOME.into(),
        ..Agent::default()
    }
}

/// dsh's features over ACP: it resumes without replaying, has no modes and
/// asks no questions. Its money left is read, not a window.
fn features() -> Features {
    use Feature::*;
    Features::from([
        (Run, Support::YES),
        (
            Resume,
            Support::YES.with("without replaying the conversation"),
        ),
        (Interrupt, Support::YES),
        (Model, Support::YES.with("when dsh offers models")),
        (Images, Support::YES.with("on models that take them")),
        (Mcp, Support::YES),
        (HandoffIn, Support::YES),
        (Quota, Support::YES.with("the API key's balance")),
        (Background, Support::NO.with("ACP has no background tasks")),
        (
            History,
            Support::NO.with("dsh's session logs are compressed and still change"),
        ),
    ])
}

impl Adapter for DeepSeek {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "dsh".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Preview
    }
    /// DSH_HOME, or ~/.dsh, when dsh is installed.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg.installed(KIND) {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(KIND),
            name: "DeepSeek".into(),
            dir: self.home(),
        }]
    }
    rush_core::has!(
        driver: Driver,
        made: Made,
        keyed: Keyed,
        programmer: Programmer,
        quota_source: QuotaSource,
    );
}

impl Made for DeepSeek {
    fn maker(&self) -> String {
        "DeepSeek".into()
    }
}

impl Keyed for DeepSeek {
    /// Where DeepSeek's API key is read from.
    fn key_env(&self) -> String {
        "DEEPSEEK_API_KEY".into()
    }
}

impl Programmer for DeepSeek {
    /// dsh, which npm puts on PATH.
    fn program(&self) -> (String, Vec<String>) {
        (COMMAND.into(), vec![format!("{HOME}/bin")])
    }
}

impl Driver for DeepSeek {
    /// Runs dsh over ACP in the profile's home.
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        let o = self.acp_options(o);
        Ok(Box::new(self.cli.run(o, sink, false)?))
    }
}

/// /user/balance's answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Balance {
    is_available: bool,
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
}

fn err(s: impl Into<String>) -> rush_core::adapter::Error {
    s.into().into()
}

impl QuotaSource for DeepSeek {
    /// What's left on the DeepSeek API key dsh uses. DeepSeek bills as you
    /// go, so there are no windows, only the balance.
    fn quota(&self, p: &Profile, _a: &Account) -> Result<Quota> {
        let dir = if p.dir.is_empty() {
            self.home()
        } else {
            p.dir.clone()
        };
        let key = api_key(Path::new(&dir), &self.getenv);
        if key.is_empty() {
            return Err(err(NO_KEY));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut res = agent
            .get(&self.balance_url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Accept", "application/json")
            .call()?;
        let body = res
            .body_mut()
            .with_config()
            .limit(1 << 20)
            .read_to_vec()
            .unwrap_or_default();
        match res.status().as_u16() {
            401 => return Err(err("DeepSeek refused the API key")),
            200 => {}
            _ => return Err(err(format!("DeepSeek balance: {}", status(res.status())))),
        }
        let b: Balance =
            crate::gojson::from_slice(&body).map_err(|e| err(format!("DeepSeek balance: {e}")))?;
        Ok(quota_of(&b, rush_core::time::now()))
    }
}

/// A status as Go's resp.Status says it: "502 Bad Gateway".
fn status(s: ureq::http::StatusCode) -> String {
    match s.canonical_reason() {
        Some(r) => format!("{} {r}", s.as_u16()),
        None => s.as_u16().to_string(),
    }
}

/// A balance as rush's reading: "¥110.00 left", or why it can't be used.
pub(crate) fn quota_of(b: &Balance, now: rush_core::time::Ms) -> Quota {
    let mut q = Quota {
        fetched_at: now,
        source: Source::Fetched,
        ..Quota::default()
    };
    let parts: Vec<String> = b
        .balance_infos
        .iter()
        .map(|i| money(&i.currency, &i.total_balance))
        .collect();
    if !parts.is_empty() {
        q.balance = format!("{} left", parts.join(" + "));
    }
    if !b.is_available {
        q.problem = "DeepSeek balance used up: top up to keep going".into();
    }
    q
}

/// An amount with its currency's sign.
fn money(currency: &str, amount: &str) -> String {
    let amount = match amount.parse::<f64>() {
        Ok(f) => format!("{f:.2}"),
        Err(_) => amount.to_string(),
    };
    match currency.to_uppercase().as_str() {
        "CNY" => format!("¥{amount}"),
        "USD" => format!("${amount}"),
        _ => format!("{amount} {currency}"),
    }
}

/// The DeepSeek key dsh would use, in dsh's own order: the environment,
/// its credential store ($DSH_HOME/.credentials.yaml), then $DSH_HOME/.env.
pub fn api_key(dir: &Path, getenv: &Getenv) -> String {
    if let Some(k) = getenv("DEEPSEEK_API_KEY") {
        return k;
    }
    let k = credentials_key(&dir.join(".credentials.yaml"));
    if !k.is_empty() {
        return k;
    }
    dotenv_key(&dir.join(".env"))
}

/// refs.DEEPSEEK_API_KEY from dsh's credential file:
///
/// ```yaml
/// version: 1
/// refs:
///   DEEPSEEK_API_KEY: sk-…
/// ```
///
/// It reads just that, line by line, rather than all of YAML.
fn credentials_key(path: &Path) -> String {
    let Ok(f) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut in_refs = false;
    for line in BufReader::new(f).lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_refs = trimmed.starts_with("refs:");
            continue;
        }
        if in_refs && let Some(v) = trimmed.strip_prefix("DEEPSEEK_API_KEY:") {
            return unquote(v);
        }
    }
    String::new()
}

/// DEEPSEEK_API_KEY from a .env file.
fn dotenv_key(path: &Path) -> String {
    let Ok(b) = std::fs::read_to_string(path) else {
        return String::new();
    };
    for line in b.split('\n') {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some(v) = line.strip_prefix("DEEPSEEK_API_KEY=") {
            return unquote(v);
        }
    }
    String::new()
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') {
        return match v[1..].find(q) {
            Some(end) => v[1..1 + end].to_string(),
            None => v.to_string(),
        };
    }
    match v.find(" #") {
        Some(i) => v[..i].trim().to_string(),
        None => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn env(vars: &[(&str, &str)]) -> Getenv {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Arc::new(move |k| m.get(k).filter(|v| !v.is_empty()).cloned())
    }

    fn testdata(name: &str) -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("testdata/deepseek")
                .join(name),
        )
        .unwrap()
    }

    /// Answers each request with the next body, keeping each request's
    /// Authorization header.
    fn serve(bodies: Vec<Vec<u8>>) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/user/balance", l.local_addr().unwrap());
        let auth = Arc::new(Mutex::new(Vec::new()));
        let seen = auth.clone();
        std::thread::spawn(move || {
            for body in bodies {
                let (mut s, _) = l.accept().unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                }
                let text = String::from_utf8_lossy(&req).into_owned();
                let a = text
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("authorization")
                            .then(|| v.trim().to_string())
                    })
                    .unwrap_or_default();
                seen.lock().unwrap().push(a);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                s.write_all(head.as_bytes()).unwrap();
                s.write_all(&body).unwrap();
            }
        });
        (url, auth)
    }

    // DeepSeek is nowhere until dsh is installed, and there once it is.
    #[test]
    fn dormant_until_installed() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let reg = Registry::with_env(bin.path().to_str().unwrap(), home.path().to_str().unwrap());
        let getenv = env(&[("HOME", home.path().to_str().unwrap())]);
        reg.register(DeepSeek::with(reg.clone(), getenv.clone(), BALANCE_URL));
        let a = reg.get(KIND).expect("DeepSeek isn't registered");
        assert_eq!(reg.label(KIND), "DeepSeek (dsh)");
        if !reg.path(KIND).is_empty() {
            return; // dsh is installed where every machine looks
        }
        assert!(
            !reg.installed(KIND) && a.profiles().is_empty(),
            "DeepSeek shows without dsh"
        );
        assert!(
            reg.installed_all()
                .iter()
                .all(|a| a.kind().as_str() != KIND)
        );

        let dsh = bin.path().join("dsh");
        std::fs::write(&dsh, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dsh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        reg.recheck();
        assert!(
            reg.installed(KIND),
            "dsh is on PATH but DeepSeek isn't installed"
        );
        let ps = a.profiles();
        assert_eq!(ps.len(), 1);
        assert_eq!(Path::new(&ps[0].dir), home.path().join(".dsh"));
    }

    // A start over ACP leaves a mode alone, and keeps a profile's home.
    #[test]
    fn acp_options() {
        let d = DeepSeek::with(Registry::new(), env(&[("HOME", "/h")]), BALANCE_URL);
        let o = d.acp_options(StartOptions {
            mode: "default".into(),
            model: "deepseek-official/deepseek-v4-pro".into(),
            ..StartOptions::default()
        });
        assert!(o.mode.is_empty() && o.env.is_empty());
        assert_eq!(o.model, "deepseek-official/deepseek-v4-pro");
        let o = d.acp_options(StartOptions {
            profile: Profile {
                dir: "/elsewhere".into(),
                ..Profile::default()
            },
            ..StartOptions::default()
        });
        assert_eq!(o.env, ["DSH_HOME=/elsewhere"]);
        assert_eq!(d.home(), "/h/.dsh");
    }

    /// A program at dir/name that checks it was run with args, then answers
    /// each request it's sent with the next line of fixture, keeping the
    /// requests in log (Go's fakeProgram).
    #[cfg(unix)]
    fn fake_program(dir: &Path, name: &str, args: &str, fixture: &Path, log: &Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let script = format!(
            r#"#!/bin/sh
[ "$*" = "{args}" ] || {{ echo "run with: $*" >&2; exit 2; }}
while IFS= read -r resp <&3; do
	IFS= read -r req || exit 0
	printf '%s\n' "$req" >> '{}'
	printf '%s\n' "$resp"
done 3< '{}'
cat > /dev/null
"#,
            log.display(),
            fixture.display()
        );
        let p = dir.join(name);
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// A sink that hands each Init on.
    struct Inits(Mutex<std::sync::mpsc::Sender<rush_core::event::Init>>);

    impl rush_core::adapter::Sink for Inits {
        fn event(&self, ev: rush_core::event::Event) {
            if let rush_core::event::Event::Init(i) = ev {
                let _ = self.0.lock().unwrap().send(*i);
            }
        }
        fn ended(&self, _: Option<rush_core::adapter::Error>) {}
    }

    // deepseek_test.go: TestStart: a session starts over ACP with dsh's acp
    // profile, in the model asked for; a mode, which dsh hasn't, is left
    // alone.
    #[cfg(unix)]
    #[test]
    fn start() {
        let bin = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let log = bin.path().join("requests");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/deepseek/dsh-acp.jsonl");
        let dsh = fake_program(bin.path(), "dsh", "--profile acp", &fixture, &log);
        let (tx, rx) = std::sync::mpsc::channel();
        let d = DeepSeek::with(Registry::new(), env(&[("HOME", "/h")]), BALANCE_URL);
        let conn = d
            .start(
                StartOptions {
                    binary: dsh,
                    dir: work.path().to_string_lossy().into_owned(),
                    model: "deepseek-official/deepseek-v4-pro".into(),
                    mode: "default".into(),
                    ..StartOptions::default()
                },
                Arc::new(Inits(Mutex::new(tx))),
            )
            .unwrap();
        let mut last = rush_core::event::Init::default();
        while last.model != "deepseek-official/deepseek-v4-pro" {
            last = rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("no Init in the model asked for; last {last:?}"));
        }
        assert_eq!(
            (last.session_id.as_str(), last.version.as_str()),
            ("ses_01JQ7ZK4M3", "0.1.7-rc.2")
        );
        let reqs = std::fs::read_to_string(&log).unwrap();
        let reqs: Vec<&str> = reqs.trim().split('\n').collect();
        assert_eq!(reqs.len(), 3, "{reqs:?}");
        assert!(reqs[1].contains(r#""session/new""#), "{reqs:?}");
        assert!(
            reqs[2].contains(r#""session/set_config_option""#)
                && reqs[2].contains(r#""configId":"model""#),
            "{reqs:?}"
        );
        conn.close().unwrap();
    }

    // What Go's dsh says it does over ACP.
    #[test]
    fn features_over_acp() {
        use rush_core::adapter::Has;
        let f = DeepSeek::new(Registry::new()).features();
        for k in [
            Feature::Run,
            Feature::Resume,
            Feature::Interrupt,
            Feature::Mcp,
        ] {
            assert_eq!(f[&k].is, Has::Yes, "{k:?}");
        }
        assert_eq!(f[&Feature::Background].is, Has::No);
        assert!(!f.contains_key(&Feature::Modes), "dsh has no modes");
    }

    // The balance is read with the key dsh keeps, and says when it's used up.
    #[test]
    fn quota() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".credentials.yaml"),
            testdata("credentials.yaml"),
        )
        .unwrap();
        let (url, auth) = serve(vec![
            testdata("balance.json"),
            testdata("balance-empty.json"),
        ]);
        let p = Profile {
            kind: Kind::from(KIND),
            dir: dir.path().to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let d = DeepSeek::with(Registry::new(), env(&[]), &url);
        let q = d.quota(&p, &Account::default()).unwrap();
        assert_eq!(auth.lock().unwrap()[0], "Bearer sk-from-store");
        assert_eq!(q.balance, "¥110.00 left");
        assert!(q.problem.is_empty() && q.windows.is_empty());

        let d = DeepSeek::with(
            Registry::new(),
            env(&[("DEEPSEEK_API_KEY", "sk-from-env")]),
            &url,
        );
        let q = d.quota(&p, &Account::default()).unwrap();
        assert_eq!(auth.lock().unwrap()[1], "Bearer sk-from-env");
        assert_eq!(q.balance, "$0.00 left");
        assert!(!q.problem.is_empty());

        let empty = tempfile::tempdir().unwrap();
        let d = DeepSeek::with(Registry::new(), env(&[]), &url);
        let p = Profile {
            dir: empty.path().to_string_lossy().into_owned(),
            ..Profile::default()
        };
        let e = d.quota(&p, &Account::default()).unwrap_err();
        assert_eq!(e.to_string(), NO_KEY);
    }

    #[test]
    fn dotenv_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "OTHER=1\nexport DEEPSEEK_API_KEY='sk-dotenv' # home\n",
        )
        .unwrap();
        assert_eq!(api_key(dir.path(), &env(&[])), "sk-dotenv");
    }

    #[test]
    fn unquotes() {
        for (v, want) in [
            ("\"sk-a\"", "sk-a"),
            ("'sk-b' # note", "sk-b"),
            ("sk-c # note", "sk-c"),
            ("  sk-d  ", "sk-d"),
            ("\"open", "\"open"),
        ] {
            assert_eq!(unquote(v), want, "{v}");
        }
        assert_eq!(money("EUR", "3"), "3.00 EUR");
    }
}
