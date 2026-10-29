//! Claude Code on Ollama: the "ollama" adapter. Claude Code runs on
//! Ollama's Anthropic-compatible API, fitted to the model it runs, in a
//! Claude config folder of rush's own.
//!
//! Ported from Go's internal/adapters/ollama/ollama.go:1-132, with Claude
//! Code's attribution settings (Go's claude/attribution.go:23-57), read in
//! the folder of its own.

use std::path::Path;
use std::sync::Arc;

use rush_core::adapter::{
    Adapter, Attributor, Choice, Conn, ContextWindower, Driver, Feature, Features, Kind, Level,
    Media, MediaReader, ModelLister, ModelNamer, Pricer, Profile, Programmer, Registry, Result,
    Rider, Sink, StartOptions, Summarizer, Support,
};
use rush_core::agent::TokenUsage;

use crate::{Local, driver_of, harness_binary, manage, tune};

/// Ollama's kind.
pub const KIND: &str = "ollama";

/// Claude Code's kind, which it rides.
const CLAUDE: &str = "claude";

/// Claude Code on Ollama.
#[derive(Clone)]
pub struct Ollama {
    reg: Registry,
    local: Arc<Local>,
}

impl Ollama {
    pub fn new(reg: Registry, local: Arc<Local>) -> Ollama {
        Ollama { reg, local }
    }

    /// The Claude config folder sessions on Ollama keep to.
    pub fn home(&self) -> String {
        self.local
            .state_dir()
            .join("ollama")
            .to_string_lossy()
            .into_owned()
    }

    /// Links each skill Claude Code has in cwd into dir's skills folder,
    /// where Claude Code on Ollama reads them; links from before go first,
    /// so a skill removed since goes too. The first of a name wins.
    fn link_skills(&self, dir: &str, cwd: &str) {
        let skills = Path::new(dir).join("skills");
        if let Ok(old) = std::fs::read_dir(&skills) {
            for e in old.flatten() {
                if e.file_type().is_ok_and(|t| t.is_symlink()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let Some(claude) = self.reg.get(CLAUDE) else {
            return;
        };
        let Some(roots) = claude.skill_rooter() else {
            return;
        };
        let ps = self.reg.profiles_of(&claude);
        if ps.is_empty() || std::fs::create_dir_all(&skills).is_err() {
            return;
        }
        for root in roots.skill_roots(&ps[0], cwd) {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            let mut found: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join("SKILL.md").exists())
                .collect();
            found.sort();
            for d in found {
                if let Some(name) = d.file_name() {
                    #[cfg(unix)]
                    let _ = std::os::unix::fs::symlink(&d, skills.join(name));
                }
            }
        }
    }
}

/// Claude Code's features, less what a local model can't do or its
/// separate folder hides: its sessions aren't in ~/.claude for fleet to
/// find, and there are no limits or accounts, only this machine.
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
        (Guide, yes.clone().with("at its next step")),
        (
            Model,
            yes.clone().with("any model Ollama has that calls tools"),
        ),
        (Modes, yes.clone()),
        (
            Compact,
            yes.clone().with("at the window the model was loaded with"),
        ),
        (Images, yes.clone().with("on models that take them")),
        (HandoffIn, yes.clone()),
        (Pricing, yes.with("free: it runs on this machine")),
        (
            Effort,
            Support::NO.with("a model thinks or doesn't, as it was made"),
        ),
        (
            Subagents,
            Support::NO.with("left out to keep the prompt small"),
        ),
        (Quota, Support::NO.with("no limits but this machine's")),
    ])
}

impl Adapter for Ollama {
    fn kind(&self) -> Kind {
        Kind::from(KIND)
    }
    fn name(&self) -> String {
        "Ollama".into()
    }
    fn features(&self) -> Features {
        features()
    }
    fn level(&self) -> Level {
        Level::Tested
    }
    /// rush's own Claude folder for Ollama, when ollama and Claude Code are
    /// both here.
    fn profiles(&self) -> Vec<Profile> {
        if !self.reg.installed(KIND) {
            return Vec::new();
        }
        vec![Profile {
            kind: Kind::from(KIND),
            name: "Ollama".into(),
            dir: self.home(),
        }]
    }
    rush_core::has!(
        programmer: Programmer,
        rider: Rider,
        attributor: Attributor,
        pricer: Pricer,
        driver: Driver,
        media_reader: MediaReader,
        model_lister: ModelLister,
        context_windower: ContextWindower,
        model_namer: ModelNamer,
        summarizer: Summarizer,
    );
}

impl Programmer for Ollama {
    /// ollama, which it needs beside Claude Code.
    fn program(&self) -> (String, Vec<String>) {
        program()
    }
}

/// ollama, and where its app keeps it.
pub(crate) fn program() -> (String, Vec<String>) {
    (
        "ollama".into(),
        vec!["/Applications/Ollama.app/Contents/Resources".into()],
    )
}

impl Rider for Ollama {
    /// Claude Code: its sessions are claude's.
    fn rides(&self) -> Kind {
        Kind::from(CLAUDE)
    }
}

impl Pricer for Ollama {
    /// Nothing: the model runs here.
    fn cost(&self, _model: &str, _u: &TokenUsage) -> Option<f64> {
        Some(0.0)
    }
}

