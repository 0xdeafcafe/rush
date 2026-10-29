//! The agents a session can hand work to: runners (a provider in the
//! harness it's set to run in, as rush's stand-ins host it), the picks
//! spawn_agent names, the subagent types a Claude Code session runs
//! itself, and the words that tell a session of them.
//!
//! Ported from internal/host/agents.go and agent.go:772-782 (skillRoots).

use std::collections::{BTreeMap, HashSet};

use rush_core::adapter::{AdapterRef, Choice, Registry, Spawnable};
use rush_core::wire::RawJson;
use rush_state::Config as Settings;

use crate::agentmatch::clean_name;
use crate::kind::{installed, sign_in_if_out};

/// An agent a session can hand work to from its shell, as rush's
/// stand-ins host it: a provider in the harness it's set to run in.
#[derive(Clone, Debug, Default)]
pub struct Runner {
    pub k: String,
    pub prov: String,
    /// Its harness's, where Settings signs it in: "Codex".
    pub name: String,
    /// `codex exec "<task>"`, RUSH_AGENT= before one riding another's
    /// harness.
    pub cmd: String,
    /// How a model is picked: "-m", or "VAR=" set before the command.
    pub flag: String,
    pub models: Vec<Choice>,
    /// Signed in, or has its key.
    pub ready: bool,
    /// Its cell of Settings › Config (provider id), in its harness.
    pub cell: String,
}

/// The agents a session led by harness leader can hand work to. The
/// leader's own harness runs what its row of Settings › Config lets it,
/// on the models it may call there; every other installed provider runs in
/// the harness it's set to run in (its ★), when that harness is on, on the
/// models its cell there allows. One that can't run yet is still one, not
/// ready, so asked for by name it's known and the user told to sign it in.
/// Config is read on each call, so a change in Settings reaches a session
/// already running.
pub fn runners(reg: &Registry, leader: &str) -> Vec<Runner> {
    let cfg = rush_state::Store::load().config;
    let mut out = Vec::new();
    for a in installed(reg) {
        let k = a.kind().as_str().to_string();
        let prov = reg.provider_of(&k);
        if reg.key_only(&k) && !cfg.has_api_key(&prov) {
            continue; // no key to pay with
        }
        let Some(cell) = takes(reg, &cfg, leader, &k, &prov) else {
            continue;
        };
        let base = match a.rider() {
            Some(r) => match reg.get(r.rides().as_str()) {
                Some(b) => b,
                None => continue,
            },
            None => a.clone(),
        };
        let Some(sp) = base.spawnable() else {
            continue; // no stand-in hosts it
        };
        let mut r = new_runner(reg, &a, &base, sp, &k, &prov);
        if !cell.is_empty() {
            let h = reg.harness_of(&k);
            r.models.retain(|c| cfg.calls(&cell, h.as_str(), &c.id));
        }
        r.cell = cell;
        out.push(r);
    }
    out
}

/// Agent a as a runner: base its harness, sp the stand-in that hosts it,
/// and the models it offers, its own first.
fn new_runner(
    reg: &Registry,
    a: &AdapterRef,
    base: &AdapterRef,
    sp: &dyn Spawnable,
    k: &str,
    prov: &str,
) -> Runner {
    let mut r = Runner {
        k: k.into(),
        prov: prov.into(),
        name: base.name(),
        ..Runner::default()
    };
    let snap = a.runner_snapshot();
    if let Some(s) = snap {
        (r.models, r.ready) = s.runner_snapshot();
    } else {
        r.ready = signed_in(reg, base) && has_key(reg, a);
    }
    (r.cmd, r.flag) = sp.spawn_command();
    if k != base.kind().as_str() {
        r.cmd = format!("RUSH_AGENT={k} {}", r.cmd);
    }
    if snap.is_none() {
        r.models = reg.choices_of(k).map(|c| c.models).unwrap_or_default();
    }
    // Its own models first: one riding another harness (Ollama in Codex)
    // has other models than that harness's account offers.
    let lister = if a.model_lister().is_some() { a } else { base };
    if let Some(ml) = lister.model_lister()
        && snap.is_none()
        && r.models.is_empty()
        && let Some(p) = reg.profiles_of(lister).first()
    {
        r.models = ml.list_models(p);
    }
    if r.models.is_empty() {
        r.models = local_models(reg, prov)
            .into_iter()
            .map(|id| Choice {
                id,
                ..Choice::default()
            })
            .collect();
    }
    r
}

