//! The cleanup agent's research mode (Rust-only; Go had no disk engine):
//! Settings › Disk's "Research now" starts the cleanup agent to find what
//! fills the disk and who made it, and to propose rules for the disk
//! engine, each with its evidence. It deletes, stops and runs nothing that
//! changes the disk; what it proposes waits in Settings › Disk for your
//! yes, and runs only once you accept it.
//!
//! rush looks first, with what Clean up uses (rush_fleet's survey: the
//! rules' caches and the worktrees' build caches, triage's groups;
//! diskmap's home map: where the disk goes, what's big and untouched),
//! what the plugins report (disk.candidates and cleanup.report) and the
//! rules there are, and the agent is told it all. What it writes
//! ($TMPDIR/disk-proposals.json) is read once it ends (care.rs end_care,
//! take_in), each proposal checked (rush_fleet::disk::propose).

use std::path::PathBuf;

use rush_core::time::Ms;
use rush_fleet::disk::{self, Candidate, KNOWN_TOOLS, PROPOSALS_FILE, Proposed, RulesFile};
use rush_plugin::CleanupReport;

use super::care::{CARE_PLAN_FILE, CARE_PLUGIN, CareStart, CareStarter, care_allow, care_known};
use super::data::*;
use crate::frame::Out;
use crate::model::Model;
use crate::style::disk as size;

/// The research agent's name in the list.
pub const RESEARCH_NAME: &str = "Disk research";
/// Who proposed what it proposes, as Settings › Disk says.
pub const RESEARCH_BY: &str = "the cleanup agent's research";
/// At most so many of a plugin kind's candidates are told, biggest first.
const PER_KIND: usize = 5;
/// At most so many of home's top folders are told.
const TOPS: usize = 12;

/// What rush found before the research agent starts.
pub struct Found {
    /// Clean up's survey, and the biggest of what rush never touches.
    pub survey: Option<(rush_fleet::Survey, Vec<rush_fleet::FoundCache>)>,
    pub map: Option<rush_fleet::HomeMap>,
    pub candidates: Vec<Candidate>,
    pub reports: Vec<CleanupReport>,
    /// The rules and proposals there are; why not when they can't be read.
    pub rules: Result<RulesFile, String>,
}

/// Research between frames.
#[derive(Default)]
pub struct Research {
    /// rush is looking before it starts the agent.
    pub looking: bool,
}

/// Whether a cleanup agent (research or not) is at work.
pub fn care_at_work(m: &Model) -> bool {
    m.agents.iter().any(|a| {
        a.rush
            && a.started_by == CARE_PLUGIN
            && !a.done
            && a.job.state != rush_core::agent::State::Done
    })
}

/// Research now: rush looks at the disk off the UI, then starts the
/// cleanup agent in research mode. Not while a cleanup agent is at work,
/// or rush is still looking.
pub fn research_now(m: &mut Model) -> Out {
    if care_at_work(m) {
        m.flash_err("a cleanup agent is already at work");
        return Out::Draw;
    }
    if m.kept::<CareStarter>().0.is_none() {
        m.flash_err("starting a cleanup agent isn't in this rush yet");
        return Out::Draw;
    }
    if m.kept::<Research>().looking {
        m.flash("research: rush is still looking at the disk");
        return Out::Draw;
    }
    m.kept::<Research>().looking = true;
    m.flash("research: rush looks at the disk first, then starts the cleanup agent");
    let (wts, free, between) = super::clean::survey_paths(m);
    let (now, frozen) = (m.now, data(m).frozen);
    m.later(
        move || Found {
            survey: (!frozen).then(|| super::clean::survey_all(&wts, &free, &between, now)),
            map: (!frozen).then(|| rush_fleet::home_map(&rush_fleet::cache_rules(), now)),
            candidates: super::diskauto::plugin_candidates(),
            reports: super::care::care_reports(),
            rules: disk::load_rules().map_err(|e| e.to_string()),
        },
        start_research,
    );
    Out::Draw
}

