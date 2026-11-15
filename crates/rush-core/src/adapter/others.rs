//! Every model rush can run, offered to a session as its own harness's
//! subagent types: what an adapter is handed to register them (OtherAgent)
//! and to answer a call for one (Delegate), and the tiers a model sits in.
//! See docs/native-agents.md.
//!
//! Rust-only: Go's sessions reached other agents through spawn_agent and
//! their shell alone.

use std::sync::Arc;

/// One model rush offers a session as one of its harness's own subagent
/// types: by its plain name ("kimi", "astra", "top"), with what it's good
/// for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OtherAgent {
    pub name: String,
    pub description: String,
}

/// A call for one of the OtherAgents, as the harness made it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DelegateCall {
    /// The type named: a model ("kimi") or a tier ("quick long-context").
    pub agent: String,
    /// The task, whole.
    pub prompt: String,
    /// Return at once; the answer comes as a message when it's done.
    pub background: bool,
    /// The harness's id for the call, so rush draws the run under it.
    pub step: String,
}

/// Runs an OtherAgent for a session: rush starts the model on its own
/// harness as the session's child and gives back its final answer (or,
/// in the background, its id and how its answer will come). It blocks
/// until then; an adapter calls it off its reader thread.
pub trait Delegate: Send + Sync {
    fn run(&self, call: DelegateCall) -> Result<String, String>;
}

/// A Delegate as StartOptions holds it.
pub type DelegateRef = Arc<dyn Delegate>;

/// How capable and costly a model is, for picking one by the work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Hard reasoning, design, final review.
    Top,
    /// Most coding.
    #[default]
    Workhorse,
    /// Search, summaries, mechanical edits.
    Quick,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Top, Tier::Workhorse, Tier::Quick];

    /// Its name, as an agent names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Top => "top",
            Tier::Workhorse => "workhorse",
            Tier::Quick => "quick",
        }
    }

    /// The tier a word names; None for any other word.
    pub fn parse(s: &str) -> Option<Tier> {
        Tier::ALL
            .into_iter()
            .find(|t| t.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// What it's for, in a few words.
    pub fn what(self) -> &'static str {
        match self {
            Tier::Top => "hard reasoning, design and final review",
            Tier::Workhorse => "most coding work",
            Tier::Quick => "search, summaries and mechanical edits",
        }
    }
}

/// What a model is besides its tier.
pub const LONG_CONTEXT: &str = "long-context";
pub const VISION: &str = "vision";
pub const CHEAP: &str = "cheap";
/// Every trait, in the order they're said.
pub const TRAITS: [&str; 3] = [LONG_CONTEXT, VISION, CHEAP];

/// A model's tier and traits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub tier: Tier,
    pub traits: Vec<&'static str>,
}

impl Placement {
    pub fn has(&self, t: &str) -> bool {
        self.traits.contains(&t)
    }
}

// The words a model's id is known by, kept beside the model lists rush
// knows: a model added to a harness's list falls into a tier by its family
// name, and a workhorse when its name says nothing. Settings › Config can
// move any model to another tier (Config.model_tiers).
const TOP_WORDS: &[&str] = &[
    "opus", "astra", "fable", "mythos", "large", "reasoner", "max", "pro", "ultra",
];
const QUICK_WORDS: &[&str] = &[
    "haiku",
    "luna",
    "flash",
    "mini",
    "nano",
    "small",
    "lite",
    "highspeed",
    "devstral",
    "fast",
    "air",
];
const VISION_WORDS: &[&str] = &[
    "opus", "sonnet", "haiku", "fable", "mythos", "gpt", "gemini", "vision", "vl",
];
const LONG_WORDS: &[&str] = &["1m", "gemini", "long"];
/// A window this long or longer is long context.
pub const LONG_WINDOW: i64 = 500_000;

/// The tier and traits of model id, whose window (0 when unknown) its
/// lister said; local says it runs on this machine, which costs nothing.
pub fn place(id: &str, context: i64, local: bool) -> Placement {
    let words = id_words(id);
    let any = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    let tier = if any(TOP_WORDS) {
        Tier::Top
    } else if any(QUICK_WORDS) || local {
        Tier::Quick
    } else {
        Tier::Workhorse
    };
    let mut traits = Vec::new();
    if context >= LONG_WINDOW || any(LONG_WORDS) {
        traits.push(LONG_CONTEXT);
    }
    if any(VISION_WORDS) {
        traits.push(VISION);
    }
    if local || (tier == Tier::Quick && any(QUICK_WORDS)) {
        traits.push(CHEAP);
    }
    Placement { tier, traits }
}

