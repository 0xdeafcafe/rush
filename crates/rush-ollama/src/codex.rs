//! Codex on Ollama: `codex app-server`, as the codex adapter drives it,
//! with a model provider of rush's own pointed at Ollama's
//! OpenAI-compatible Responses API, in a CODEX_HOME of its own so its
//! threads never mix with the user's ~/.codex, nor reach their OpenAI
//! account.
//!
//! Ported from Go's internal/adapters/ollama/codex.go:1-160. Its sessions
//! and their history are Codex's, read through the codex adapter in the
//! registry.

use std::sync::Arc;

use rush_core::adapter::{
    Adapter, Choice, Conn, ContextWindower, Discoverer, Driver, Feature, Features, Follow,
    HistoryFollower, HistoryReader, Kind, Level, Media, MediaReader, ModelLister, ModelNamer,
    Pricer, Profile, Programmer, Provided, Registry, Result, Rider, Session, Sink, StartOptions,
    Support, TailReader,
};
use rush_core::agent::TokenUsage;
use rush_core::event::Event;
use rush_core::time::Ms;

use crate::api::Model;
use crate::{KIND, Local, driver_of, err, harness_binary, manage, ours, tune};

/// Ollama in Codex's kind.
pub const CODEX_KIND: &str = "ollama-codex";

/// Codex's kind, which it rides.
const CODEX: &str = "codex";

/// The id of the model provider rush gives Codex. It's rush's own rather
/// than Codex's built-in "ollama", so its base URL can follow OLLAMA_HOST.
const CODEX_PROVIDER: &str = "rush-ollama";

/// Codex on Ollama.
#[derive(Clone)]
pub struct OllamaCodex {
    reg: Registry,
    local: Arc<Local>,
}

impl OllamaCodex {
    pub fn new(reg: Registry, local: Arc<Local>) -> OllamaCodex {
        OllamaCodex { reg, local }
    }

    /// The CODEX_HOME threads on Ollama keep to.
    pub fn home(&self) -> String {
        self.local
            .state_dir()
            .join(CODEX_KIND)
            .to_string_lossy()
            .into_owned()
    }

    fn codex(&self) -> Option<rush_core::adapter::AdapterRef> {
        self.reg.get(CODEX)
    }
}

/// A Go-quoted string, as strconv.Quote writes it: what Codex's -c reads
/// as a TOML string.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// app-server's config overrides that run it on model m, served at base,
/// ahead of flags so the caller's own win.
pub fn codex_flags(m: &Model, base: &str, flags: &[String]) -> Vec<String> {
    let mut out = vec![
        "-c".to_string(),
        format!("model_provider={}", quote(CODEX_PROVIDER)),
        "-c".to_string(),
        format!(
            "model_providers.{CODEX_PROVIDER}={{name=\"Ollama\",base_url={},wire_api=\"responses\"}}",
            quote(&format!("{base}/v1"))
        ),
        "-c".to_string(),
        format!("model={}", quote(&m.name)),
    ];
    let n = tune::window(m);
    if n > 0 {
        out.push("-c".into());
        out.push(format!("model_context_window={n}"));
    }
    out.extend(flags.iter().cloned());
    out
}

/// Codex's features, less what a local model or rush's own folder
/// hasn't: no account, no limits, and nothing to sign in to.
fn features() -> Features {
    use Feature::*;
    let yes = Support::YES;
    Features::from([
        (Run, yes.clone()),
        (Prompt, yes.clone()),
        (Port, yes.clone()),
        (Resume, yes.clone()),
        (Fork, yes.clone()),
        (Interrupt, yes.clone()),
        (
            Model,
            yes.clone().with("any model Ollama has that calls tools"),
        ),
        (Modes, yes.clone().with("read-only, auto, full-access")),
        (Images, yes.clone().with("on models that take them")),
        (Questions, yes.clone()),
        (
            Context,
            yes.clone().with("how full it is, not what fills it"),
        ),
        (Compact, yes.clone()),
        (HandoffIn, yes.clone()),
        (Live, yes.clone()),
        (History, yes.clone()),
        (Pricing, yes.with("free: it runs on this machine")),
        (
            Effort,
            Support::NO.with("a model thinks or doesn't, as it was made"),
        ),
        (Quota, Support::NO.with("no limits but this machine's")),
        (Switch, Support::NO.with("no accounts, only this machine")),
        (SignIn, Support::NO.with("nothing to sign in to")),
    ])
}

impl Adapter for OllamaCodex {
    fn kind(&self) -> Kind {
        Kind::from(CODEX_KIND)
    }
    fn name(&self) -> String {
        "Ollama in Codex".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Tested
    }
    /// rush's own CODEX_HOME for Ollama, when ollama and codex are both
    /// here.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg.installed(CODEX_KIND) {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(CODEX_KIND),
            name: "Ollama in Codex".into(),
            dir: self.home(),
        }]
    }
    rush_core::has!(
        programmer: Programmer,
        rider: Rider,
        provided: Provided,
        pricer: Pricer,
        driver: Driver,
        discoverer: Discoverer,
        history_reader: HistoryReader,
        tail_reader: TailReader,
        history_follower: HistoryFollower,
        media_reader: MediaReader,
        model_lister: ModelLister,
        context_windower: ContextWindower,
        model_namer: ModelNamer,
    );
}

impl Programmer for OllamaCodex {
    /// ollama, which it needs beside Codex.
    fn program(&self) -> (String, Vec<String>) {
        crate::claude::program()
    }
}

impl Rider for OllamaCodex {
    /// Codex: its sessions are codex's threads.
    fn rides(&self) -> Kind {
        Kind::from(CODEX)
    }
}

