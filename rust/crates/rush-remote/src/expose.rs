//! What puts serve in reach of other machines, as the remote-tailscale and
//! remote-cloudflared plugins say: the installed tailscale's `serve` for
//! serve's port, and the installed cloudflared with the tunnel's token
//! file. Each is serve's own and the only thing taken down: never another
//! serve's mapping, tunnel or agent.
//!
//! After Go's internal/remote/expose.go (Expose :38, exposure :87, set
//! :95, wait :134, tailscaleBin :142, unmapTailscale :155, tailscale :178,
//! serveStatus :186, mapping :197, portFlag :224, tailscaleUp :236,
//! cloudflaredRun :277).

use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::config::{Config, dir};
use crate::ctx::Ctx;
use crate::plugins::{self, PLUGIN_CLOUDFLARED, PLUGIN_SSH, PLUGIN_TAILSCALE, plugin_on};

/// Where serve says what it's doing: its log (stderr, or serve.log).
pub type Log = Arc<Mutex<dyn Write + Send>>;

/// Says s on log.
pub fn say(log: &Log, s: &str) {
    let mut l = log.lock().unwrap_or_else(|e| e.into_inner());
    let _ = l.write_all(s.as_bytes());
    let _ = l.flush();
}

/// Runs until ctx ends, or fails; undoes what it did itself.
pub type Up = Arc<dyn Fn(&Ctx) -> Result<(), String> + Send + Sync>;

/// One way of being reached, started and stopped as the plugins say.
pub struct Exposure {
    name: String,
    log: Log,
    up: Option<Up>,
    stop: Option<Ctx>,
    worker: Option<JoinHandle<()>>,
}

impl Exposure {
    pub fn new(name: &str, log: Log, up: Option<Up>) -> Exposure {
        Exposure {
            name: name.into(),
            log,
            up,
            stop: None,
            worker: None,
        }
    }

    /// Starts it or stops it; nothing when it's already so.
    pub fn set(&mut self, ctx: &Ctx, on: bool) {
        if on == self.stop.is_some() {
            return;
        }
        if !on {
            rush_events::log("expose-off", fields(json!({ "via": self.name })));
            if let Some(s) = self.stop.take() {
                s.cancel();
            }
            self.join();
            return;
        }
        let Some(up) = self.up.clone() else {
            return;
        };
        rush_events::log("expose-on", fields(json!({ "via": self.name })));
        // A worker turned off is gone, its mapping with it, before the
        // next starts.
        self.join();
        let c = ctx.child();
        self.stop = Some(c.clone());
        let (name, log) = (self.name.clone(), self.log.clone());
        self.worker = std::thread::Builder::new()
            .name(format!("expose-{name}"))
            .spawn(move || {
                // A failure is logged once until it changes.
                let mut said = String::new();
                while !c.done() {
                    match up(&c) {
                        Err(e) if !c.done() => {
                            if e != said {
                                said = e.clone();
                                rush_events::log(
                                    "expose-failed",
                                    fields(json!({ "via": name, "err": said, "why": "tried again every 15s" })),
                                );
                            }
                            say(&log, &format!("rush serve: {name}: {e}\n"));
                            // A failure is tried again, not spun on.
                            c.sleep(Duration::from_secs(15));
                        }
                        _ => break,
                    }
                }
            })
            .ok();
    }

    fn join(&mut self) {
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }

    /// Stops it and waits for it to be down.
    pub fn wait(&mut self) {
        if let Some(s) = self.stop.take() {
            s.cancel();
        }
        self.join();
    }
}