/// id's words, lower case: split at anything but letters and digits, and
/// letters apart from digits ("k3" stays whole, "gpt-6-luna" is gpt, 6,
/// luna).
fn id_words(id: &str) -> Vec<String> {
    id.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// A tier an agent names, with the traits it asks for: "quick",
/// "quick long-context", "top-vision". None when q names no tier first.
pub fn tier_query(q: &str) -> Option<(Tier, Vec<&'static str>)> {
    let q = q.trim().to_lowercase();
    let mut rest = q.as_str();
    let tier = Tier::ALL.into_iter().find(|t| {
        let n = t.as_str();
        match rest.strip_prefix(n) {
            Some(r) if r.is_empty() || r.starts_with([' ', '-', '_', '·', ',']) => {
                rest = r;
                true
            }
            _ => false,
        }
    })?;
    let rest: String = rest
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    let words: Vec<&str> = rest.split_whitespace().collect();
    let mut traits = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let t = match w {
            "long" => {
                if words.get(i + 1) == Some(&"context") {
                    i += 1;
                }
                Some(LONG_CONTEXT)
            }
            "vision" | "images" | "image" => Some(VISION),
            "cheap" | "cheaper" | "cheapest" => Some(CHEAP),
            _ => None,
        };
        // A word that's no trait makes it no tier: "top secret" names an
        // agent, not the top tier.
        let t = t?;
        if !traits.contains(&t) {
            traits.push(t);
        }
        i += 1;
    }
    Some((tier, traits))
}

/// A tier and traits as a type's name: "top", "quick-long-context".
pub fn tier_name(tier: Tier, traits: &[&str]) -> String {
    let mut n = tier.as_str().to_string();
    for t in traits {
        n.push('-');
        n.push_str(t);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_fall_into_tiers_by_their_family() {
        for (id, ctx, local, tier, traits) in [
            ("opus", 0, false, Tier::Top, vec![VISION]),
            (
                "claude-opus-5-5[1m]",
                0,
                false,
                Tier::Top,
                vec![LONG_CONTEXT, VISION],
            ),
            ("sonnet", 0, false, Tier::Workhorse, vec![VISION]),
            ("haiku", 0, false, Tier::Quick, vec![VISION, CHEAP]),
            ("gpt-6-astra", 0, false, Tier::Top, vec![VISION]),
            ("gpt-6.2-sol", 0, false, Tier::Workhorse, vec![VISION]),
            ("gpt-6-luna", 0, false, Tier::Quick, vec![VISION, CHEAP]),
            ("kimi-code/k3", 0, false, Tier::Workhorse, vec![]),
            ("kimi-code/k3-256k", 0, false, Tier::Workhorse, vec![]),
            ("glm-5.3", 0, false, Tier::Workhorse, vec![]),
            ("glm-5.3-flash", 0, false, Tier::Quick, vec![CHEAP]),
            ("devstral", 0, false, Tier::Quick, vec![CHEAP]),
            ("mistral-medium-3.5", 0, false, Tier::Workhorse, vec![]),
            ("qwen3-coder:30b", 0, true, Tier::Quick, vec![CHEAP]),
            (
                "some-new-model",
                1_000_000,
                false,
                Tier::Workhorse,
                vec![LONG_CONTEXT],
            ),
        ] {
            let p = place(id, ctx, local);
            assert_eq!((p.tier, p.traits), (tier, traits), "{id}");
        }
    }

    #[test]
    fn tiers_are_named_with_or_without_traits() {
        for (q, want) in [
            ("top", Some((Tier::Top, vec![]))),
            ("Quick", Some((Tier::Quick, vec![]))),
            (
                "quick long-context",
                Some((Tier::Quick, vec![LONG_CONTEXT])),
            ),
            (
                "quick-long-context",
                Some((Tier::Quick, vec![LONG_CONTEXT])),
            ),
            (
                "workhorse long context",
                Some((Tier::Workhorse, vec![LONG_CONTEXT])),
            ),
            ("top vision cheap", Some((Tier::Top, vec![VISION, CHEAP]))),
            ("top · vision", Some((Tier::Top, vec![VISION]))),
            ("top secret", None),
            ("topaz", None),
            ("kimi", None),
            ("", None),
        ] {
            assert_eq!(tier_query(q), want, "{q}");
        }
        assert_eq!(
            tier_name(Tier::Quick, &[LONG_CONTEXT]),
            "quick-long-context"
        );
        assert_eq!(
            tier_query(&tier_name(Tier::Top, &[VISION])),
            Some((Tier::Top, vec![VISION]))
        );
    }
}
