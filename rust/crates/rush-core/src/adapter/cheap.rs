//! Asking the cheapest model rush can run without a session. After Go's
//! internal/agent/cheap.go:1-34 (AskCheap); a plugin's models, tried first,
//! are new in the Rust port.

use super::{LEGACY_KIND, Registry, Result};

/// The helper adapter whose models are the ones plugins offer for rush's
/// small jobs, each named "<plugin>/<model>".
pub const PLUGIN_MODELS: &str = "plugin-models";

/// Whether prefer asks for a stronger model than a small job's: Sonnet
/// (the sidekick, a long last message) or Opus. A plugin's model isn't
/// tried for those.
fn wants_strong(prefer: &str) -> bool {
    prefer.contains("sonnet") || prefer.contains("opus")
}

/// Asks the cheapest model rush can run without a session, the first of
/// its models whose name has prefer when one does, and says which it
/// asked: (model, answer). A plugin's model goes first, unless prefer asks
/// for a stronger one, then Claude Code's, then Ollama's; one that fails
/// hands on to the next, and the last failure is what's said when none
/// answers.
pub fn ask_cheap(
    reg: &Registry,
    system: &str,
    prompt: &str,
    prefer: &str,
) -> Result<(String, String)> {
    let mut last = None;
    if !wants_strong(prefer) {
        match ask_plugin_models(reg, system, &|_| prompt.to_string()) {
            Ok(r) => return Ok(r),
            Err(e) => last = e,
        }
    }
    for k in [LEGACY_KIND, "ollama"] {
        let Some(a) = reg.get(k) else { continue };
        let Some(sm) = a.summarizer() else { continue };
        if !reg.installed(k) {
            continue;
        }
        let Ok(ms) = sm.summary_models() else {
            continue;
        };
        let Some(pick) = ms
            .iter()
            .find(|x| !prefer.is_empty() && x.contains(prefer))
            .or(ms.first())
        else {
            continue;
        };
        match sm.summarize(pick, system, prompt) {
            Ok(out) => return Ok((pick.clone(), out)),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        "no model to ask: it runs on Claude Code, Ollama or a plugin's model".into()
    }))
}

/// Tokens kept free of a small model's context: the plugin's room for the
/// answer (apple-intelligence keeps 512) and the prompt's wrapping.
const ANSWER_ROOM: usize = 640;

/// How many tokens of a message fit a model holding context tokens, with
/// system's instructions beside it: none when they leave no room.
pub fn message_room(context: usize, system: &str) -> usize {
    // Over-counting the instructions (3 characters a token) keeps it
    // inside, as fit_tokens under-counts the message.
    context.saturating_sub(ANSWER_ROOM + system.len().div_ceil(3))
}

