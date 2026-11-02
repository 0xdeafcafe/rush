//! Asking the cheapest model rush can run without a session. After Go's
//! internal/agent/cheap.go:1-34 (AskCheap).

use super::{LEGACY_KIND, Registry, Result};

/// Asks the cheapest model rush can run without a session, the first of
/// its models whose name has prefer when one does, and says which it
/// asked: (model, answer).
pub fn ask_cheap(
    reg: &Registry,
    system: &str,
    prompt: &str,
    prefer: &str,
) -> Result<(String, String)> {
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
        let out = sm.summarize(pick, system, prompt)?;
        return Ok((pick.clone(), out));
    }
    Err("no model to ask: it runs on Claude Code or Ollama".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // With neither Claude Code nor Ollama, it says what it needs.
    #[test]
    fn no_model_says_so() {
        let e = ask_cheap(&Registry::new(), "s", "p", "haiku").unwrap_err();
        assert_eq!(
            e.to_string(),
            "no model to ask: it runs on Claude Code or Ollama"
        );
    }
}