/// Starts the research agent with what rush found, as rush's own.
fn start_research(m: &mut Model, f: Found) -> Out {
    m.kept::<Research>().looking = false;
    if let Some((s, never)) = &f.survey {
        let d = m.kept::<Data>();
        d.wt_facts.extend(s.facts.clone());
        d.survey = s.caches.clone();
        d.never = never.clone();
    }
    if care_at_work(m) {
        m.flash_err("a cleanup agent is already at work");
        return Out::Draw;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let mut known = care_known(m);
    known.extend(research_known(&f, &home, m.now));
    let start = CareStart {
        prompt: research_prompt(&known, &f.reports),
        cwd: home,
        name: RESEARCH_NAME.into(),
        auto: false,
        started_by: CARE_PLUGIN.into(),
        why: "researching disk rules".into(),
        allow: care_allow(),
    };
    let Some(go) = m.kept::<CareStarter>().0 else {
        m.flash_err("starting a cleanup agent isn't in this rush yet");
        return Out::Draw;
    };
    go(m, start)
}

/// p with home put back as ~.
fn tilde(p: &str, home: &str) -> String {
    let h = home.trim_end_matches('/');
    match p.strip_prefix(h) {
        Some(rest) if !h.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => p.to_string(),
    }
}

/// What rush found, as lines for the research agent, past what
/// care_known says: where the disk goes, what's big and untouched, what
/// plugins report by kind, and the rules and proposals there are (as
/// JSON, as the agent writes them).
pub fn research_known(f: &Found, home: &str, now: Ms) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(map) = &f.map {
        if !map.tops.is_empty() {
            out.push(String::new());
            out.push(format!(
                "Where the disk goes in the home folder, as rush walked it {} (folder · size):",
                super::care::ago(now - map.at)
            ));
            for (p, n) in map.tops.iter().take(TOPS) {
                out.push(format!("- {} · {}", tilde(p, home), size(*n)));
            }
        }
        if !map.untouched.is_empty() {
            out.push(String::new());
            out.push("Big and untouched for a month, no rule names them (path · size · what its name says · last changed):".into());
            for u in &map.untouched {
                let kind = if u.kind.is_empty() {
                    "?"
                } else {
                    u.kind.as_str()
                };
                out.push(format!(
                    "- {} · {} · {kind} · {}",
                    tilde(&u.path, home),
                    size(u.size),
                    super::clean::untouched_for(now - u.newest)
                ));
            }
        }
    }
    let mut kinds: Vec<&str> = f.candidates.iter().map(|c| c.kind.as_str()).collect();
    kinds.sort();
    kinds.dedup();
    if !kinds.is_empty() {
        out.push(String::new());
        out.push("What plugins report, by kind (path · size · what · plugin), the biggest of each; a plugin-candidate rule names the kind:".into());
        for k in kinds {
            let mut of: Vec<&Candidate> = f.candidates.iter().filter(|c| c.kind == k).collect();
            of.sort_by_key(|c| std::cmp::Reverse(c.bytes_on_disk));
            let total: u64 = of.iter().map(|c| c.bytes_on_disk).sum();
            out.push(format!("{k}: {} in {}", size(total as i64), of.len()));
            for c in of.iter().take(PER_KIND) {
                out.push(format!(
                    "- {} · {} · {} · {}",
                    tilde(&c.path.to_string_lossy(), home),
                    size(c.bytes_on_disk as i64),
                    if c.why.is_empty() {
                        "?"
                    } else {
                        c.why.as_str()
                    },
                    c.plugin
                ));
            }
        }
    }
    out.push(String::new());
    match &f.rules {
        Err(e) => out.push(format!(
            "The engine's rules can't be read ({e}): rush can't keep proposals until they can; say so in your report."
        )),
        Ok(r) => {
            out.push("The engine's rules, as JSON (propose none of these again):".into());
            for rule in &r.rules {
                if let Ok(j) = serde_json::to_string(rule) {
                    out.push(format!("- {j}"));
                }
            }
            if !r.proposed.is_empty() {
                out.push("Proposed already, waiting for the user (propose none of these again):".into());
                for p in &r.proposed {
                    if let Ok(j) = serde_json::to_string(&p.rule) {
                        out.push(format!("- {j}"));
                    }
                }
            }
        }
    }
    out
}

