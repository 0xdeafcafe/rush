//! Pi on Ollama: `pi --mode rpc`, as the pi adapter drives it, in a Pi
//! config folder of rush's own whose models.json points a provider named
//! "ollama" at Ollama's OpenAI-compatible API, so its sessions never mix
//! with the user's ~/.pi, nor read their keys.
//!
//! Ported from Go's internal/adapters/ollama/pi.go:1-212. Its sessions and
//! their history are Pi's, read through the pi adapter in the registry.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use rush_core::adapter::{
    Adapter, Choice, Conn, ContextWindower, Discoverer, Driver, Feature, Features, HistoryReader,
    Kind, Level, Media, MediaReader, ModelLister, ModelNamer, Orphans, Pricer, Profile, Programmer,
    Provided, Registry, Result, Rider, Session, Sink, StartOptions, Support,
};
use rush_core::agent::TokenUsage;
use rush_core::event::Event;
use rush_core::time::Ms;

use crate::api::Model;
use crate::{KIND, Local, driver_of, err, harness_binary, manage, ours, tune};

/// Ollama in Pi's kind.
pub const PI_KIND: &str = "ollama-pi";

/// Pi's kind, which it rides.
const PI: &str = "pi";

/// The provider rush names in Pi's models.json.
pub(crate) const PI_PROVIDER: &str = "ollama";

/// Pi on Ollama.
#[derive(Clone)]
pub struct OllamaPi {
    reg: Registry,
    local: Arc<Local>,
}

impl OllamaPi {
    pub fn new(reg: Registry, local: Arc<Local>) -> OllamaPi {
        OllamaPi { reg, local }
    }

    /// The PI_CODING_AGENT_DIR sessions on Ollama keep to.
    pub fn home(&self) -> String {
        self.local
            .state_dir()
            .join(PI_KIND)
            .to_string_lossy()
            .into_owned()
    }

    fn pi(&self) -> Option<rush_core::adapter::AdapterRef> {
        self.reg.get(PI)
    }
}

impl Local {
    /// The models a harness is told of: m, picked and loaded, first, then
    /// every other one Ollama has that calls tools, for a switch
    /// mid-session.
    pub(crate) fn tool_models(&self, m: &Model) -> Vec<Model> {
        let mut out = vec![m.clone()];
        // Ten seconds all told, as Go's one context.
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let Ok(all) = self.names(crate::tune::left(until)) else {
            return out;
        };
        for n in all {
            if n == m.name {
                continue;
            }
            if let Ok(o) = self.show(&n, crate::tune::left(until))
                && o.can_code()
            {
                out.push(o);
            }
        }
        out
    }
}

/// One of models.json's models.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PiModel {
    id: String,
    name: String,
    reasoning: bool,
    input: Vec<&'static str>,
    #[serde(skip_serializing_if = "is_zero")]
    context_window: i64,
    #[serde(skip_serializing_if = "is_zero")]
    max_tokens: i64,
    cost: Value,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Pi's models.json with one provider, Ollama served at base, and the
/// models given. Ollama's OpenAI API takes neither the developer role nor
/// reasoning_effort, so the system prompt goes as a system message and a
/// model thinks as it was made to. Each model's window is the one it's
/// loaded with, else the one it was trained to, so Pi compacts in time; it
/// may write a quarter of it.
pub fn pi_config(models: &[Model], base: &str) -> Value {
    let list: Vec<PiModel> = models
        .iter()
        .map(|m| {
            let mut input = vec!["text"];
            if m.can("vision") {
                input.push("image");
            }
            let n = tune::window(m);
            PiModel {
                id: m.name.clone(),
                name: m.name.clone(),
                reasoning: m.can("thinking"),
                input,
                context_window: n.max(0),
                max_tokens: if n > 0 { 32000.min(n / 4) } else { 0 },
                cost: json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}),
            }
        })
        .collect();
    json!({"providers": {PI_PROVIDER: {
        "baseUrl": format!("{base}/v1"), "api": "openai-completions", "apiKey": "ollama",
        "compat": {"supportsDeveloperRole": false, "supportsReasoningEffort": false},
        "models": list,
    }}})
}

