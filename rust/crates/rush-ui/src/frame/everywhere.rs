//! What General's "Every agent" says, written into each agent's own
//! settings where they don't say it already: when it's changed (say), at
//! start, and as accounts turn up. After Go's internal/ui/everywhere.go
//! (applyEverywhere :16, attributionAgents :49).

use rush_core::adapter::Registry;
use serde_json::{Map, Value};

use super::Out;
use crate::model::Model;

/// What applying "on" or "off" did: the agents set, and those that
/// failed, each said.
pub fn apply_attribution(reg: &Registry, v: &str) -> (Vec<String>, Vec<String>) {
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for c in reg.apply_attribution(v == "on") {
        let mut f = Map::new();
        f.insert("agent".into(), Value::from(c.kind.to_string()));
        f.insert("profile".into(), Value::from(c.profile.clone()));
        f.insert("on".into(), Value::from(v == "on"));
        match &c.err {
            Some(e) => {
                f.insert("err".into(), Value::from(e.to_string()));
                failed.push(c.to_string());
            }
            None => {
                let n = reg.harness_label(c.kind.as_str());
                if done.last() != Some(&n) {
                    done.push(n);
                }
            }
        }
        rush_events::log("attribution-set", f);
    }
    (done, failed)
}

/// The words for what apply_attribution did; None when there's nothing to
/// say.
pub fn said(v: &str, say: bool, done: &[String], failed: &[String]) -> Option<(String, bool)> {
    if !failed.is_empty() {
        return Some((
            format!("couldn't set attribution in {}", failed.join(", ")),
            true,
        ));
    }
    if say && !done.is_empty() {
        return Some((format!("attribution {v} in {}", done.join(", ")), false));
    }
    if say {
        return Some((format!("attribution was already {v} in every agent"), false));
    }
    None
}

/// The agents rush can set attribution in, named (attributionAgents).
pub fn attribution_agents(reg: &Registry) -> String {
    let mut names: Vec<String> = Vec::new();
    for a in reg.all().iter() {
        if a.attributor().is_some() {
            let n = reg.harness_label(a.kind().as_str());
            if names.last() != Some(&n) {
                names.push(n);
            }
        }
    }
    names.join(", ")
}

impl Model {
    /// Writes the setting v ("on", "off"; "" leaves each agent be) into
    /// every agent, off the UI thread, saying what it did when say
    /// (applyEverywhere).
    pub fn apply_everywhere(&mut self, v: &str, say: bool) {
        if v.is_empty() {
            return;
        }
        if !say
            && rush_core::dryrun::would(
                "set-attribution",
                &attribution_agents(&self.reg),
                &format!(
                    "General's Every agent says attribution {v}: written into each agent's settings"
                ),
            )
        {
            return;
        }
        let (reg, v) = (self.reg.clone(), v.to_string());
        self.later(
            move || {
                let (done, failed) = apply_attribution(&reg, &v);
                said(&v, say, &done, &failed)
            },
            |m, s| match s {
                Some((t, true)) => {
                    m.fail(&t);
                    Out::Draw
                }
                Some((t, false)) => {
                    m.flash(&t);
                    Out::Draw
                }
                None => Out::Idle,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_it_did() {
        let d = vec!["Claude Code".to_string()];
        assert_eq!(
            said("on", true, &d, &[]),
            Some(("attribution on in Claude Code".into(), false))
        );
        assert_eq!(
            said("off", true, &[], &[]),
            Some(("attribution was already off in every agent".into(), false))
        );
        assert_eq!(said("on", false, &d, &[]), None);
        assert_eq!(
            said("on", false, &d, &["codex (x): no".to_string()]),
            Some(("couldn't set attribution in codex (x): no".into(), true))
        );
    }
}