/// An example of the research file, as the agent writes it.
const EXAMPLE: &str = r#"[
  {"rule": {"id": "cache:Library/Caches/com.spotify.client", "title": "Spotify's cache", "kind": {"type": "cache-dir", "path": "~/Library/Caches/com.spotify.client", "why": "Spotify's cache; Spotify downloads it again"}, "low": {"idleSecs": 604800}},
   "evidence": "~/Library/Caches/com.spotify.client: 9.1G (du), Spotify's streaming cache (its bundle id), last changed 3 days ago; Spotify fetches what it plays again, nothing of the user's is in it."},
  {"rule": {"id": "pnpm-store", "title": "pnpm's store, pruned", "kind": {"type": "tool-command", "command": ["pnpm", "store", "prune"], "why": "pnpm's store: what no project uses"}, "low": {}},
   "evidence": "~/Library/pnpm/store/v10: 14.2G; node_modules are hard links into it, so removing them frees little until it's pruned; prune keeps what projects use."}
]"#;

/// The research agent's task.
pub fn research_prompt(known: &[String], reports: &[CleanupReport]) -> String {
    let mut b = String::new();
    b += "Research what fills this disk and propose rules for rush's disk engine, which the user accepts or not in Settings › Disk. rush started you because the user asked it to (Research now). ";
    b += "Delete, remove, stop, prune and run nothing that changes the disk: you read, and write one file. No rule you propose runs until the user accepts it.\n\n";
    b += "Survey what rush knows (below), then look further: where the space goes in the home folder, ~/Library/Caches, ~/.cache, ~/Library/Developer and the package managers' folders, and what made each big folder (its name, a bundle id, a tool's config, what's in it). These run without asking: du, df, ls, stat, git status, git worktree list, git rev-list, git branch (listing it only), xcrun simctl list, and writing files in $TMPDIR; run each plainly as its own command (git in the repository's folder, nothing chained after it, no redirection, a path with a space in it quoted). Anything else asks the user, so keep to these.\n\n";
    b += "A rule is about one of these, as JSON (the engine's kinds):\n";
    b += "- {\"type\": \"cache-dir\", \"path\": \"~/Library/Caches/<app>\", \"why\": \"<what makes it again>\"}: a folder its tool makes again, removed whole once nothing in it changed for idleSecs (0: whenever the rule runs); \"each\": true makes it each folder in it named in hex (a workspace's hash). Only a folder directly in ~/Library/Caches or ~/.cache, or two or more below home; never in ~/.colima, ~/.codex/sessions, ~/.claude, ~/Library/Application Support, ~/Library/Containers, ~/Library/Group Containers, ~/Pictures or ~/.ollama (rush never touches them), Documents, Desktop, Downloads, Movies or Music, rush's own folders, or a git checkout.\n";
    b += &format!(
        "- {{\"type\": \"tool-command\", \"command\": [\"pnpm\", \"store\", \"prune\"], \"why\": \"...\"}}: a tool's own clean-up, run once each time the engine runs at a level it has; its program, named bare, one of {}. Anything else is refused. pnpm's store is always this, pnpm store prune, never a folder removed whole: projects' node_modules are hard links into it.\n",
        KNOWN_TOOLS.join(", ")
    );
    b += "- {\"type\": \"plugin-candidate\", \"kind\": \"<kind>\"}: what plugins report of that kind (listed below), each removed whole once untouched for idleSecs, unless a session works there. Never js.pnpm-store: that's pnpm store prune, above.\n";
    b += "- {\"type\": \"cargo-target\"}, {\"type\": \"cargo-artifacts\"}, {\"type\": \"worktree\"}: rush's own, listed below; propose one only to change its numbers, with its id.\n";
    b += &format!(
        "A rule runs at the levels it has numbers for: \"normal\" (under {}G free) and \"low\" (under {}G free), each {{\"idleSecs\": N}} (cargo-artifacts: ageSecs, keep, supersededSecs); a level left out is one it doesn't run at. Prefer low only, and days idle, for what costs a rebuild or a download.\n\n",
        disk::NORMAL_UNDER >> 30,
        disk::LOW_UNDER >> 30
    );
    b += &format!(
        "Write your proposals as $TMPDIR/{PROPOSALS_FILE}, the one file you write: a JSON array, each {{\"rule\": {{\"id\", \"title\", \"kind\", \"normal\" and/or \"low\"}}, \"evidence\": \"...\"}}. The id of a folder's rule is cache: and its path below home, as rush's are; the title a few words. The evidence, in a few sentences: the paths, each with its size, what made them and how you know, when they last changed, and why it's safe to go (what makes it again; what's lost is a rebuild or a download, never work). For example:\n{EXAMPLE}\n"
    );
    b += "Propose nothing the rules below already do or that's waiting already, and only what loses no work: what you're unsure of goes in your report, never a rule. rush checks each proposal when you end and leaves out any it can't run as written.\n\n";
    b += "End with a short report, under 12 lines: what you proposed and about how much each would free, and the biggest things you found but left, each with its path, size and why.\n\n";
    b += &format!("What rush knows:\n{}\n", known.join("\n"));
    for r in reports {
        b += &format!("\nWhat {} reports:\n{}\n", r.title, r.report.trim());
    }
    b
}