/// Makes dir, only you able to read it, if it isn't there.
pub(crate) fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// Writes b to dir/name through a temporary file only you can read.
pub(crate) fn write_private(dir: &Path, name: &str, b: &[u8]) -> std::io::Result<()> {
    make_private_dir(dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(&tmp)?.write_all(b)?;
    }
    std::fs::rename(tmp, dir.join(name))
}

/// Writes models.json into Pi's folder dir.
pub(crate) fn write_pi_models(dir: &Path, models: &[Model], base: &str) -> std::io::Result<()> {
    let mut b =
        serde_json::to_vec_pretty(&pi_config(models, base)).map_err(std::io::Error::other)?;
    b.push(b'\n');
    write_private(dir, "models.json", &b)
}

/// Pi's features, less what a local model or rush's own folder hasn't.
fn features() -> Features {
    use Feature::*;
    let yes = Support::YES;
    let no = Support::NO;
    Features::from([
        (Run, yes.clone()),
        (Prompt, yes.clone()),
        (Resume, yes.clone()),
        (
            Fork,
            yes.clone().with("the whole session, not from a message"),
        ),
        (Interrupt, yes.clone()),
        (
            Model,
            yes.clone().with("any model Ollama has that calls tools"),
        ),
        (Images, yes.clone().with("on models that take them")),
        (
            Compact,
            yes.clone().with("at the window the model was loaded with"),
        ),
        (
            Questions,
            yes.clone().with("an extension's select and confirm"),
        ),
        (
            Context,
            yes.clone().with("how full it is, not what fills it"),
        ),
        (HandoffIn, yes.clone()),
        (
            Live,
            yes.clone()
                .with("sessions written to in the last two minutes"),
        ),
        (History, yes.clone()),
        (Pricing, yes.with("free: it runs on this machine")),
        (
            Effort,
            no.clone().with("a model thinks or doesn't, as it was made"),
        ),
        (
            Modes,
            no.clone().with("Pi asks before nothing: every tool runs"),
        ),
        (Quota, no.clone().with("no limits but this machine's")),
        (Switch, no.clone().with("no accounts, only this machine")),
        (SignIn, no.with("nothing to sign in to")),
    ])
}

impl Adapter for OllamaPi {
    fn kind(&self) -> Kind {
        Kind::from(PI_KIND)
    }
    fn name(&self) -> String {
        "Ollama in Pi".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Tested
    }
    /// rush's own Pi folder for Ollama, when ollama and pi are both here.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg.installed(PI_KIND) {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(PI_KIND),
            name: "Ollama in Pi".into(),
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
        orphans: Orphans,
        media_reader: MediaReader,
        model_lister: ModelLister,
        context_windower: ContextWindower,
        model_namer: ModelNamer,
    );
}

impl Programmer for OllamaPi {
    /// ollama, which it needs beside Pi.
    fn program(&self) -> (String, Vec<String>) {
        crate::claude::program()
    }
}

impl Rider for OllamaPi {
    /// Pi: its sessions are pi's.
    fn rides(&self) -> Kind {
        Kind::from(PI)
    }
}

impl Provided for OllamaPi {
    /// Ollama, whichever harness it runs in.
    fn provider(&self) -> String {
        KIND.into()
    }
}

impl Pricer for OllamaPi {
    /// Nothing: the model runs here.
    fn cost(&self, _model: &str, _u: &TokenUsage) -> Option<f64> {
        Some(0.0)
    }
}

impl Driver for OllamaPi {
    /// Picks and loads the model, writes Pi's models.json for it, then
    /// runs Pi on it.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        let h = driver_of(&self.reg, PI)?;
        let want = o
            .model
            .strip_prefix(&format!("{PI_PROVIDER}/"))
            .unwrap_or(&o.model)
            .to_string();
        let m = self.local.pick(&want)?;
        if o.profile.dir.is_empty() {
            o.profile.dir = self.home();
        }
        let models = self.local.tool_models(&m);
        write_pi_models(Path::new(&o.profile.dir), &models, &self.local.server())?;
        o.model = m.name;
        o.effort.clear();
        o.flags
            .splice(0..0, ["--provider".to_string(), PI_PROVIDER.to_string()]);
        // The host hands over the adapter's own program, which is
        // ollama's, unless it was told of a pi to run.
        o.binary = harness_binary(&self.reg, &o.binary, PI);
        h.driver().expect("checked").start(o, sink)
    }
}

