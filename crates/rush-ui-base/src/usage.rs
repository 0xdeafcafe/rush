//! What of rush you open, by name, in the event log: kind `ui-open`
//! (`rush events -k ui-open`), so which screens, sheets, views, presets
//! and commands are used can be counted, and what never is can go.
//! Rust-only; Go kept no such record. Names only, and only rush's own: a
//! sheet's type, a view's or a preset's name, a command rush answers.
//! Never what's typed, a session's name or a path.

use rush_events::Fields;
use serde_json::Value;

/// Notes that the thing of this sort (sheet, view, preset, command) and
/// name was opened. The log's writer is a thread of its own: this only
/// queues.
pub fn opened(what: &str, name: &str) {
    let name = tidy(name);
    if name.is_empty() {
        return;
    }
    let mut f = Fields::new();
    f.insert("what".into(), Value::from(what));
    f.insert("name".into(), Value::from(name));
    rush_events::log("ui-open", f);
}

/// A type's path as its last part, and nothing but a short name let
/// through: letters, digits, - and _, 40 at most. Anything else isn't a
/// name of rush's, and isn't written.
pub fn tidy(name: &str) -> &str {
    let name = name.rsplit("::").next().unwrap_or(name);
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok { name } else { "" }
}

#[cfg(test)]
mod tests {
    use super::tidy;

    #[test]
    fn only_names() {
        for (given, want) in [
            ("rush_ui_accounts::newsheet::NewSheet", "NewSheet"),
            ("changes", "changes"),
            ("Orchestrator", "Orchestrator"),
            ("model", "model"),
            ("fix the login bug", ""),
            ("/Users/me/secret.txt", ""),
            ("", ""),
            ("Box<dyn Sheet>", ""),
        ] {
            assert_eq!(tidy(given), want, "{given:?}");
        }
        assert_eq!(tidy(&"x".repeat(41)), "");
    }
}
