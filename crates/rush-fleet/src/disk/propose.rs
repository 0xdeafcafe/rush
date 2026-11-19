//! Proposals: rules someone proposes to the engine, checked, and kept in
//! disk-rules.json's proposed until you accept one in Settings › Disk.
//! Rust-only: Go had no engine. They come from the cleanup agent's
//! research (PROPOSALS_FILE in its $TMPDIR), from what a cleanup agent's
//! plan teaches (rules.rs learn), and once from the cache rules learned
//! before rules waited for your yes (rules.rs take_learned_in).
//!
//! A proposal is taken only when its rule is one the engine would run as
//! it says: a tool command only of KNOWN_TOOLS, a cache folder only where
//! a cache rule may be (rules.rs may_be_rule, diskmap.rs may_be_yours),
//! never what rush never touches, a plugin's kind only as plugins name
//! them; and only with evidence. One that says what a rule or proposal
//! there already says is dropped.

use std::io;
use std::path::Path;

use rush_core::time::Ms;
use serde_json::{Value, json};

use super::{
    CACHE_DIR, CARGO_TARGET, KNOWN_TOOLS, Kind, Proposal, Rule, RulesFile, TOOL_COMMAND, WORKTREE,
    known_tool, update_rules,
};
use crate::temp::agent_event;

/// The file the cleanup agent's research writes its proposals to, in its
/// $TMPDIR: a JSON array of `{ "rule": {...}, "evidence": "..." }`.
pub const PROPOSALS_FILE: &str = "disk-proposals.json";

/// Plugin kinds a rule may not remove whole, and what to propose instead.
const NOT_WHOLE: &[(&str, &str)] = &[(
    "js.pnpm-store",
    "pnpm's store is cleared by pnpm store prune, never removed whole: propose a tool-command",
)];

/// What came of some proposals: the ids taken, the ids dropped as what a
/// rule or proposal already says, and the ids refused, each with why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proposed {
    pub added: Vec<String>,
    pub same: Vec<String>,
    pub refused: Vec<(String, String)>,
    /// What they came from was taken before: nothing was looked at.
    pub already: bool,
}

/// ~/ at the start of p made home.
fn untilde(p: &str, home: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => format!("{}/{rest}", home.trim_end_matches('/')),
        _ => p.to_string(),
    }
}

/// Whether a cache-dir rule may name path (~ allowed) for home: a folder
/// directly in ~/Library/Caches or ~/.cache, not rush's own
/// (may_be_rule), or two or more below home outside what the home walk
/// never enters and rush's own folders (may_be_yours); never what rush
/// never touches.
pub fn cache_dir_allowed(path: &str, home: &str) -> bool {
    let p = untilde(path.trim_end_matches('/'), home);
    if !p.starts_with('/') || crate::worktree::clean(&p) != p {
        return false;
    }
    crate::sweep::never_touch(&p, home).is_none()
        && (crate::rules::may_be_rule(&p, home) || crate::diskmap::may_be_yours(&p, home))
}

/// Why rule r isn't one the engine may run as it says, for home; Ok when
/// it is.
pub fn check_rule(r: &Rule, home: &str) -> Result<(), String> {
    let id = r.id.trim();
    if id.is_empty() {
        return Err("it has no id".into());
    }
    if r.title.trim().is_empty() {
        return Err(format!("{id}: it has no title"));
    }
    let levels: Vec<_> = [r.normal.as_ref(), r.low.as_ref()]
        .into_iter()
        .flatten()
        .collect();
    if levels.is_empty() {
        return Err(format!(
            "{id}: it runs at no level: give it normal or low numbers"
        ));
    }
    match &r.kind {
        Kind::CargoTarget | Kind::Worktree => {
            if levels.iter().any(|p| p.idle_secs.is_none()) {
                return Err(format!(
                    "{id}: say how long it sits idle (idleSecs) at each level it runs at"
                ));
            }
        }
        Kind::CargoArtifacts => {
            if levels
                .iter()
                .any(|p| p.age_secs.is_none() && (p.keep.is_none() || p.superseded_secs.is_none()))
            {
                return Err(format!(
                    "{id}: say how old artifacts go (ageSecs), or how many builds stay and when the rest go (keep, supersededSecs)"
                ));
            }
        }
        Kind::CacheDir {
            path,
            why,
            go_clean,
            ..
        } => {
            if why.trim().is_empty() {
                return Err(format!("{id}: say what makes the folder again (why)"));
            }
            if !(path.is_empty() && *go_clean) && !cache_dir_allowed(path, home) {
                return Err(format!(
                    "{id}: {path} isn't a folder a rule may name: one directly in ~/Library/Caches or ~/.cache, or two or more below home, never what rush never touches or keeps"
                ));
            }
        }
        Kind::ToolCommand { command, why } => {
            if !known_tool(command) {
                return Err(format!(
                    "{id}: {} isn't a known tool ({})",
                    command.first().map_or("nothing", String::as_str),
                    KNOWN_TOOLS.join(", ")
                ));
            }
            if why.trim().is_empty() {
                return Err(format!("{id}: say what the command frees (why)"));
            }
        }
        Kind::PluginCandidate { kind } => {
            if let Some((_, why)) = NOT_WHOLE.iter().find(|(k, _)| k == kind) {
                return Err(format!("{id}: {why}"));
            }
            let engine = [CARGO_TARGET, WORKTREE, CACHE_DIR, TOOL_COMMAND].contains(&kind.as_str());
            if engine || !kind.contains('.') || kind.contains(char::is_whitespace) {
                return Err(format!(
                    "{id}: {kind:?} isn't a kind plugins report (plugin.what, as js.node-modules)"
                ));
            }
        }
    }
    Ok(())
}