/// Asks the models plugins offer, each in turn, until one answers:
/// (model, answer). prompt makes what's sent from the tokens of message a
/// model has room for, None when it doesn't say its size. Err(None) when
/// there are none to ask, else the last one's failure ("too small" for a
/// prompt it can't hold, say).
pub fn ask_plugin_models(
    reg: &Registry,
    system: &str,
    prompt: &dyn Fn(Option<usize>) -> String,
) -> std::result::Result<(String, String), Option<super::Error>> {
    let Some(a) = reg.get(PLUGIN_MODELS) else {
        return Err(None);
    };
    let Some(sm) = a.summarizer() else {
        return Err(None);
    };
    if !reg.installed(PLUGIN_MODELS) {
        return Err(None);
    }
    let ms = sm.summary_models().map_err(Some)?;
    let mut last = None;
    for m in &ms {
        let room = sm.context_tokens(m).map(|n| message_room(n, system));
        if room == Some(0) {
            last = Some(format!("{m} is too small for these instructions").into());
            continue;
        }
        match sm.summarize(m, system, &prompt(room)) {
            Ok(out) => return Ok((m.clone(), out)),
            Err(e) => last = Some(e),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{Adapter, Features, Kind, Level, Profile, Summarizer};
    use std::sync::Mutex;

    // With neither Claude Code nor Ollama, it says what it needs.
    #[test]
    fn no_model_says_so() {
        let e = ask_cheap(&Registry::new(), "s", "p", "haiku").unwrap_err();
        assert_eq!(
            e.to_string(),
            "no model to ask: it runs on Claude Code, Ollama or a plugin's model"
        );
    }

    /// An adapter whose one model answers with answer, or fails with it
    /// when it starts with "!"; it notes who was asked.
    struct One {
        kind: &'static str,
        helper: bool,
        model: &'static str,
        answer: &'static str,
        asked: &'static Mutex<Vec<String>>,
    }

    impl Adapter for One {
        fn kind(&self) -> Kind {
            Kind::from(self.kind)
        }
        fn name(&self) -> String {
            self.kind.into()
        }
        fn features(&self) -> Features {
            Features::default()
        }
        fn level(&self) -> Level {
            Level::Preview
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        fn helper(&self) -> bool {
            self.helper
        }
        fn summarizer(&self) -> Option<&dyn Summarizer> {
            Some(self)
        }
    }

    impl Summarizer for One {
        fn summary_models(&self) -> Result<Vec<String>> {
            Ok(vec![self.model.into()])
        }
        fn summarize(&self, model: &str, _: &str, _: &str) -> Result<String> {
            self.asked.lock().unwrap().push(model.into());
            match self.answer.strip_prefix('!') {
                Some(e) => Err(e.into()),
                None => Ok(self.answer.into()),
            }
        }
    }

    fn reg(
        plugin: &'static str,
        claude: &'static str,
        asked: &'static Mutex<Vec<String>>,
    ) -> Registry {
        let r = Registry::new();
        r.register(One {
            kind: PLUGIN_MODELS,
            helper: true,
            model: "ai/on-device",
            answer: plugin,
            asked,
        });
        r.register(One {
            kind: LEGACY_KIND,
            helper: false,
            model: "claude-haiku",
            answer: claude,
            asked,
        });
        r
    }

    // A plugin's model goes first; one that fails hands on to Claude's;
    // the sidekick's Sonnet skips it; with nothing else, its own failure
    // is what's said, so quick ask can try again without its guide.
    #[test]
    fn plugin_models_first_then_fall_through() {
        static A: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let r = reg("from the plugin", "from claude", &A);
        assert_eq!(
            ask_cheap(&r, "s", "p", "haiku").unwrap(),
            ("ai/on-device".into(), "from the plugin".into())
        );
        assert_eq!(ask_cheap(&r, "s", "p", "sonnet").unwrap().1, "from claude");
        assert_eq!(*A.lock().unwrap(), ["ai/on-device", "claude-haiku"]);

        static B: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let r = reg(
            "!the input is too big: the context is too small",
            "from claude",
            &B,
        );
        assert_eq!(ask_cheap(&r, "s", "p", "").unwrap().1, "from claude");

        static C: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let r = reg("!context too small", "!claude failed", &C);
        assert_eq!(
            ask_cheap(&r, "s", "p", "").unwrap_err().to_string(),
            "claude failed"
        );
        r.unregister(LEGACY_KIND);
        assert_eq!(
            ask_cheap(&r, "s", "p", "").unwrap_err().to_string(),
            "context too small"
        );
    }

    /// A plugin model holding 4096 tokens that answers with how long the
    /// prompt it was sent is.
    struct Small;

    impl Adapter for Small {
        fn kind(&self) -> Kind {
            Kind::from(PLUGIN_MODELS)
        }
        fn name(&self) -> String {
            "small".into()
        }
        fn features(&self) -> Features {
            Features::default()
        }
        fn level(&self) -> Level {
            Level::Preview
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        fn helper(&self) -> bool {
            true
        }
        fn summarizer(&self) -> Option<&dyn Summarizer> {
            Some(self)
        }
    }

    impl Summarizer for Small {
        fn summary_models(&self) -> Result<Vec<String>> {
            Ok(vec!["ai/on-device".into()])
        }
        fn summarize(&self, _: &str, _: &str, text: &str) -> Result<String> {
            Ok(text.len().to_string())
        }
        fn context_tokens(&self, _: &str) -> Option<usize> {
            Some(4096)
        }
    }

    // A small model is sent the message cut to the room its size leaves
    // beside the instructions; one whose instructions fill it isn't asked.
    #[test]
    fn plugin_models_are_sent_what_fits() {
        let r = Registry::new();
        r.register(Small);
        let system = "s".repeat(300);
        let room = message_room(4096, &system);
        assert_eq!(room, 4096 - 640 - 100);
        let long = "word ".repeat(20_000);
        let (_, n) = ask_plugin_models(&r, &system, &|room| {
            crate::adapter::fit_tokens(&long, room.unwrap_or(8000))
        })
        .unwrap();
        // fit_tokens keeps about 3 characters a token, plus its note.
        let n: usize = n.parse().unwrap();
        assert!(n <= room * 3 + 80, "{n} characters for {room} tokens");
        assert!(n > room * 2, "{n} characters is cut too far");

        let full = "s".repeat(4096 * 3);
        let e = ask_plugin_models(&r, &full, &|_| "p".into()).unwrap_err();
        assert!(e.unwrap().to_string().contains("too small"));
    }
}