/// Whether a session led by leader hands work to agent k, and the Config
/// cell (provider id) it runs as. In the leader's own harness, that's the
/// leader's row; a provider the leader's row runs is run only there. Any
/// other provider runs in the harness it's set to run in, its key in its
/// own, when that harness is on.
pub fn takes(reg: &Registry, cfg: &Settings, leader: &str, k: &str, prov: &str) -> Option<String> {
    let h = reg.harness_of(k);
    if !leader.is_empty() && h.as_str() == leader {
        let cell = leader_cell(reg, cfg, leader, k, prov);
        return (!cell.is_empty()).then_some(cell);
    }
    if !leader.is_empty() && leader_runs(reg, cfg, leader, prov) {
        return None;
    }
    if !cfg.harness_on(h.as_str()) {
        return None;
    }
    if reg
        .kind_for(prov, &cfg.harness_for(prov))
        .is_some_and(|w| w.as_str() == k)
    {
        return Some(prov.to_string());
    }
    if reg.split(prov) {
        let key = rush_core::adapter::key_of(prov);
        if reg
            .kind_for(prov, &cfg.harness_for(&key))
            .is_some_and(|w| w.as_str() == k)
        {
            return Some(key);
        }
    }
    None
}

/// The plan and key ids of provider prov.
fn ids_of(reg: &Registry, prov: &str) -> Vec<String> {
    let mut ids = vec![prov.to_string()];
    if reg.split(prov) {
        ids.push(rush_core::adapter::key_of(prov));
    }
    ids
}

/// Whether leader's row of Config runs provider prov, by its plan or its
/// key, in leader itself.
fn leader_runs(reg: &Registry, cfg: &Settings, leader: &str, prov: &str) -> bool {
    ids_of(reg, prov).iter().any(|id| {
        let (ck, _, _) = reg.compat(id, leader);
        !ck.is_empty() && cfg.cell_on(id, leader)
    })
}

/// The provider id agent k runs as in leader's row of Config, when it's
/// that harness's and the cell is on: its plan, else its key; "" when the
/// leader may not run it.
pub fn leader_cell(reg: &Registry, cfg: &Settings, leader: &str, k: &str, prov: &str) -> String {
    if reg.harness_of(k).as_str() != leader {
        return String::new();
    }
    for id in ids_of(reg, prov) {
        let (ck, _, _) = reg.compat(&id, leader);
        if ck.as_str() == k && cfg.cell_on(&id, leader) {
            return id;
        }
    }
    String::new()
}

/// Tells a session which agents it can start from its shell, for a harness
/// that takes no MCP tools (Pi): the rest have spawn_agent. "" when
/// there's none to tell of. leader is the harness leading it.
pub fn agents_prompt(reg: &Registry, leader: &str) -> String {
    let lines: Vec<String> = runners(reg, leader)
        .iter()
        .map(|r| runner_line(reg, r))
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "You can hand work to other agents by running them from your shell; rush hosts each one, signed in, and shows it to the user as your subagent, and its output comes back as the program's own:\n{}\nWhen the user asks for one by name (\"an ollama lane\"), run that line as given; a model named on its own, or by part of its name, is the line whose list has it, run with that model. Pick one by what the work needs: a cheaper or local model for routine work, another provider for a second opinion. Treat them as you treat your own subagent types: if one fails to start, use another and tell the user; never debug its sign-in or setup.",
        lines.join("\n")
    )
}

/// One runner's line of agents_prompt.
fn runner_line(reg: &Registry, r: &Runner) -> String {
    let out = if r.ready {
        String::new()
    } else {
        format!(
            "; availability unverified, so don't run it: check or refresh its account in rush, Settings, {}",
            r.name
        )
    };
    let ids: Vec<String> = r
        .models
        .iter()
        .map(|c| {
            if c.note.is_empty() {
                return c.id.clone();
            }
            // What it's for, so "a cheap one" finds one.
            let mut ch = c.note.chars();
            let first = ch.next().map(|f| f.to_lowercase().to_string()).unwrap_or_default();
            let rest = ch.as_str();
            format!(
                "{} ({first}{})",
                c.id,
                rest.strip_suffix('.').unwrap_or(rest)
            )
        })
        .collect();
    let what = if !ids.is_empty() {
        format!("one of {}", ids.join(", "))
    } else if r.prov == "ollama" {
        "<a model from `ollama list`>".to_string()
    } else {
        "<model>".to_string()
    };
    let mut pick = format!(", {} {what}", r.flag);
    if r.flag.ends_with('=') {
        // A variable, set before the command.
        pick = format!(", {}<model> before it", r.flag);
        if what != "<model>" {
            pick += &format!(", {what}");
        }
    }
    format!(
        "- {}: {}'s models in {}, `{}`{pick}{out}",
        r.prov,
        reg.provider_label(&r.prov),
        reg.harness_label(&r.k),
        r.cmd
    )
}