/// What a cleanup agent left for rush to take in once it ended: its plan's
/// caches proposed (rush_fleet::learn) and its research's proposals
/// (disk::propose_file), each file once. Off the UI.
pub fn take_in(id: &str, now: Ms) -> (usize, Option<Proposed>, Vec<String>) {
    let dir = PathBuf::from(rush_fleet::temp_dir(id));
    let mut errs = Vec::new();
    let learned = match std::fs::read_to_string(dir.join(CARE_PLAN_FILE)) {
        Ok(t) => rush_fleet::learn(&format!("{id}/{CARE_PLAN_FILE}"), &t).unwrap_or_else(|e| {
            errs.push(format!("couldn't propose the plan's caches: {e}"));
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    let research = disk::propose_file(
        &dir.join(PROPOSALS_FILE),
        RESEARCH_BY,
        now,
        &format!("{id}/{PROPOSALS_FILE}"),
    )
    .unwrap_or_else(|e| {
        errs.push(format!("couldn't keep the research's proposals: {e}"));
        None
    });
    (learned.len(), research, errs)
}

/// What take_in brought, in a few words for the news; "" when nothing new
/// was proposed or refused.
pub fn proposed_words(learned: usize, research: Option<&Proposed>) -> String {
    let (added, refused) = research
        .filter(|p| !p.already)
        .map_or((0, &[][..]), |p| (p.added.len(), &p.refused[..]));
    let n = learned + added;
    let mut w = String::new();
    if n > 0 {
        w = format!(
            "disk: {n} rule{} proposed · accept or dismiss in Settings › Disk",
            if n == 1 { "" } else { "s" }
        );
    }
    if let Some((id, why)) = refused.first() {
        let left = format!(
            "{} proposal{} left out ({id}: {why})",
            refused.len(),
            if refused.len() == 1 { "" } else { "s" }
        );
        w = if w.is_empty() {
            format!("disk: {left}")
        } else {
            format!("{w} · {left}")
        };
    }
    w
}