impl Provided for OllamaCodex {
    /// Ollama, whichever harness it runs in.
    fn provider(&self) -> String {
        KIND.into()
    }
}

impl Pricer for OllamaCodex {
    /// Nothing: the model runs here.
    fn cost(&self, _model: &str, _u: &TokenUsage) -> Option<f64> {
        Some(0.0)
    }
}

impl Driver for OllamaCodex {
    /// Picks and loads the model, then runs Codex on it.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        let h = driver_of(&self.reg, CODEX)?;
        let m = self.local.pick(&o.model)?;
        o.flags = codex_flags(&m, &self.local.server(), &o.flags);
        o.model = m.name;
        o.effort.clear();
        if o.profile.dir.is_empty() {
            o.profile.dir = self.home();
        }
        // Codex refuses a CODEX_HOME that isn't there, and nothing else
        // makes it.
        crate::pi::make_private_dir(std::path::Path::new(&o.profile.dir))?;
        // The host hands over the adapter's own program, which is
        // ollama's, unless it was told of a codex to run.
        o.binary = harness_binary(&self.reg, &o.binary, CODEX);
        h.driver().expect("checked").start(o, sink)
    }
}

impl Discoverer for OllamaCodex {
    /// The threads Codex wrote to lately in rush's folder.
    fn live(&self, p: &Profile) -> Vec<Session> {
        let ss = self
            .codex()
            .and_then(|a| a.discoverer().map(|d| d.live(p)))
            .unwrap_or_default();
        ours(ss, CODEX_KIND)
    }
    /// Every thread in rush's folder.
    fn past(&self, p: &Profile) -> Vec<Session> {
        let ss = self
            .codex()
            .and_then(|a| a.discoverer().map(|d| d.past(p)))
            .unwrap_or_default();
        ours(ss, CODEX_KIND)
    }
}

fn no_history() -> rush_core::adapter::Error {
    err(format!("rush can't read {CODEX}'s history"))
}

impl HistoryReader for OllamaCodex {
    /// A thread's rollout read back, as Codex's own are.
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> Result<()> {
        let a = self.codex().ok_or_else(no_history)?;
        let r = a.history_reader().ok_or_else(no_history)?;
        r.history(s, before, most, each)
    }
}

impl TailReader for OllamaCodex {
    fn history_tail(&self, s: &Session, most: i64) -> Result<(Vec<Event>, bool)> {
        let a = self.codex().ok_or_else(no_history)?;
        let r = a.tail_reader().ok_or_else(no_history)?;
        r.history_tail(s, most)
    }
}

impl HistoryFollower for OllamaCodex {
    fn follow_history(&self, s: &Session) -> Result<Box<dyn Follow>> {
        let a = self.codex().ok_or_else(no_history)?;
        let r = a.history_follower().ok_or_else(no_history)?;
        r.follow_history(s)
    }
}

impl MediaReader for OllamaCodex {
    fn reads(&self, model: &str) -> Option<Media> {
        self.local.reads(model)
    }
}

impl ModelLister for OllamaCodex {
    fn list_models(&self, _p: &Profile) -> Vec<Choice> {
        self.local.list_models()
    }
}

impl ContextWindower for OllamaCodex {
    fn context_window(&self, model: &str) -> i64 {
        self.local.cached_window(model)
    }
}

impl ModelNamer for OllamaCodex {
    fn model_name(&self, id: &str) -> String {
        manage::model_name(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_point_at_ollama() {
        let m = Model {
            context: 32768,
            ..Model::named("qwen2.5:1.5b", &["tools"])
        };
        let flags = codex_flags(
            &m,
            "http://127.0.0.1:11434",
            &["-c".into(), "model=mine".into()],
        );
        for want in [
            r#"model_provider="rush-ollama""#,
            r#"model_providers.rush-ollama={name="Ollama",base_url="http://127.0.0.1:11434/v1",wire_api="responses"}"#,
            r#"model="qwen2.5:1.5b""#,
            "model_context_window=32768",
        ] {
            assert!(flags.iter().any(|f| f == want), "no -c {want} in {flags:?}");
        }
        // The caller's own come last, so theirs win.
        assert_eq!(flags.last().unwrap(), "model=mine");
        let f = codex_flags(&Model::named("x", &[]), "http://h:1", &[]);
        assert!(
            !f.iter().any(|s| s.starts_with("model_context_window")),
            "a window given when none is known: {f:?}"
        );
    }

    #[test]
    fn is_ollama_in_codex() {
        let reg = Registry::new();
        crate::register(&reg);
        assert_eq!(reg.harness_of(CODEX_KIND).as_str(), CODEX);
        assert_eq!(reg.provider_of(CODEX_KIND), KIND);
        assert!(reg.harnesses(KIND).iter().any(|k| k.as_str() == CODEX_KIND));
        let ss = ours(
            vec![Session {
                id: "t".into(),
                kind: Kind::from(CODEX),
                profile: Profile {
                    kind: Kind::from(CODEX),
                    ..Profile::default()
                },
                ..Session::default()
            }],
            CODEX_KIND,
        );
        assert_eq!(ss[0].kind.as_str(), CODEX_KIND);
        assert_eq!(ss[0].profile.kind.as_str(), CODEX_KIND);
        // Codex isn't registered: no threads, and no history to read.
        let a = reg.get(CODEX_KIND).unwrap();
        assert!(a.discoverer().unwrap().past(&Profile::default()).is_empty());
        let e = a
            .history_follower()
            .unwrap()
            .follow_history(&Session::default())
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "rush can't read codex's history");
    }

    #[test]
    fn quotes_as_go() {
        assert_eq!(quote("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }
}