impl Attributor for Ollama {
    /// Whether Claude Code on Ollama credits itself: its settings are its
    /// own, not ~/.claude's.
    fn attribution(&self, p: &Profile) -> (bool, bool) {
        attribution_in(Path::new(&p.dir))
    }
    fn set_attribution(&self, p: &Profile, on: bool) -> Result<()> {
        set_attribution_in(Path::new(&p.dir), on)
    }
}

/// Whether Claude Code credits itself, by dir's settings.json: its
/// attribution texts, else includeCoAuthoredBy (Go's claude.AttributionIn).
pub fn attribution_in(dir: &Path) -> (bool, bool) {
    let Ok(s) = rush_state::settingsfile::File::load(dir.join("settings.json")) else {
        return (false, false);
    };
    let commit = s.get::<String>("attribution.commit");
    let pr = s.get::<String>("attribution.pr");
    if commit.is_some() || pr.is_some() {
        let on = commit.is_some_and(|c| !c.is_empty()) || pr.is_some_and(|p| !p.is_empty());
        return (on, true);
    }
    match s.get::<bool>("includeCoAuthoredBy") {
        Some(co) => (co, true),
        None => (false, false),
    }
}

/// Says in dir's settings whether Claude Code credits itself: off empties
/// its texts, on removes them so it adds its own.
pub fn set_attribution_in(dir: &Path, on: bool) -> Result<()> {
    let mut s = rush_state::settingsfile::File::load(dir.join("settings.json"))?;
    if on {
        s.delete("attribution.commit");
        s.delete("attribution.pr");
        s.delete("includeCoAuthoredBy");
    } else {
        s.set("attribution.commit", "")?;
        s.set("attribution.pr", "")?;
        s.set("includeCoAuthoredBy", &false)?;
    }
    s.save()?;
    Ok(())
}

impl Driver for Ollama {
    /// Picks and loads the model, then runs Claude Code on it.
    fn start(&self, mut o: StartOptions, sink: Arc<dyn Sink>) -> Result<Box<dyn Conn>> {
        let h = driver_of(&self.reg, CLAUDE)?;
        let m = self.local.pick(&o.model)?;
        let (env, flags) = tune::tune(&m, &self.local.server(), &o.flags);
        o.env.extend(env);
        o.flags = flags;
        o.model = m.name.clone();
        o.effort.clear();
        if o.profile.dir.is_empty() {
            o.profile.dir = self.home();
        }
        if tune::skilled(&m) {
            self.link_skills(&o.profile.dir, &o.dir);
        }
        // The host hands over the adapter's own program, which is
        // ollama's, unless it was told of a claude to run.
        o.binary = harness_binary(&self.reg, &o.binary, CLAUDE);
        h.driver().expect("checked").start(o, sink)
    }
}

impl MediaReader for Ollama {
    fn reads(&self, model: &str) -> Option<Media> {
        self.local.reads(model)
    }
}

impl ModelLister for Ollama {
    fn list_models(&self, _p: &Profile) -> Vec<Choice> {
        self.local.list_models()
    }
}

impl ContextWindower for Ollama {
    fn context_window(&self, model: &str) -> i64 {
        self.local.cached_window(model)
    }
}

impl ModelNamer for Ollama {
    fn model_name(&self, id: &str) -> String {
        manage::model_name(id)
    }
}

impl Summarizer for Ollama {
    fn summary_models(&self) -> Result<Vec<String>> {
        self.local.summary_models()
    }
    fn summarize(&self, model: &str, system: &str, text: &str) -> Result<String> {
        self.local.summarize(model, system, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_in_its_own_folder() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(attribution_in(d.path()), (false, false));
        set_attribution_in(d.path(), false).unwrap();
        assert_eq!(attribution_in(d.path()), (false, true));
        set_attribution_in(d.path(), true).unwrap();
        assert_eq!(attribution_in(d.path()), (false, false));
        std::fs::write(
            d.path().join("settings.json"),
            r#"{"includeCoAuthoredBy":true}"#,
        )
        .unwrap();
        assert_eq!(attribution_in(d.path()), (true, true));
    }

    // A shell running ollama isn't an agent starting: `ollama pull` is
    // only a download.
    #[test]
    fn ollama_is_no_agents_program() {
        let reg = Registry::new();
        crate::register(&reg);
        assert_eq!(reg.program_kind("/opt/homebrew/bin/ollama"), None);
        assert!(!reg.is_program("ollama"));
        assert_eq!(reg.harness_of(KIND).as_str(), "claude");
        assert_eq!(reg.price(KIND, "any", &TokenUsage::default()), Some(0.0));
    }

    // Claude Code isn't registered here: nothing runs a session on it, and
    // the server isn't asked before that's known.
    #[test]
    fn starts_only_through_claude_code() {
        let d = tempfile::tempdir().unwrap();
        let l = Local::with(
            Registry::new(),
            Some("http://192.0.2.1:1"),
            Some(d.path().into()),
            None,
        );
        let o = Ollama::new(Registry::new(), l);
        let e = o
            .start(StartOptions::default(), Arc::new(crate::testsink::Nothing))
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "rush can't run claude");
        assert_eq!(o.home(), d.path().join("ollama").to_string_lossy());
    }
}