impl Discoverer for OllamaPi {
    /// The sessions Pi wrote to lately in rush's folder.
    fn live(&self, p: &Profile) -> Vec<Session> {
        let ss = self
            .pi()
            .and_then(|a| a.discoverer().map(|d| d.live(p)))
            .unwrap_or_default();
        ours(ss, PI_KIND)
    }
    /// Every session in rush's folder.
    fn past(&self, p: &Profile) -> Vec<Session> {
        let ss = self
            .pi()
            .and_then(|a| a.discoverer().map(|d| d.past(p)))
            .unwrap_or_default();
        ours(ss, PI_KIND)
    }
}

impl HistoryReader for OllamaPi {
    /// A session read back, as Pi's own are.
    fn history(
        &self,
        s: &Session,
        before: Ms,
        most: usize,
        each: &mut dyn FnMut(Vec<Event>) -> bool,
    ) -> Result<()> {
        let no = || err(format!("rush can't read {PI}'s history"));
        let a = self.pi().ok_or_else(no)?;
        let r = a.history_reader().ok_or_else(no)?;
        r.history(s, before, most, each)
    }
}

impl Orphans for OllamaPi {
    /// Its sessions are pi's.
    fn runs_session(&self, args: &[String], session_id: &str) -> bool {
        self.pi()
            .and_then(|a| a.orphans().map(|o| o.runs_session(args, session_id)))
            .unwrap_or(false)
    }
}

impl MediaReader for OllamaPi {
    fn reads(&self, model: &str) -> Option<Media> {
        self.local.reads(model)
    }
}

impl ModelLister for OllamaPi {
    fn list_models(&self, _p: &Profile) -> Vec<Choice> {
        self.local.list_models()
    }
}

impl ContextWindower for OllamaPi {
    fn context_window(&self, model: &str) -> i64 {
        self.local
            .cached_window(model.strip_prefix("ollama/").unwrap_or(model))
    }
}

impl ModelNamer for OllamaPi {
    fn model_name(&self, id: &str) -> String {
        manage::model_name(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_point_at_ollama() {
        let dir = tempfile::tempdir().unwrap();
        let models = [
            Model {
                max_context: 262144,
                context: 65536,
                ..Model::named("qwen3.8:27b-mlx", &["tools", "thinking", "vision"])
            },
            Model::named("qwen2.5:1.5b", &["tools"]),
        ];
        write_pi_models(dir.path(), &models, "http://127.0.0.1:11434").unwrap();
        let f = dir.path().join("models.json");
        let got: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        let p = &got["providers"]["ollama"];
        assert_eq!(p["baseUrl"], "http://127.0.0.1:11434/v1");
        assert_eq!(p["api"], "openai-completions");
        assert!(!p["apiKey"].as_str().unwrap().is_empty());
        assert_eq!(p["compat"]["supportsDeveloperRole"], false);
        assert_eq!(p["compat"]["supportsReasoningEffort"], false);
        let ms = p["models"].as_array().unwrap();
        assert_eq!(ms.len(), 2);
        let first = &ms[0];
        assert_eq!(first["id"], "qwen3.8:27b-mlx");
        assert_eq!(first["reasoning"], true);
        assert_eq!(first["contextWindow"], 65536);
        assert_eq!(first["maxTokens"], 16384);
        assert!(first["input"].as_array().unwrap().contains(&json!("image")));
        let small = &ms[1];
        assert_eq!(small["reasoning"], false);
        assert!(small.get("contextWindow").is_none());
        assert!(!small["input"].as_array().unwrap().contains(&json!("image")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn is_ollama_in_pi() {
        let reg = Registry::new();
        crate::register(&reg);
        assert_eq!(reg.harness_of(PI_KIND).as_str(), PI);
        assert_eq!(reg.provider_of(PI_KIND), KIND);
        assert!(reg.harnesses(KIND).iter().any(|k| k.as_str() == PI_KIND));
        assert_eq!(
            reg.program_of(PI_KIND),
            "",
            "a shell running ollama would be taken for Ollama in Pi"
        );
        assert_eq!(reg.label(PI_KIND), "Ollama (pi)");
    }
}