pub(crate) fn fields(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

/// Runs what puts serve (listening on port) in reach, as the plugins say,
/// reading their switches every few seconds, so a change takes effect
/// without a restart. sshd, when given, is serve's SSH, run while
/// remote-ssh is on and mapped onto the tailnet too while serve is
/// reached over Tailscale. The returned handle ends once ctx is done and
/// they are down.
pub fn expose(ctx: &Ctx, cfg: Config, port: u16, log: Log, sshd: Option<Up>) -> JoinHandle<()> {
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("expose".into())
        .spawn(move || {
            let ts_cfg = cfg.clone();
            let ts_log = log.clone();
            let mut ts = Exposure::new(
                "tailscale",
                log.clone(),
                Some(Arc::new(move |c: &Ctx| {
                    let bin = tailscale_bin()?;
                    tailscale_up(
                        c,
                        &bin,
                        ts_cfg.tailscale_port_or(),
                        &format!("http://127.0.0.1:{port}"),
                        &ts_log,
                    )
                })),
            );
            let cf_cfg = cfg.clone();
            let cf_log = log.clone();
            let mut cf = Exposure::new(
                "cloudflared",
                log.clone(),
                Some(Arc::new(move |c: &Ctx| {
                    cloudflared_run(c, &cf_cfg, port, &cf_log)
                })),
            );
            let has_sshd = sshd.is_some();
            let ssh_on = || has_sshd && plugin_on(PLUGIN_SSH);
            let mut sd = Exposure::new("ssh", log.clone(), sshd);
            let sts_cfg = cfg.clone();
            let sts_log = log.clone();
            let mut sts = Exposure::new(
                "tailscale ssh",
                log.clone(),
                Some(Arc::new(move |c: &Ctx| {
                    let listen = sts_cfg.ssh_listen_or();
                    let p = listen
                        .rsplit_once(':')
                        .map(|(_, p)| p.to_string())
                        .ok_or_else(|| format!("address {listen}: missing port in address"))?;
                    let bin = tailscale_bin()?;
                    tailscale_up(
                        c,
                        &bin,
                        sts_cfg.ssh_port_or(),
                        &format!("tcp://127.0.0.1:{p}"),
                        &sts_log,
                    )
                })),
            );
            loop {
                plugins::migrate();
                let live = !ctx.done();
                let ts_on = plugin_on(PLUGIN_TAILSCALE);
                ts.set(&ctx, ts_on && live);
                cf.set(&ctx, plugin_on(PLUGIN_CLOUDFLARED) && live);
                sd.set(&ctx, ssh_on() && live);
                sts.set(&ctx, ssh_on() && ts_on && live);
                if ctx.done() {
                    ts.wait();
                    cf.wait();
                    sd.wait();
                    sts.wait();
                    return;
                }
                ctx.sleep(Duration::from_secs(3));
            }
        })
        .expect("a thread for expose")
}

/// The installed tailscale: on PATH, or the Mac app's.
pub fn tailscale_bin() -> Result<PathBuf, String> {
    if let Some(p) = crate::config::look_path("tailscale") {
        return Ok(p);
    }
    let app = Path::new("/Applications/Tailscale.app/Contents/MacOS/Tailscale");
    if app.exists() {
        return Ok(app.into());
    }
    Err("tailscale isn't installed".into())
}

/// The tailnet's https address for this serve, as `tailscale status
/// --json` names this machine; "" when it can't say (pair.go reachedAt).
pub fn tailnet_url(cfg: &Config) -> String {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct St {
        #[serde(rename = "Self")]
        me: Me,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Me {
        #[serde(rename = "DNSName")]
        dns: String,
    }
    let Ok(bin) = tailscale_bin() else {
        return String::new();
    };
    let Ok(out) = run(
        tailscale(&bin, &["status", "--json"]),
        false,
        Duration::from_secs(5),
    ) else {
        return String::new();
    };
    match serde_json::from_slice::<St>(&out) {
        Ok(st) if !st.me.dns.is_empty() => format!(
            "https://{}:{}",
            st.me.dns.trim_end_matches('.'),
            cfg.tailscale_port_or()
        ),
        _ => String::new(),
    }
}

/// A tailscale command. The Mac app's binary is its CLI only from a
/// terminal or with TAILSCALE_BE_CLI set.
fn tailscale(bin: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(bin);
    c.args(args)
        .env("TAILSCALE_BE_CLI", "1")
        .stdin(Stdio::null());
    c
}

/// Runs c to its end or for at most `most`, its output (stdout, or both).
fn run(mut c: Command, both: bool, most: Duration) -> Result<Vec<u8>, String> {
    c.stdout(Stdio::piped());
    c.stderr(if both { Stdio::piped() } else { Stdio::null() });
    let child = c.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = match rx.recv_timeout(most) {
        Ok(r) => r.map_err(|e| e.to_string())?,
        Err(_) => {
            // SAFETY: kill only signals the child this started.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            return Err("signal: killed".into());
        }
    };
    let mut b = out.stdout;
    if both {
        b.extend_from_slice(&out.stderr);
    }
    if !out.status.success() {
        let code = out
            .status
            .code()
            .map_or("signal: killed".to_string(), |c| format!("exit status {c}"));
        return Err(format!("{code}\x00{}", String::from_utf8_lossy(&b)));
    }
    Ok(b)
}

/// An error from run, as Go's `%v: %s` says it.
fn ran_err(e: &str) -> (String, String) {
    match e.split_once('\x00') {
        Some((a, b)) => (a.into(), b.into()),
        None => (e.into(), String::new()),
    }
}

/// `tailscale serve status --json`'s part that says what's on a port.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ServeStatus {
    #[serde(rename = "TCP")]
    tcp: Option<std::collections::BTreeMap<String, TcpEntry>>,
    #[serde(rename = "Web")]
    web: Option<std::collections::BTreeMap<String, WebEntry>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TcpEntry {
    #[serde(rename = "TCPForward")]
    tcp_forward: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WebEntry {
    #[serde(rename = "Handlers")]
    handlers: Option<std::collections::BTreeMap<String, Handler>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Handler {
    #[serde(rename = "Proxy")]
    proxy: String,
}

/// What the tailnet's port holds: whether anything does, and whether it
/// is exactly one "/" proxying to target, or for a tcp:// target a
/// forward to it.
fn mapping(bin: &Path, port: i64, target: &str) -> Result<(bool, bool), String> {
    let out = run(
        tailscale(bin, &["serve", "status", "--json"]),
        false,
        Duration::from_secs(60),
    )
    .map_err(|e| format!("tailscale serve status: {}", ran_err(&e).0))?;
    let st: ServeStatus =
        serde_json::from_slice(&out).map_err(|e| format!("tailscale serve status: {e}"))?;
    let p = port.to_string();
    let tcp = st.tcp.unwrap_or_default();
    let held_tcp = tcp.get(&p);
    if let Some(fwd) = target.strip_prefix("tcp://") {
        return Ok((
            held_tcp.is_some(),
            held_tcp.is_some_and(|t| t.tcp_forward == fwd),
        ));
    }
    let mut held = held_tcp.is_some();
    let mut ours = false;
    for (host, w) in st.web.unwrap_or_default() {
        if !host.ends_with(&format!(":{p}")) {
            continue;
        }
        held = true;
        let h = w.handlers.unwrap_or_default();
        if h.len() == 1 && h.get("/").is_some_and(|x| x.proxy == target) {
            ours = true;
        }
    }
    Ok((held, ours))
}

/// tailscale serve's flag for the tailnet port target is on.
fn port_flag(ts_port: i64, target: &str) -> String {
    if target.starts_with("tcp://") {
        format!("--tcp={ts_port}")
    } else {
        format!("--https={ts_port}")
    }
}

/// Takes down a tailnet mapping to this machine's serve (cfg's listen
/// port), and nothing else: for when serve has been stopped without the
/// chance to.
pub fn unmap_tailscale(cfg: &Config) {
    let Ok(bin) = tailscale_bin() else {
        return;
    };
    unmap_tailscale_with(&bin, cfg);
}

/// unmap_tailscale with the tailscale at bin.
pub fn unmap_tailscale_with(bin: &Path, cfg: &Config) {
    let Some((_, port)) = cfg.listen.rsplit_once(':') else {
        return;
    };
    let mut maps = vec![(cfg.tailscale_port_or(), format!("http://127.0.0.1:{port}"))];
    if let Some((_, sport)) = cfg.ssh_listen_or().rsplit_once(':') {
        maps.push((cfg.ssh_port_or(), format!("tcp://127.0.0.1:{sport}")));
    }
    for (ts, target) in maps {
        if let Ok((_, true)) = mapping(bin, ts, &target) {
            let _ = run(
                tailscale(bin, &["serve", &port_flag(ts, &target), "off"]),
                true,
                Duration::from_secs(20),
            );
        }
    }
}

/// Maps the tailnet's port to target (serve, or its SSH), and blocks
/// until ctx ends, then takes the mapping down. A mapping already there is
/// someone's, and is left alone; only one this call made, and that still
/// reads as it made it, is removed.
pub fn tailscale_up(
    ctx: &Ctx,
    bin: &Path,
    ts_port: i64,
    target: &str,
    log: &Log,
) -> Result<(), String> {
    let flag = port_flag(ts_port, target);
    // Startup runs to its end even if ctx ends meanwhile: a mapping made
    // and not looked at again would be left behind.
    let (held, ours) = mapping(bin, ts_port, target)?;
    let out = if ours {
        // A serve here that ended without taking it down left it: it points
        // at the port this serve holds, so it's this serve's to keep.
        "(kept the mapping a serve here left)\n".to_string()
    } else if held {
        return Err(format!(
            "tailnet https port {ts_port} already serves something; not touching it (pick another with remote.json's tailscalePort, or `tailscale serve {flag} off` if it's stale)"
        ));
    } else {
        match run(
            tailscale(bin, &["serve", "--bg", &flag, target]),
            true,
            Duration::from_secs(60),
        ) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                let (e, out) = ran_err(&e);
                return Err(format!("tailscale serve: {e}: {out}"));
            }
        }
    };
    // Ours from here, if it reads as we set it.
    match mapping(bin, ts_port, target) {
        Ok((_, true)) => {}
        Ok(_) => {
            return Err(
                "tailscale serve didn't leave the mapping it should (<nil>); leaving it".into(),
            );
        }
        Err(e) => {
            return Err(format!(
                "tailscale serve didn't leave the mapping it should ({e}); leaving it"
            ));
        }
    }
    say(
        log,
        &format!(
            "rush serve: on the tailnet, {}\n{out}",
            flag.trim_start_matches("--")
        ),
    );
    ctx.wait();
    if !matches!(mapping(bin, ts_port, target), Ok((_, true))) {
        say(
            log,
            &format!("rush serve: tailnet port {ts_port} no longer reads as ours; leaving it\n"),
        );
        return Ok(());
    }
    if let Err(e) = run(
        tailscale(bin, &["serve", &flag, "off"]),
        true,
        Duration::from_secs(20),
    ) {
        let (e, out) = ran_err(&e);
        say(
            log,
            &format!("rush serve: tailscale serve off: {e}: {out}\n"),
        );
    }
    Ok(())
}

/// Runs the tunnel until ctx ends. cloudflared 2025.4.0 or later reads the
/// token from a file; its hostname in Cloudflare points at
/// http://127.0.0.1:<port>.
pub fn cloudflared_run(ctx: &Ctx, cfg: &Config, port: u16, log: &Log) -> Result<(), String> {
    let tok = if cfg.cloudflared.is_empty() {
        dir().join("cloudflared-token")
    } else {
        PathBuf::from(&cfg.cloudflared)
    };
    if !tok.exists() {
        return Err(format!("no tunnel token file at {}", tok.display()));
    }
    let bin = crate::config::look_path("cloudflared").ok_or("cloudflared isn't installed")?;
    say(
        log,
        &format!(
            "rush serve: cloudflared tunnel (its hostname should point at http://127.0.0.1:{port})\n"
        ),
    );
    let mut child = Command::new(bin)
        .args(["tunnel", "--no-autoupdate", "run", "--token-file"])
        .arg(&tok)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Its output goes to serve's log.
    for r in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let log = log.clone();
        std::thread::spawn(move || {
            let mut r = r;
            let mut buf = [0u8; 8192];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut l = log.lock().unwrap_or_else(|e| e.into_inner());
                let _ = l.write_all(&buf[..n]);
            }
        });
    }
    let pid = child.id() as i32;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait());
    });
    // Waits on the tunnel or the end, whichever is first.
    let stopper = {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            ctx.wait();
            // SAFETY: kill only signals the tunnel this started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        })
    };
    let st = rx.recv().map_err(|e| e.to_string())?;
    let _ = stopper;
    match st {
        Ok(s) if !s.success() && !ctx.done() => Err(match s.code() {
            Some(c) => format!("exit status {c}"),
            None => "signal: killed".into(),
        }),
        Err(e) if !ctx.done() => Err(e.to_string()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A tailscale that keeps its serve config in status.json and its calls
    /// in calls: just enough of `serve status --json`, `serve --bg
    /// --https=P URL` and `serve --https=P off` (expose_test.go's).
    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Fake {
            let dir = tempfile::tempdir().unwrap();
            let d = dir.path().display();
            let script = format!(
                r#"#!/bin/sh
D="{d}"
echo "$@" >> "$D/calls"
case "$1 $2" in
"serve status") cat "$D/status.json" 2>/dev/null || echo '{{}}' ;;
"serve --bg")
  [ -e "$D/failserve" ] && {{ echo boom >&2; exit 1; }}
  p=${{3#--https=}}
  printf '{{"TCP":{{"%s":{{"HTTPS":true}}}},"Web":{{"box.ts.net:%s":{{"Handlers":{{"/":{{"Proxy":"%s"}}}}}}}}}}' "$p" "$p" "$4" > "$D/status.json" ;;
"serve --https="*) echo '{{}}' > "$D/status.json" ;;
esac
"#
            );
            let p = dir.path().join("tailscale");
            std::fs::write(&p, script).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            Fake { dir }
        }
        fn bin(&self) -> PathBuf {
            self.dir.path().join("tailscale")
        }
        fn read(&self, n: &str) -> String {
            std::fs::read_to_string(self.dir.path().join(n)).unwrap_or_default()
        }
        fn count(&self, sub: &str) -> usize {
            self.read("calls").matches(sub).count()
        }
        fn set(&self, s: &str) {
            std::fs::write(self.dir.path().join("status.json"), s).unwrap();
        }
        fn exposure(&self, log: &Log, serve: u16) -> Exposure {
            let (bin, l) = (self.bin(), log.clone());
            Exposure::new(
                "tailscale",
                log.clone(),
                Some(Arc::new(move |c: &Ctx| {
                    tailscale_up(c, &bin, 8443, &format!("http://127.0.0.1:{serve}"), &l)
                })),
            )
        }
    }

    const FOREIGN: &str = r#"{"TCP":{"8443":{"HTTPS":true}},"Web":{"box.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}"#;

    fn log() -> (Log, Arc<Mutex<Vec<u8>>>) {
        let b = Arc::new(Mutex::new(Vec::new()));
        (b.clone() as Log, b)
    }

    fn text(b: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8_lossy(&b.lock().unwrap()).into_owned()
    }

    /// Waits for ok, at most 10s: the worker is a thread of its own.
    fn until(what: &str, ok: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if ok() {
                return;
            }
            std::thread::yield_now();
            std::thread::park_timeout(Duration::from_millis(5));
        }
        panic!("timed out: {what}");
    }

    #[test]
    fn happy_path_removes_only_its_mapping() {
        let f = Fake::new();
        let (l, _) = log();
        let mut e = f.exposure(&l, 7878);
        let ctx = Ctx::new();
        e.set(&ctx, true);
        until("mapped", || {
            f.read("status.json").contains("127.0.0.1:7878")
        });
        e.set(&ctx, false);
        assert_eq!(f.read("status.json"), "{}\n");
        assert_eq!(f.count(" off"), 1);
    }

    #[test]
    fn leaves_a_foreign_mapping() {
        let f = Fake::new();
        f.set(FOREIGN);
        let (l, b) = log();
        let mut e = f.exposure(&l, 7878);
        let ctx = Ctx::new();
        e.set(&ctx, true);
        until("refused", || text(&b).contains("not touching it"));
        e.set(&ctx, false);
        assert_eq!(f.read("status.json"), FOREIGN);
        assert_eq!(f.count("--bg"), 0);
        assert_eq!(f.count(" off"), 0);
    }

    #[test]
    fn a_failed_start_does_no_cleanup() {
        let f = Fake::new();
        std::fs::write(f.dir.path().join("failserve"), "").unwrap();
        let (l, b) = log();
        let mut e = f.exposure(&l, 7878);
        let ctx = Ctx::new();
        e.set(&ctx, true);
        until("failed", || text(&b).contains("boom"));
        e.set(&ctx, false);
        assert_eq!(f.count(" off"), 0, "{}", f.read("calls"));
    }

    #[test]
    fn leaves_a_mapping_changed_under_it() {
        let f = Fake::new();
        let (l, b) = log();
        let mut e = f.exposure(&l, 7878);
        let ctx = Ctx::new();
        e.set(&ctx, true);
        until("mapped", || text(&b).contains("on the tailnet"));
        // Someone replaced it.
        f.set(FOREIGN);
        e.set(&ctx, false);
        assert!(text(&b).contains("no longer reads as ours"));
        assert_eq!(f.read("status.json"), FOREIGN);
    }

    #[test]
    fn keeps_what_a_killed_serve_left() {
        let f = Fake::new();
        f.set(r#"{"TCP":{"8443":{"HTTPS":true}},"Web":{"box.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7878"}}}}}"#);
        let (l, b) = log();
        let mut e = f.exposure(&l, 7878);
        let ctx = Ctx::new();
        e.set(&ctx, true);
        until("kept", || text(&b).contains("kept the mapping"));
        assert_eq!(f.count("--bg"), 0);
        e.set(&ctx, false);
        assert_eq!(f.read("status.json"), "{}\n");
    }

    #[test]
    fn unmap_only_ours() {
        let f = Fake::new();
        let cfg = Config {
            listen: "127.0.0.1:7878".into(),
            ..Config::default()
        };
        f.set(FOREIGN);
        unmap_tailscale_with(&f.bin(), &cfg);
        assert_eq!(f.count(" off"), 0);
        f.set(
            r#"{"Web":{"box.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7878"}}}}}"#,
        );
        unmap_tailscale_with(&f.bin(), &cfg);
        assert_eq!(f.count("--https=8443 off"), 1);
    }

    #[test]
    fn competing_serves() {
        let f = Fake::new();
        let (l1, _) = log();
        let (l2, b2) = log();
        let (mut a, mut b) = (f.exposure(&l1, 7878), f.exposure(&l2, 7879));
        let ctx = Ctx::new();
        a.set(&ctx, true);
        until("a mapped", || f.read("status.json").contains("7878"));
        b.set(&ctx, true);
        until("b refused", || text(&b2).contains("not touching it"));
        b.set(&ctx, false);
        assert!(f.read("status.json").contains("7878"));
        assert_eq!(f.count(" off"), 0);
        a.set(&ctx, false);
        assert_eq!(f.read("status.json"), "{}\n");
    }

    #[test]
    fn rapid_toggle() {
        let f = Fake::new();
        let (l, _) = log();
        let mut e = f.exposure(&l, 7878);
        let ctx = Ctx::new();
        for _ in 0..5 {
            e.set(&ctx, true);
            until("mapped", || f.read("status.json").contains("7878"));
            e.set(&ctx, false);
            e.set(&ctx, true);
        }
        until("mapped at the end", || {
            f.read("status.json").contains("7878")
        });
        assert_eq!(f.count("--bg"), f.count(" off") + 1, "{}", f.read("calls"));
        e.wait();
        assert_eq!(f.read("status.json"), "{}\n");
    }
}
