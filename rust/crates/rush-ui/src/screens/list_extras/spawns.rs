//! Agents a session ran from its shell (claude -p, codex exec) or with a
//! spawn_agent call, linked to the step that ran them by evidence. After
//! Go's internal/ui/spawns.go (spawnPrefix :28, spawnEvery/Grace/Slop
//! :33, hostedLive :257, claim :291, names :328). The looking (host
//! infos, transcripts) waits on rush hosting sessions; claim is what it
//! asks of each one found.

use rush_core::time::Ms;

/// Marks a spawned agent among the subagent runs (spawnPrefix).
pub const SPAWN_PREFIX: &str = "spawn:";
/// How often spawned agents are looked for (spawnEvery), how long after
/// its command ended a step may still have started one (spawnGrace), and
/// how far clocks may disagree on when one began (spawnSlop).
pub const SPAWN_EVERY: Ms = 2_000;
pub const SPAWN_GRACE: Ms = 10_000;
pub const SPAWN_SLOP: Ms = 3_000;

/// A step's window: the shell command or spawn_agent call it ran, and
/// when (convo.Window).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub step: String,
    pub command: String,
    /// What a spawn_agent call asked; "" for a shell step.
    pub asked: String,
    pub from: Ms,
    pub to: Ms,
}

/// A spawned agent's session, once found (spawnRun): the step that ran
/// it, who it is and what it was asked, its transcript, and, when rush
/// hosts it, the rush session it runs as and whether that still works.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpawnRun {
    pub step: String,
    pub kind: String,
    pub name: String,
    pub prompt: String,
    pub model: String,
    pub path: String,
    pub born: Ms,
    pub hosted: String,
    pub live: bool,
}

/// The spawned agents found, as subagent runs, as they began
/// (spawnSubs). title is what the call that ran a step says it's for.
pub fn spawn_subs<'a>(
    runs: impl IntoIterator<Item = (&'a String, &'a SpawnRun)>,
    title: &dyn Fn(&str) -> String,
) -> Vec<rush_convo::Subagent> {
    let mut out: Vec<rush_convo::Subagent> = runs
        .into_iter()
        .filter(|(_, r)| !r.path.is_empty())
        .map(|(id, r)| {
            let t = title(&r.step);
            let p = crate::style::one_line(&r.prompt);
            rush_convo::Subagent {
                id: format!("{SPAWN_PREFIX}{id}"),
                typ: r.name.clone(),
                description: if !t.is_empty() { t } else { p },
                model: r.model.clone(),
                tool_use_id: r.step.clone(),
                path: r.path.clone(),
                born: r.born,
                ..rush_convo::Subagent::default()
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.born_order()
            .cmp(&b.born_order())
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// The shell steps' and spawn_agent calls' windows in s, in order; one
/// still running (or its task) runs to now, and a grace after
/// (Session.Windows).
pub fn windows(s: &rush_convo::Session, now: Ms, grace: Ms) -> Vec<Window> {
    let mut out = Vec::new();
    for st in s.steps() {
        let spawn = rush_convo::model::runs::is_spawn(&st.tool);
        if st.kind != rush_core::tool::Kind::Shell && !spawn || st.start == 0 {
            continue;
        }
        let mut end = st.end;
        if let Some(j) = s.jobs().iter().find(|j| *j.tool_use_id == *st.id)
            && j.background
        {
            end = j.end;
        }
        if st.status == rush_convo::Status::Running || end == 0 || s.job_running(&st.id) {
            end = now;
        }
        let asked = if spawn {
            serde_json::from_str::<serde_json::Value>(st.input_json())
                .ok()
                .and_then(|v| v.get("prompt").and_then(|p| p.as_str()).map(str::to_string))
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.push(Window {
            step: st.id.to_string(),
            command: st.call().input.command.clone(),
            asked,
            from: st.start,
            to: end + grace,
        });
    }
    out
}

/// Whether a hosted run works on: idle, its turn is done (hostedLive).
pub fn hosted_live(state: &str) -> bool {
    state != "stopped" && state != "idle"
}

/// Whether cmd has prog as a word of its own, a path to it too (names).
pub fn names(cmd: &str, prog: &str) -> bool {
    cmd.split(|c: char| !(c.is_alphanumeric() || "-_.".contains(c)))
        .any(|w| w == prog)
}

/// The step that ran a session of program prog, asked asked, begun at at
/// (claim): of the steps whose window holds it, a spawn_agent call that
/// asked just that, else the shell step naming its program, else the
/// latest to start before it; "" when no window holds it, or none names
/// it while a subagent (whose shell is the session's too) works.
pub fn claim<'a>(wins: &'a [Window], prog: &str, asked: &str, at: Ms, sub_busy: bool) -> &'a str {
    let (mut best, mut named, mut from) = ("", false, 0);
    for w in wins {
        if at < w.from - SPAWN_SLOP || at > w.to {
            continue;
        }
        let mut n = !prog.is_empty() && names(&w.command, prog);
        if !w.asked.is_empty() {
            if w.asked.trim() != asked.trim() {
                continue; // it started another
            }
            n = true;
        }
        if best.is_empty() || n && !named || n == named && w.from > from {
            best = &w.step;
            named = n;
            from = w.from;
        }
    }
    if !named && sub_busy {
        return "";
    }
    best
}
