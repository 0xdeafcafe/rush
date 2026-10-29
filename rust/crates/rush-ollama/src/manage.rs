//! Keeping Ollama: starting a local server when it can't be reached, the
//! models it has (found without loading one), the context each is to run
//! with, and their names as rush shows them.
//!
//! Ported from Go's internal/adapters/ollama/manage.go:1-262.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;

use rush_core::adapter::Result;

use crate::api::{Fail, Model};
use crate::{Local, err, lock, tune};

/// The context each model is to run with, as saved in
/// ollama-context.json, and the file it was read from.
#[derive(Default)]
pub(crate) struct Prefs {
    path: Option<PathBuf>,
    values: BTreeMap<String, i64>,
}

/// How long a start of the server is given to answer.
const START_FOR: Duration = Duration::from_secs(20);

/// How often a server starting is asked whether it's up, as Go's ticker.
const START_EVERY: Duration = Duration::from_millis(200);

impl Local {
    /// Whether any HTTP server answers at the endpoint: one already owns
    /// it, and none is ever started over it.
    fn probe(&self) -> bool {
        match self.send("GET", "/api/version", None, Some(Duration::from_secs(1))) {
            Ok(_) => true,
            Err(Fail::Slow | Fail::Down) => false,
        }
    }

    /// Starts a local server only when its API cannot be reached. Existing
    /// and remote servers are never restarted or reconfigured.
    pub fn ensure_running(&self) -> Result<()> {
        let _one = lock(&self.starting);
        if self.probe() {
            return Ok(());
        }
        let server = self.server();
        let host = host_of(&server);
        let loopback =
            host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        if !loopback {
            return Err(err(format!(
                "Ollama at {server} is unavailable; start it on that machine"
            )));
        }
        let bin = match &self.program {
            Some(p) => p.clone(),
            None => {
                let p = self.reg.path(crate::KIND);
                if p.is_empty() {
                    rush_core::adapter::find("ollama", &[]).ok_or_else(|| {
                        err("install Ollama to start local models: exec: \"ollama\": executable file not found in $PATH")
                    })?
                } else {
                    p
                }
            }
        };
        let mut cmd = Command::new(bin);
        cmd.arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The shared local server must outlive the session that first
        // needs it.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe and touches nothing of
            // the parent's.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn().map_err(|e| err(format!("start Ollama: {e}")))?;
        let (tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        let until = Instant::now() + START_FOR;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(err(format!(
                    "Ollama started but its API is not ready at {server}"
                )));
            }
            match done.recv_timeout(left.min(START_EVERY)) {
                Ok(status) => {
                    if self.probe() {
                        return Ok(());
                    }
                    let why = match status {
                        // As Go's *exec.ExitError says it.
                        Ok(s) => match s.code() {
                            Some(c) => format!("exit status {c}"),
                            None => s.to_string(),
                        },
                        Err(e) => e.to_string(),
                    };
                    return Err(err(format!("Ollama exited before becoming ready: {why}")));
                }
                Err(_) => {
                    if self.probe() {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Reads the saved context sizes, unless they were read from this
    /// file already.
    fn read_contexts(&self, p: &mut Prefs) {
        let path = self.state_dir().join("ollama-context.json");
        if p.path.as_ref() == Some(&path) {
            return;
        }
        p.values = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        p.path = Some(path);
    }

    /// The context asked for model, zero for the server's default. The
    /// sizes are read while discovering models, off the UI.
    pub fn context_size(&self, model: &str) -> i64 {
        lock(&self.prefs).values.get(model).copied().unwrap_or(0)
    }

    /// The saved context for model, read from disk if it hasn't been.
    pub(crate) fn context_pref(&self, model: &str) -> i64 {
        let mut p = lock(&self.prefs);
        self.read_contexts(&mut p);
        p.values.get(model).copied().unwrap_or(0)
    }

    /// Saves a context for future sessions of model, without reloading any
    /// running model. Zero is the server's default again.
    pub fn set_context_size(&self, model: &str, tokens: i64) -> Result<()> {
        if model.is_empty()
            || !(0..=16 * 1024 * 1024).contains(&tokens)
            || (tokens > 0 && tokens < 2048)
        {
            return Err(err(
                "context must be 0 (automatic), or 2048–16777216 tokens",
            ));
        }
        if let Some(m) = self.cached_model(model)
            && tokens > m.max_context
            && m.max_context > 0
        {
            return Err(err(format!(
                "{model} supports at most {} context tokens",
                m.max_context
            )));
        }
        let mut p = lock(&self.prefs);
        self.read_contexts(&mut p);
        let mut next = p.values.clone();
        if tokens == 0 {
            next.remove(model);
        } else {
            next.insert(model.into(), tokens);
        }
        let path = p.path.clone().expect("read");
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        rush_state::write_json(&path, &next)?;
        p.values = next;
        Ok(())
    }

    /// What was last found of model; no disk or network, so safe in a
    /// view.
    pub fn cached_model(&self, name: &str) -> Option<Model> {
        lock(&self.known).get(name).cloned()
    }

    /// What the last discovery found; no I/O.
    pub fn cached_installed_models(&self) -> Arc<[Model]> {
        lock(&self.catalog).clone().unwrap_or_else(|| Arc::from([]))
    }

    /// Every model Ollama has, those that can't call tools too, so the UI
    /// can say why they can't run a coding harness. It never loads a
    /// model.
    pub fn installed_models(&self, timeout: Option<Duration>) -> Result<Vec<Model>> {
        self.ensure_running()?;
        {
            let mut p = lock(&self.prefs);
            self.read_contexts(&mut p);
        }
        let all = self.names(timeout)?;
        let in_mem = self.loaded(timeout).unwrap_or_default();
        let mut out = Vec::with_capacity(all.len());
        for name in all {
            if name.starts_with("rush-context-") {
                continue;
            }
            let mut m = match self.show(&name, timeout) {
                Ok(m) => m,
                Err(e) => {
                    let why = e.to_string();
                    if !why.contains("took too long") {
                        lock(&self.broken).insert(name, why);
                    }
                    continue;
                }
            };
            lock(&self.broken).remove(&name);
            if let Some(l) = in_mem.get(&name) {
                m.context = l.context;
                m.vram = l.vram;
            }
            lock(&self.known).insert(name, m.clone());
            out.push(m);
        }
        *lock(&self.catalog) = Some(Arc::from(out.clone()));
        Ok(out)
    }

    /// m as a model of its own that keeps the context asked for (num_ctx):
    /// a preload option alone would be lost when a harness makes its next
    /// API request.
    pub(crate) fn configured_model(
        &self,
        mut m: Model,
        timeout: Option<Duration>,
    ) -> Result<Model> {
        let n = self.context_pref(&m.name);
        if n == 0 {
            return Ok(m);
        }
        if m.max_context > 0 && n > m.max_context {
            return Err(err(format!(
                "context {n} exceeds {}'s maximum {}",
                m.name, m.max_context
            )));
        }
        let name = format!("rush-context-{}-ctx-{n}", m.name);
        self.call_raw(
            "POST",
            "/api/create",
            Some(&json!({"model": name, "from": m.name, "parameters": {"num_ctx": n}, "stream": false})),
            timeout,
        )?;
        m.name = name;
        m.context = n;
        lock(&self.known).insert(m.name.clone(), m.clone());
        Ok(m)
    }

    /// The window of a model found before; 0 when none was.
    pub fn cached_window(&self, name: &str) -> i64 {
        self.cached_model(name).map_or(0, |m| tune::window(&m))
    }
}

/// The host of a server's URL, without its port.
fn host_of(server: &str) -> String {
    let rest = server.split_once("://").map_or(server, |(_, r)| r);
    let hostport = rest.split('/').next().unwrap_or("");
    let hostport = hostport.rsplit_once('@').map_or(hostport, |(_, h)| h);
    if let Some(v6) = hostport.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("").to_string();
    }
    hostport.split(':').next().unwrap_or("").to_string()
}

/// A model's name as rush shows it: a model made for a context size (one
/// configured_model made) is the model it's made from, and that context.
pub fn model_name(id: &str) -> String {
    let id = id.strip_prefix("ollama/").unwrap_or(id);
    if let Some(base) = id.strip_prefix("rush-context-")
        && let Some(at) = base.rfind("-ctx-")
    {
        return format!("{} · {} context", &base[..at], &base[at + 5..]);
    }
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::serve;
    use rush_core::adapter::Registry;

    fn local(url: &str) -> (Arc<Local>, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let l = Local::with(
            Registry::new(),
            Some(url),
            Some(d.path().to_path_buf()),
            Some("/nonexistent/ollama"),
        );
        (l, d)
    }

    #[test]
    fn installed_models_keep_loaded_context_without_loading() {
        let url = serve(|r| {
            let out = match r.path.as_str() {
                "/api/version" => r#"{"version":"test"}"#,
                "/api/tags" => {
                    r#"{"models":[{"name":"coder"},{"name":"embed"},{"name":"rush-context-hidden"}]}"#
                }
                "/api/ps" => {
                    r#"{"models":[{"name":"coder","context_length":8192,"size_vram":123}]}"#
                }
                "/api/show" if r.json()["model"] == "coder" => {
                    r#"{"capabilities":["tools"],"model_info":{"test.context_length":32768}}"#
                }
                "/api/show" => r#"{"capabilities":["embedding"]}"#,
                p => panic!("discovery must not load models: {p}"),
            };
            (200, out.to_string())
        });
        let (l, _d) = local(&url);
        let models = l.installed_models(None).unwrap();
        assert_eq!(models.len(), 2, "{models:?}");
        assert_eq!((models[0].context, models[0].max_context), (8192, 32768));
        assert_eq!(l.cached_window("coder"), 8192);
        assert_eq!(l.cached_installed_models().len(), 2);
    }

    #[test]
    fn context_preference_makes_a_model_that_keeps_it() {
        let url = serve(|r| {
            let v = r.json();
            match r.path.as_str() {
                "/api/create" => {
                    assert!(
                        v["model"]
                            .as_str()
                            .unwrap()
                            .starts_with("rush-context-my-coder")
                    );
                    assert_eq!(v["from"], "my-coder");
                    assert_eq!(v["parameters"]["num_ctx"], 16384);
                    assert_eq!(v["stream"], false);
                    (200, r#"{"status":"success"}"#.into())
                }
                "/api/generate" => {
                    assert_eq!(v["model"], "rush-context-my-coder-ctx-16384");
                    assert_eq!(v["stream"], false);
                    (200, r#"{"done":true}"#.into())
                }
                p => panic!("unexpected {p}"),
            }
        });
        let (l, _d) = local(&url);
        lock(&l.known).insert(
            "my-coder".into(),
            Model {
                name: "my-coder".into(),
                max_context: 32768,
                ..Model::default()
            },
        );
        assert!(
            l.set_context_size("my-coder", 65536).is_err(),
            "accepted above model maximum"
        );
        l.set_context_size("my-coder", 16384).unwrap();
        let m = l
            .configured_model(
                Model {
                    name: "my-coder".into(),
                    max_context: 32768,
                    ..Model::default()
                },
                None,
            )
            .unwrap();
        assert!(
            m.name.starts_with("rush-context-") && m.context == 16384,
            "{m:?}"
        );
        l.load(&m.name, None).unwrap();
        l.set_context_size("my-coder", 0).unwrap();
        assert_eq!(l.context_size("my-coder"), 0);
        assert!(l.set_context_size("my-coder", 100).is_err());
    }

    #[test]
    fn never_starts_over_an_http_server() {
        let url = serve(|_| (401, String::new()));
        let (l, _d) = local(&url);
        l.ensure_running().unwrap();
    }

    #[test]
    fn never_starts_a_remote_server() {
        let (l, _d) = local("http://192.0.2.1:1");
        let e = l.ensure_running().unwrap_err();
        assert_eq!(
            e.to_string(),
            "Ollama at http://192.0.2.1:1 is unavailable; start it on that machine"
        );
    }

    // A local server that isn't there is started, and one that won't start
    // says so.
    #[test]
    fn starts_a_local_server() {
        let l = TcpListenerFree::port();
        let (l, _d) = local(&format!("127.0.0.1:{l}"));
        let e = l.ensure_running().unwrap_err().to_string();
        assert!(e.starts_with("start Ollama: "), "{e}");
    }

    // A server that ends before it answers says how it ended.
    #[cfg(unix)]
    #[test]
    fn a_server_that_exits_says_so() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        let p = bin.path().join("ollama");
        std::fs::write(&p, "#!/bin/sh\n[ \"$1\" = serve ] && exit 3\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let port = TcpListenerFree::port();
        let d = tempfile::tempdir().unwrap();
        let l = Local::with(
            Registry::new(),
            Some(&format!("localhost:{port}")),
            Some(d.path().to_path_buf()),
            Some(p.to_str().unwrap()),
        );
        let e = l.ensure_running().unwrap_err().to_string();
        assert_eq!(e, "Ollama exited before becoming ready: exit status 3");
    }

    struct TcpListenerFree;
    impl TcpListenerFree {
        /// A port nothing listens on.
        fn port() -> u16 {
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        }
    }

    #[test]
    fn hosts() {
        for (s, want) in [
            ("http://127.0.0.1:11434", "127.0.0.1"),
            ("https://gpu.box:80", "gpu.box"),
            ("http://[::1]:11434", "::1"),
            ("http://localhost:1/x", "localhost"),
        ] {
            assert_eq!(host_of(s), want);
        }
    }

    #[test]
    fn configured_model_names_stay_recognisable() {
        for id in [
            "rush-context-qwen2.5:1.5b-ctx-16384",
            "ollama/rush-context-qwen2.5:1.5b-ctx-16384",
        ] {
            assert_eq!(model_name(id), "qwen2.5:1.5b · 16384 context", "{id}");
        }
        assert_eq!(model_name("qwen3.8:27b-mlx"), "qwen3.8:27b-mlx");
    }
}
