//! How a subagent run is named wherever one is listed: what it was asked,
//! then what runs it. Port of internal/convo/runname.go (RunTag :12,
//! OtherHarness :46), spawn.go:657 (runLabel) and exchange.go:69
//! (peerMark, SetPeerMark).

use std::sync::RwLock;

use rush_core::adapter::LEGACY_KIND;
use rush_core::adapter::global;

/// How an agent kind's logo is drawn.
type Mark = fn(&str) -> String;

/// An agent kind's logo, as rush's UI marks it; set by the UI.
static PEER_MARK: RwLock<Option<Mark>> = RwLock::new(None);

/// Sets how an agent kind's logo is drawn (Go's SetPeerMark).
pub fn set_peer_mark(f: Mark) {
    *PEER_MARK.write().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// What runs a subagent run, as every place that lists one says it after
/// what it was asked: the model, or the agent type when the model isn't
/// known. A run in a harness other than home, the session's own, says so
/// first, with that harness's mark and name: "◎ Codex astra".
pub fn run_tag(kind: &str, home: &str, model: &str, typ: &str) -> String {
    let home = if home.is_empty() { LEGACY_KIND } else { home };
    // "subagent" is only that it's one, which its row says already.
    let mut tag = if typ == "subagent" { "" } else { typ }.to_string();
    if !model.is_empty() && !model.starts_with('<') {
        // not <synthetic>
        let k = if kind.is_empty() { home } else { kind };
        tag = global().model_name(k, model);
    }
    if !other_harness(kind, home) || kind == home {
        return tag;
    }
    let h = global().harness_of(kind);
    let name = global()
        .get(h.as_str())
        .map_or_else(|| h.as_str().to_string(), |a| a.name());
    let mark = PEER_MARK
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .map(|f| format!("{} ", f(kind)))
        .unwrap_or_default();
    if tag.is_empty() || tag == name || tag == kind {
        return format!("{mark}{name}");
    }
    format!("{mark}{name} {tag}")
}

/// Whether a run of kind works in another harness than home, the session
/// that started it.
pub fn other_harness(kind: &str, home: &str) -> bool {
    let home = if home.is_empty() { LEGACY_KIND } else { home };
    !kind.is_empty() && global().harness_of(kind) != global().harness_of(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claude_subagent_is_named_by_its_type() {
        assert_eq!(run_tag("", "claude", "", "Explore"), "Explore");
        assert_eq!(run_tag("", "", "", "subagent"), "");
        assert!(!other_harness("", "claude"));
        assert!(!other_harness("claude", ""));
    }
}