/// Tells a session that has rush's tools how to reach other agents.
pub const TOOLS_NOTE: &str = "An agent or model the user names (\"have <name> do it\", \"use <name> for that\", \"a codex lane\"), of any provider, is one rush's spawn_agent tool starts: start it with that tool, in the background when you have other work meanwhile, not from your shell.";

/// Tells a session with agent_defs how to reach them, and the rest.
pub const AGENTS_NOTE: &str = "An agent or model the user names (\"have <name> do it\", \"use <name> for that\") is one of your subagent types, started with the Agent tool; any other, such as another provider's model (Codex's astra or sol, Kimi, GLM), is started with rush's spawn_agent tool, which finds it by its model or harness name too; never by message or from your shell.";

/// A subagent's that does the work itself.
pub const WORK_PROMPT: &str = "Do the task you're given, fully and carefully, then report what you did and what you found: short, with the details the one who asked needs to carry on.";

/// An agent a session can start: a runner on one of its models, by the
/// name the session calls it.
#[derive(Clone, Debug, Default)]
pub struct Pick {
    pub name: String,
    pub desc: String,
    pub r: Runner,
    /// "" for its own default.
    pub model: String,
}

/// The agents a session led by harness leader can start, each named once;
/// with no leader, every provider's.
pub fn picks(reg: &Registry, leader: &str) -> Vec<Pick> {
    picks_of(&runners(reg, leader))
}

/// picks of the runners rs.
pub fn picks_of(rs: &[Runner]) -> Vec<Pick> {
    let mut out = Vec::new();
    let mut taken = HashSet::new();
    for r in rs {
        let default = [Choice::default()];
        let models: &[Choice] = if r.models.is_empty() {
            &default // its own default
        } else {
            &r.models
        };
        for c in models {
            let name = def_name(r, &c.id, &taken);
            taken.insert(name.clone());
            let note = if c.note.is_empty() {
                "A capable general agent"
            } else {
                &c.note
            };
            let desc = format!("{}.", note.strip_suffix('.').unwrap_or(note));
            let mut ch = desc.chars();
            let desc = match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => desc,
            };
            out.push(Pick {
                name,
                desc,
                r: r.clone(),
                model: c.id.clone(),
            });
        }
    }
    out
}

/// The subagent types (--agents) a Claude Code session of kind session
/// runs itself: its own models, by name. Every other agent is
/// spawn_agent's.
pub fn agent_defs(reg: &Registry, session: &str) -> BTreeMap<String, RawJson> {
    let mut defs = BTreeMap::new();
    for p in picks(reg, reg.harness_of(session).as_str()) {
        if !own_def(&p, session) {
            continue;
        }
        let v = serde_json::json!({"description": p.desc, "model": p.model, "prompt": WORK_PROMPT});
        if let Ok(b) = RawJson::new(&v.to_string()) {
            defs.insert(p.name, b);
        }
    }
    defs
}

/// Whether p is one of session's own subagent types.
pub(crate) fn own_def(p: &Pick, session: &str) -> bool {
    p.r.k == session && p.r.prov == session && plain_model(&p.model) && p.r.ready
}

/// What an agent on model is called: the model's own short name (astra for
/// gpt-6-astra) when that's free, else its whole id, else the agent's with
/// it; the agent's own for its default.
pub fn def_name(r: &Runner, model: &str, taken: &HashSet<String>) -> String {
    let tries: Vec<String> = if model.is_empty() {
        vec![clean_name(&r.k)]
    } else {
        let short = &model[model.rfind('-').map_or(0, |i| i + 1)..];
        let mut t = vec![clean_name(model), clean_name(&format!("{}-{model}", r.k))];
        if plain_model(short) && short.len() >= 3 {
            t.insert(0, short.to_string());
        }
        t
    };
    for n in &tries {
        if !n.is_empty() && !taken.contains(n) {
            return n.clone();
        }
    }
    tries.last().cloned().unwrap_or_default()
}