/// r as compared: no id, title or whys, on, a folder's path with ~ made
/// home.
fn bare(r: &Rule, home: &str) -> Rule {
    let kind = match &r.kind {
        Kind::CacheDir {
            path,
            each,
            go_clean,
            ..
        } => Kind::CacheDir {
            path: untilde(path.trim_end_matches('/'), home),
            why: String::new(),
            each: *each,
            go_clean: *go_clean,
        },
        Kind::ToolCommand { command, .. } => Kind::ToolCommand {
            command: command.clone(),
            why: String::new(),
        },
        k => k.clone(),
    };
    Rule {
        id: String::new(),
        title: String::new(),
        kind,
        enabled: true,
        normal: r.normal.clone(),
        low: r.low.clone(),
    }
}

/// Whether a and b do the same thing at the same numbers, whatever their
/// ids, titles and words, and whether they're on.
pub fn same_rule(a: &Rule, b: &Rule, home: &str) -> bool {
    bare(a, home) == bare(b, home)
}

/// The proposals in text (a JSON array of them, an object with a
/// "proposed" array, or one), each read on its own: one that isn't a
/// proposal is refused, with why, and the rest are read.
pub fn parse_proposals(text: &str) -> (Vec<Proposal>, Vec<(String, String)>) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), vec![(PROPOSALS_FILE.into(), e.to_string())]),
    };
    let items = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("proposed") {
            Some(Value::Array(a)) => a,
            Some(_) => {
                return (
                    Vec::new(),
                    vec![(PROPOSALS_FILE.into(), "proposed isn't a list".into())],
                );
            }
            None => vec![Value::Object(o)],
        },
        _ => {
            return (
                Vec::new(),
                vec![(PROPOSALS_FILE.into(), "not a list of proposals".into())],
            );
        }
    };
    let (mut out, mut refused) = (Vec::new(), Vec::new());
    for (i, it) in items.into_iter().enumerate() {
        let id = it
            .pointer("/rule/id")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("#{}", i + 1));
        match serde_json::from_value::<Proposal>(it) {
            Ok(p) => out.push(p),
            Err(e) => refused.push((id, e.to_string())),
        }
    }
    (out, refused)
}

/// Adds new to f's proposals, each on, checked: refused without evidence,
/// when check_rule refuses it, or when its id is a rule of another kind;
/// dropped when a rule or proposal there says the same (same_rule);
/// replacing a proposal of the same id. Pure.
pub fn add_proposals(f: &mut RulesFile, new: Vec<Proposal>, home: &str) -> Proposed {
    let mut out = Proposed::default();
    for mut p in new {
        let id = p.rule.id.trim().to_string();
        p.rule.id = id.clone();
        if p.evidence.trim().is_empty() {
            out.refused.push((
                id,
                "no evidence: say the paths, their sizes, what made them and why it's safe".into(),
            ));
            continue;
        }
        if let Err(e) = check_rule(&p.rule, home) {
            out.refused.push((id, e));
            continue;
        }
        let kind_of = std::mem::discriminant(&p.rule.kind);
        if f.rules
            .iter()
            .any(|r| r.id == id && std::mem::discriminant(&r.kind) != kind_of)
        {
            out.refused.push((
                id.clone(),
                format!("{id} is already a rule of another kind"),
            ));
            continue;
        }
        let same = f.rules.iter().any(|r| same_rule(r, &p.rule, home))
            || f.proposed.iter().any(|q| same_rule(&q.rule, &p.rule, home));
        if same {
            out.same.push(id);
            continue;
        }
        p.rule.enabled = true;
        f.proposed.retain(|q| q.rule.id != id);
        f.proposed.push(p);
        out.added.push(id);
    }
    out
}

/// Keeps new as proposals (add_proposals) in the rules file, under its
/// lock, by and at as given; refused are ones that couldn't be read. With
/// from (what they came from, "<session>/<file>"), only once: from taken
/// before is already, and nothing is added. Each time anything's added
/// or refused it's a disk-proposed event.
pub fn propose(
    new: Vec<Proposal>,
    refused: Vec<(String, String)>,
    by: &str,
    at: Ms,
    from: &str,
) -> io::Result<Proposed> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = Proposed::default();
    update_rules(|f| {
        if !from.is_empty() && f.taken_from.iter().any(|t| t == from) {
            out.already = true;
            return;
        }
        let new = new
            .into_iter()
            .map(|mut p| {
                p.by = by.to_string();
                p.at = at;
                p
            })
            .collect();
        out = add_proposals(f, new, &home);
        if !from.is_empty() {
            f.taken_from.push(from.to_string());
        }
    })?;
    if out.already {
        return Ok(out);
    }
    out.refused.splice(0..0, refused);
    if !out.added.is_empty() || !out.refused.is_empty() {
        agent_event(
            None,
            "disk-proposed",
            json!({
                "by": by,
                "from": from,
                "added": out.added,
                "same": out.same,
                "refused": out
                    .refused
                    .iter()
                    .map(|(id, why)| json!({ "id": id, "why": why }))
                    .collect::<Vec<_>>(),
            }),
        );
    }
    Ok(out)
}

/// The proposals a cleanup agent's research wrote at path (PROPOSALS_FILE
/// in its $TMPDIR) kept (propose), once per from. None when there's no
/// such file.
pub fn propose_file(path: &Path, by: &str, at: Ms, from: &str) -> io::Result<Option<Proposed>> {
    let text = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        r => r?,
    };
    let (new, refused) = parse_proposals(&text);
    propose(new, refused, by, at, from).map(Some)
}
