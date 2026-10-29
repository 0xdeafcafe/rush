//! Models served by Ollama on this machine, as rush adapters. There's no
//! agent of Ollama's own to run: rush runs Claude Code on Ollama's
//! Anthropic-compatible API, fitted to the model it runs (see `tune`), in a
//! Claude config folder of its own, so that a local model reads none of
//! the user's plugins, skills and MCP servers before every answer. Codex,
//! Pi and Vibe run on it too, each in a folder of rush's own. Each stays
//! out of sight until both ollama and its harness are installed.
//!
//! Ported from Go's internal/adapters/ollama: ollama.go (the Claude Code
//! adapter, in `claude`), api.go, manage.go, decide.go, tune.go,
//! summary.go, codex.go, pi.go and vibe.go. What Go keeps in package
//! variables (what each model reads, the models found, the context sizes
//! asked for) is a [`Local`]'s, one per Ollama server; every adapter here
//! shares the process's. A session starts through its harness's Driver,
//! found in the registry, as Go calls the claude, codex, pi and vibe
//! adapters' Start.
//!
//! Discovery never loads a model; a local server is started only when its
//! API can't be reached, and a remote one never.

mod api;
mod claude;
mod codex;
mod decide;
mod manage;
mod pi;
mod summary;
mod tune;
mod vibe;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use rush_core::adapter::{Choice, Kind, Registry, Session};

pub use api::Model;
pub use claude::{KIND, Ollama};
pub use codex::{CODEX_KIND, OllamaCodex, codex_flags};
pub use decide::clip_bytes;
pub use manage::model_name;
pub use pi::{OllamaPi, PI_KIND, pi_config};
pub use summary::summary_chunks;
pub use tune::{SUGGEST, rank, tune, window};
pub use vibe::{OllamaVibe, VIBE_KIND, vibe_config};

/// One Ollama server, and what rush has learnt of it.
pub struct Local {
    /// OLLAMA_HOST, when a test sets its own; None reads the environment.
    host: Option<String>,
    /// rush's folder; None is rush_state's.
    state_dir: Option<PathBuf>,
    /// The ollama program, when a test names its own.
    program: Option<String>,
    reg: Registry,
    /// Whether each model /api/show has told of reads images.
    shown: Mutex<HashMap<String, bool>>,
    /// The models being asked of in the background.
    asking: Mutex<HashSet<String>>,
    /// Every model told of, by name.
    known: Mutex<HashMap<String, Model>>,
    /// What the last discovery found.
    catalog: Mutex<Option<Arc<[Model]>>>,
    /// Models Ollama lists but can't read: name → why. A pull mends one.
    broken: Mutex<HashMap<String, String>>,
    /// The context each model is to run with, as saved.
    prefs: Mutex<manage::Prefs>,
    /// list_models for a minute: sessions starting ask it too.
    listed: Mutex<(Option<Instant>, Vec<Choice>)>,
    /// One start of the server at a time.
    starting: Mutex<()>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn err(s: impl Into<String>) -> rush_core::adapter::Error {
    s.into().into()
}

impl Local {
    /// The server OLLAMA_HOST names, knowing the ollama program from reg.
    pub fn new(reg: Registry) -> Arc<Local> {
        Local::with(reg, None, None, None)
    }

    /// The server at host (as OLLAMA_HOST says it), rush's folder at
    /// state_dir and ollama at program: for tests.
    pub fn with(
        reg: Registry,
        host: Option<&str>,
        state_dir: Option<PathBuf>,
        program: Option<&str>,
    ) -> Arc<Local> {
        Arc::new(Local {
            host: host.map(str::to_string),
            state_dir,
            program: program.map(str::to_string),
            reg,
            shown: Mutex::default(),
            asking: Mutex::default(),
            known: Mutex::default(),
            catalog: Mutex::default(),
            broken: Mutex::default(),
            prefs: Mutex::default(),
            listed: Mutex::default(),
            starting: Mutex::default(),
        })
    }

    /// rush's folder.
    pub(crate) fn state_dir(&self) -> PathBuf {
        self.state_dir.clone().unwrap_or_else(rush_state::dir)
    }
}

/// The process's Ollama, shared by every adapter registered with
/// [`register`]; the first registry asked for it is the one it knows.
pub fn shared(reg: &Registry) -> Arc<Local> {
    static SHARED: OnceLock<Arc<Local>> = OnceLock::new();
    SHARED.get_or_init(|| Local::new(reg.clone())).clone()
}

/// Registers Ollama in Claude Code, Codex, Pi and Vibe with reg (Go's
/// init funcs).
pub fn register(reg: &Registry) {
    let l = shared(reg);
    reg.register(Ollama::new(reg.clone(), l.clone()));
    reg.register(OllamaCodex::new(reg.clone(), l.clone()));
    reg.register(OllamaPi::new(reg.clone(), l.clone()));
    reg.register(OllamaVibe::new(reg.clone(), l));
}

/// The adapter of kind k in reg, when it has a Driver.
pub(crate) fn driver_of(
    reg: &Registry,
    k: &str,
) -> rush_core::adapter::Result<rush_core::adapter::AdapterRef> {
    reg.get(k)
        .filter(|a| a.driver().is_some())
        .ok_or_else(|| err(format!("rush can't run {k}")))
}

/// The harness's sessions as one of ours.
pub(crate) fn ours(mut ss: Vec<Session>, k: &str) -> Vec<Session> {
    for s in &mut ss {
        s.kind = Kind::from(k);
        s.profile.kind = Kind::from(k);
    }
    ss
}

/// The program the host handed over, or the harness's when that was
/// ollama's own (the adapter's program) or none.
pub(crate) fn harness_binary(reg: &Registry, binary: &str, harness: &str) -> String {
    let base = std::path::Path::new(binary)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if binary.is_empty() || base == "ollama" {
        return reg.path(harness);
    }
    binary.to_string()
}

#[cfg(test)]
pub(crate) mod testserver;

#[cfg(test)]
pub(crate) mod testsink {
    /// A Sink that drops what it's told.
    pub struct Nothing;
    impl rush_core::adapter::Sink for Nothing {
        fn event(&self, _: rush_core::event::Event) {}
        fn ended(&self, _: Option<rush_core::adapter::Error>) {}
    }
}