/// Whether id is letters alone: a name, not a version.
pub fn plain_model(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase())
}

/// Whether a's first profile can run now, rush signing it in with the
/// sign-in it keeps when it's out.
fn signed_in(reg: &Registry, a: &AdapterRef) -> bool {
    match reg.profiles_of(a).first() {
        None => true,
        Some(p) => sign_in_if_out(reg, a.kind().as_str(), p).is_ok(),
    }
}

/// Whether a finds the key its own program keeps, when it runs on one.
fn has_key(reg: &Registry, a: &AdapterRef) -> bool {
    let Some(kc) = a.key_checker() else {
        return true;
    };
    match reg.profiles_of(a).first() {
        None => true,
        Some(p) => kc.check_key(p).is_ok(),
    }
}

/// The models provider prov has on this machine, as its own agent's
/// Summarizer lists them (Ollama's), else none.
fn local_models(reg: &Registry, prov: &str) -> Vec<String> {
    reg.get(prov)
        .and_then(|a| a.summarizer().and_then(|s| s.summary_models().ok()))
        .unwrap_or_default()
}

/// The folders of skills the first installed agent that shares its own
/// has in cwd (Claude Code's), for an agent that doesn't read them itself.
pub fn skill_roots(reg: &Registry, cwd: &str) -> Vec<String> {
    for a in installed(reg) {
        if let Some(sr) = a.skill_rooter() {
            let ps = reg.profiles_of(&a);
            return ps.first().map(|p| sr.skill_roots(p, cwd)).unwrap_or_default();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(k: &str, models: &[(&str, &str)], ready: bool) -> Runner {
        Runner {
            k: k.into(),
            prov: k.into(),
            name: k.into(),
            cmd: format!("{k} exec \"<task>\""),
            flag: "-m".into(),
            models: models
                .iter()
                .map(|(id, note)| Choice {
                    id: (*id).into(),
                    note: (*note).into(),
                    ..Default::default()
                })
                .collect(),
            ready,
            ..Default::default()
        }
    }

    #[test]
    fn def_names_short_then_whole() {
        let codex = r("codex", &[], true);
        let mut taken = HashSet::new();
        let a = def_name(&codex, "gpt-6-astra", &taken);
        assert_eq!(a, "astra");
        taken.insert(a);
        // Taken: the whole id.
        assert_eq!(def_name(&codex, "gpt-7-astra", &taken), "gpt-7-astra");
        // A version isn't a name.
        assert_eq!(def_name(&codex, "gpt-6.1", &taken), "gpt-6-1");
        assert_eq!(def_name(&codex, "", &taken), "codex");
    }

    #[test]
    fn picks_describe_each_model() {
        let ps = picks_of(&[
            r("claude", &[("opus", "the deepest thinker."), ("haiku", "")], true),
            r("codex", &[], false),
        ]);
        let got: Vec<(&str, &str, &str)> = ps
            .iter()
            .map(|p| (p.name.as_str(), p.model.as_str(), p.desc.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("opus", "opus", "The deepest thinker."),
                ("haiku", "haiku", "A capable general agent."),
                ("codex", "", "A capable general agent."),
            ]
        );
        assert!(own_def(&ps[0], "claude") && !own_def(&ps[2], "codex"));
    }

    #[test]
    fn runner_lines_say_how_to_run_it() {
        let reg = Registry::new();
        let mut v = r("vibe", &[("devstral", "Fast and cheap.")], false);
        v.flag = "VIBE_MODEL=".into();
        let l = runner_line(&reg, &v);
        assert!(
            l.ends_with(", VIBE_MODEL=<model> before it, one of devstral (fast and cheap); availability unverified, so don't run it: check or refresh its account in rush, Settings, vibe"),
            "{l}"
        );
        let o = r("ollama", &[], true);
        assert!(
            runner_line(&reg, &o).ends_with("`ollama exec \"<task>\"`, -m <a model from `ollama list`>"),
            "{}",
            runner_line(&reg, &o)
        );
    }
}
