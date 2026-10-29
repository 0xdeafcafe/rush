//! How spawn_agent finds the agent a session asks for. The caller needn't
//! know the names spawn_agent lists: a model ("gpt-6.1-sol", "sol 6.1"), a
//! harness or provider ("codex", "kimi") or both ("codex astra") find it
//! too. See docs/subagents.md.
//!
//! Ported from internal/host/agentmatch.go.

use rush_core::adapter::Registry;

use crate::agents::{Pick, Runner};

/// The agent of ps that name, and model when given, ask for, and the model
/// to run it on when it's other than the agent's own ("" for that one).
pub fn find(reg: &Registry, ps: &[Pick], name: &str, model: &str) -> Option<(Pick, String)> {
    let (name, model) = (name.trim(), model.trim());
    if !model.is_empty()
        && let Some(p) = matches(reg, ps, &format!("{name} {model}"))
    {
        return Some((p, String::new()));
    }
    let p = matches(reg, ps, if name.is_empty() { model } else { name })?;
    if model.is_empty() {
        return Some((p, String::new()));
    }
    let m = model_of(&p.r, model);
    Some((p, m))
}

/// The agent of ps that q names: one by its listed name, or by its model's
/// id; else an agent, provider or harness by its own name, on its default
/// model; else the one whose words hold all q's, the one with fewest words
/// of its model left over first. Go's match.
pub fn matches(reg: &Registry, ps: &[Pick], q: &str) -> Option<Pick> {
    if q.is_empty() {
        return None;
    }
    let cq = clean_name(q);
    for p in ps {
        if p.name == q || p.name == cq || (!p.model.is_empty() && p.model.eq_ignore_ascii_case(q))
        {
            return Some(p.clone());
        }
    }
    for p in ps {
        if own_names(reg, &p.r).contains(&cq) {
            let mut p = p.clone();
            p.name = q.to_string();
            p.model.clear();
            return Some(p);
        }
    }
    let qw = words(q);
    let (mut best, mut left, mut named) = (None::<usize>, 0usize, false);
    for (i, p) in ps.iter().enumerate() {
        let mut have = [words(&p.name), words(&p.model)].concat();
        for n in own_names(reg, &p.r) {
            have.extend(words(&n));
        }
        if !holds(&have, &qw) {
            continue;
        }
        let mw = [words(&p.model), words(&p.name)].concat();
        let n = words(&p.model).iter().filter(|w| !qw.contains(w)).count();
        if best.is_none() || n < left {
            best = Some(i);
            left = n;
            named = qw.iter().any(|w| mw.contains(w));
        }
    }
    let mut p = ps[best?].clone();
    if !named {
        // Only its harness or provider was named: its default.
        p.name = q.to_string();
        p.model.clear();
    }
    Some(p)
}

/// The model of r's that q names, by its id or its words; q itself when r
/// lists none that fits, for its harness to take or refuse.
pub fn model_of(r: &Runner, q: &str) -> String {
    let qw = words(q);
    let (mut best, mut left) = (String::new(), 0isize);
    for c in &r.models {
        if c.id.eq_ignore_ascii_case(q) {
            return c.id.clone();
        }
        let mw = words(&c.id);
        if !holds(&mw, &qw) {
            continue;
        }
        let n = mw.len() as isize - qw.len() as isize;
        if best.is_empty() || n < left {
            best = c.id.clone();
            left = n;
        }
    }
    if best.is_empty() { q.to_string() } else { best }
}

/// What r is called apart from its models: its agent's, provider's and
/// harness's names and labels, as clean_name writes them.
fn own_names(reg: &Registry, r: &Runner) -> Vec<String> {
    let h = reg.harness_of(&r.k);
    vec![
        clean_name(&r.k),
        clean_name(&r.prov),
        clean_name(h.as_str()),
        clean_name(&reg.harness_label(&r.k)),
        clean_name(&reg.provider_label(&r.prov)),
    ]
}

/// Whether have has every word of want.
pub fn holds(have: &[String], want: &[String]) -> bool {
    !want.is_empty() && want.iter().all(|w| have.contains(w))
}

