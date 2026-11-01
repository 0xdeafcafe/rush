//! Agents a session ran from its shell (claude -p, codex exec) or with a
//! spawn_agent call, linked to the step that ran them by evidence. After
//! Go's internal/ui/spawns.go (spawnPrefix :28, spawnEvery/Grace/Slop
//! :33, hostedLive :257, claim :291, names :328, spawnDir :487,
//! findSpawn :508, samePrompt :563, sameDir :573). claim is what the
//! looking asks of each session rush hosted; find_spawn looks for one it
//! didn't by the session its agent wrote.

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

/// An agent a step ran, to look for by the session it wrote
/// (spawnWant): what the step says of it, the folder it ran in, and when
/// it could have begun.
#[derive(Clone, Debug, Default)]
pub struct Want {
    pub step: String,
    pub sp: rush_convo::render::Spawn,
    pub dir: String,
    pub start: Ms,
    pub end: Ms,
}

/// The agent step st ran, as it says: a spawn_agent call's, a harness's
/// subagent kept apart (input.child), or a shell command's (Step.Spawn,
/// spawn.go:472). kind is the session's own, for one that doesn't say.
pub fn step_spawn(
    reg: &rush_core::adapter::Registry,
    st: &rush_convo::Step,
    kind: &str,
) -> Option<rush_convo::render::Spawn> {
    let mut sp = if let Some(sp) = rush_convo::render::tool_run(st) {
        sp
    } else if st.kind == rush_core::tool::Kind::Subagent {
        let i = &st.call().input;
        if i.child.is_empty() {
            return None;
        }
        rush_convo::render::Spawn {
            name: rush_convo::model::agent_name(st),
            prompt: if i.prompt.is_empty() {
                i.description.clone()
            } else {
                i.prompt.clone()
            },
            child: i.child.clone(),
            ..Default::default()
        }
    } else if st.kind == rush_core::tool::Kind::Shell {
        rush_convo::render::spawn_of(reg, &st.call().input.command)?
    } else {
        return None;
    };
    if sp.kind.is_empty() {
        sp.kind = kind.to_string(); // spawnOf
    }
    Some(sp)
}

/// Where a spawned agent ran: the folder its command went to, against
/// the session's own, base (spawnDir).
pub fn spawn_dir(base: &str, d: &str) -> String {
    let mut d = d.to_string();
    if let Some(rest) = d.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        d = format!("{home}/{rest}");
    }
    if d.is_empty() {
        return base.to_string();
    }
    if d.starts_with('/') || base.is_empty() {
        return clean(&d);
    }
    clean(&format!("{base}/{d}"))
}

/// A path with its . and .. and doubled slashes worked out, as Go's
/// filepath.Clean.
fn clean(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !abs {
                    parts.push("..");
                }
            }
            _ => parts.push(c),
        }
    }
    let j = parts.join("/");
    match (abs, j.is_empty()) {
        (true, _) => format!("/{j}"),
        (false, true) => ".".into(),
        (false, false) => j,
    }
}

/// Whether a session's first words are what the command asked: the start
/// of it, as agents clip and fold what they keep. A prompt not known (fed
/// from a file) matches anything (samePrompt).
pub fn same_prompt(asked: &str, got: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (a, g) = (norm(asked), norm(got));
    if a.is_empty() {
        return true;
    }
    let n = a.len().min(g.len()).min(60);
    n > 0 && a.as_bytes()[..n] == g.as_bytes()[..n]
}

/// Whether two folders are the same, their links followed (sameDir).
fn same_dir(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The session a spawned agent wrote: one of its agent's, begun while
/// its command ran, asked what the command asked, and in the folder it
/// ran in when that's known (findSpawn, spawns.go:508). mine are the
/// session's own profiles, where its spawns likely wrote; parent its id,
/// for a harness's subagent kept apart; own and taken are transcripts
/// that are someone else's. Reads the disk.
pub fn find_spawn(
    reg: &rush_core::adapter::Registry,
    w: &Want,
    mine: &[rush_core::adapter::Profile],
    parent: &str,
    own: &str,
    taken: &std::collections::HashSet<String>,
    now: Ms,
) -> Option<rush_core::adapter::Session> {
    let (from, to) = (w.start - SPAWN_SLOP, w.end + SPAWN_SLOP);
    let fits = |s: &rush_core::adapter::Session| {
        !(s.transcript.is_empty() || s.transcript == own || taken.contains(&s.transcript))
            && s.created_at >= from
            && s.created_at <= to
            && same_prompt(&w.sp.prompt, &s.name)
    };
    let a = reg.get(rush_core::adapter::migrated(&w.sp.kind).as_str())?;
    let mut profiles = mine.to_vec();
    profiles.extend(a.profiles());
    if !w.sp.child.is_empty() {
        // Its children keep no sessions rush can find without it.
        let f = a.child_finder()?;
        return profiles.iter().find_map(|p| {
            f.find_child(p, parent, &w.sp.child, w.start)
                .filter(|s| !taken.contains(&s.transcript))
        });
    }
    if let Some(f) = a.spawn_finder() {
        return f.find_spawn(&profiles, &w.dir, w.start, &fits);
    }
    let d = a.discoverer()?;
    let mut best: Option<rush_core::adapter::Session> = None;
    for p in a.profiles() {
        let mut list = d.live(&p);
        if now - w.end > 60_000 {
            list.extend(d.past(&p));
        }
        for s in list {
            if !fits(&s) || !s.cwd.is_empty() && !w.dir.is_empty() && !same_dir(&s.cwd, &w.dir) {
                continue;
            }
            if best.as_ref().is_none_or(|b| s.created_at < b.created_at) {
                best = Some(s);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session's first words are the start of what the command asked,
    /// folded and clipped as agents keep them; a prompt not known matches
    /// anything (samePrompt).
    #[test]
    fn same_prompt_by_its_start() {
        for (asked, got, want) in [
            ("fix the build", "fix the  build\nplease", true),
            ("fix the build", "fix the", true),
            ("fix the build", "write docs", false),
            ("", "anything", true),
            ("fix", "", false),
        ] {
            assert_eq!(same_prompt(asked, got), want, "{asked:?} {got:?}");
        }
    }

    /// Where a spawned agent ran: its command's folder against the
    /// session's, cleaned; the session's own when it says none
    /// (spawnDir).
    #[test]
    fn spawn_dirs() {
        for (base, d, want) in [
            ("/w/app", "", "/w/app"),
            ("/w/app", "sub/../x", "/w/app/x"),
            ("/w/app", "/tmp//t/", "/tmp/t"),
            ("", "rel/./d", "rel/d"),
        ] {
            assert_eq!(spawn_dir(base, d), want, "{base:?} {d:?}");
        }
    }
}