/// s's words, lower case: letters apart from numbers, and a version (6.1)
/// kept whole, so "sol6.1" is sol and 6.1, as "gpt-6.1-sol" has them.
pub fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut digits = false;
    let rs: Vec<char> = s.to_lowercase().chars().collect();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for (i, &c) in rs.iter().enumerate() {
        if c.is_numeric() {
            if !cur.is_empty() && !digits {
                flush(&mut cur, &mut out);
            }
            digits = true;
            cur.push(c);
        } else if c == '.'
            && digits
            && !cur.is_empty()
            && rs.get(i + 1).is_some_and(|n| n.is_numeric())
        {
            cur.push(c);
        } else if c.is_alphabetic() {
            if !cur.is_empty() && digits {
                flush(&mut cur, &mut out);
            }
            digits = false;
            cur.push(c);
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// The error for an agent none of ps is, naming them all.
pub fn no_agent(reg: &Registry, name: &str, model: &str, ps: &[Pick]) -> String {
    let q = format!("{name} {model}");
    let q = crate::reload::go_quote(q.trim());
    let mut names = Vec::new();
    let mut harnesses: Vec<String> = Vec::new();
    for p in ps {
        names.push(p.name.clone());
        let h = clean_name(reg.harness_of(&p.r.k).as_str());
        if !harnesses.contains(&h) {
            harnesses.push(h);
        }
    }
    if names.is_empty() {
        return format!(
            "rush has no agent matching {q}, nor any other it can start here: Settings › Config switches harnesses and providers on"
        );
    }
    format!(
        "rush has no agent matching {q}. Name one of these: {}; or a harness ({}), a model, or both (\"codex astra\"). A harness switched off in Settings › Config isn't offered",
        names.join(", "),
        harnesses.join(", ")
    )
}

/// s as an agent's name: lower case, a hyphen for anything but letters and
/// numbers, none doubled or at the ends.
pub fn clean_name(s: &str) -> String {
    let m: String = s
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut s = m.trim_matches('-').to_string();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::def_name;
    use rush_core::adapter::Choice;
    use std::collections::HashSet;

    fn runner(k: &str, models: &[&str]) -> Runner {
        Runner {
            k: k.into(),
            prov: k.into(),
            models: models
                .iter()
                .map(|m| Choice {
                    id: (*m).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// spawn_agent's agents as a machine with Codex and Kimi lists them,
    /// named as picks names them (agentmatch_test.go:codexPicks).
    fn codex_picks() -> Vec<Pick> {
        let codex = runner(
            "codex",
            &["gpt-6.1-sol", "gpt-6-astra", "gpt-6-sol", "gpt-5.6-sol"],
        );
        let kimi = runner("kimi", &["kimi-code/k3", "kimi-code/kimi-for-coding"]);
        let mut out = Vec::new();
        let mut taken = HashSet::new();
        for r in [&codex, &kimi] {
            for c in &r.models {
                let n = def_name(r, &c.id, &taken);
                taken.insert(n.clone());
                out.push(Pick {
                    name: n,
                    desc: String::new(),
                    r: r.clone(),
                    model: c.id.clone(),
                });
            }
        }
        out
    }

    // agentmatch_test.go:33
    #[test]
    fn find_agent() {
        let reg = Registry::new();
        let ps = codex_picks();
        for (agent, model, kind, want_model, want_run) in [
            ("astra", "", "codex", "gpt-6-astra", ""),
            ("sol", "", "codex", "gpt-6.1-sol", ""),
            ("gpt-6.1-sol", "", "codex", "gpt-6.1-sol", ""),
            ("GPT-6.1-Sol", "", "codex", "gpt-6.1-sol", ""),
            ("sol6.1", "", "codex", "gpt-6.1-sol", ""),
            ("sol 6.1", "", "codex", "gpt-6.1-sol", ""),
            ("sol 6", "", "codex", "gpt-6-sol", ""),
            ("gpt 5.6 sol", "", "codex", "gpt-5.6-sol", ""),
            ("gpt-5-6-sol", "", "codex", "gpt-5.6-sol", ""),
            ("codex", "", "codex", "", ""),
            ("Codex", "", "codex", "", ""),
            ("codex astra", "", "codex", "gpt-6-astra", ""),
            ("codex:sol6.1", "", "codex", "gpt-6.1-sol", ""),
            ("codex", "astra", "codex", "gpt-6-astra", ""),
            ("codex", "gpt-6-sol", "codex", "gpt-6-sol", ""),
            ("", "sol 6.1", "codex", "gpt-6.1-sol", ""),
            ("kimi", "", "kimi", "", ""),
            ("kimi coding", "", "kimi", "kimi-code/kimi-for-coding", ""),
            ("kimi-code/k3", "", "kimi", "kimi-code/k3", ""),
            ("codex", "gpt-7-nova", "codex", "", "gpt-7-nova"), // its harness takes or refuses it
        ] {
            let Some((p, run)) = find(&reg, &ps, agent, model) else {
                panic!("find({agent:?}, {model:?}) found nothing");
            };
            assert_eq!(
                (p.r.k.as_str(), p.model.as_str(), run.as_str()),
                (kind, want_model, want_run),
                "find({agent:?}, {model:?})"
            );
        }
    }

    // agentmatch_test.go:68: nothing fits, and the error names them all.
    #[test]
    fn find_agent_none() {
        let reg = Registry::new();
        let ps = codex_picks();
        for q in ["gemini", "sol 7", "nova"] {
            assert!(find(&reg, &ps, q, "").is_none(), "{q}");
        }
        let e = no_agent(&reg, "gemini", "", &ps);
        for n in ["astra", "sol", "kimi-code-k3", "codex", "kimi"] {
            assert!(e.contains(n), "the error leaves out {n}: {e}");
        }
    }

    // agentmatch_test.go:83
    #[test]
    fn words_keep_versions_whole() {
        for (s, want) in [
            ("gpt-6.1-sol", vec!["gpt", "6.1", "sol"]),
            ("sol6.1", vec!["sol", "6.1"]),
            ("opus[1m]", vec!["opus", "1", "m"]),
            ("v2.", vec!["v", "2"]),
            ("Kimi K3", vec!["kimi", "k", "3"]),
        ] {
            assert_eq!(words(s), want, "{s}");
        }
    }
}
