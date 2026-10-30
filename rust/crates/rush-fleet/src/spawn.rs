//! Agents that sessions ran from their shells, listed with the session
//! that ran them rather than as rows of their own. Port of
//! internal/fleet/fleet.go:1586-1797 (spawn, foldSpawns, hostedSpawns,
//! addSpend, lookForAskers, promptNeedle, wrote).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};

use rush_core::agent::{Agent, State};
use rush_core::time::{Ms, now, parse_rfc3339};
use rush_sys::proc::{Pid, Table};

use crate::hosted::HostInfo;
use crate::loader::Loader;
use crate::machine::{ran_by, spawn_of};

/// How far back a finished headless run is looked for in the sessions
/// that may have run it, and how long before it began its command may
/// have been written.
const ASK_FOR: Ms = 30 * 24 * 3_600_000;
const ASKED_BEFORE: Ms = 10 * 60_000;

/// A headless run not seen running, and the sessions that may have run
/// it: those working when it began, less other runs like it.
pub(crate) struct Asking {
    key: String,
    needle: String,
    from: Ms,
    to: Ms,
    keys: Vec<String>,
    paths: Vec<String>,
}

/// Counts what a run it folds in cost with its own.
fn add_spend(a: &mut Agent, cost: f64, today: f64) {
    a.spend.cost += cost;
    a.spend.today += today;
}

impl Loader {
    /// Links the rush sessions an agent's shell ran (rush spawn) to the
    /// session that ran them, when that's a rush session too; one run by
    /// another is found by its process, while it runs.
    pub(crate) fn hosted_spawns(
        &mut self,
        hosted: &[HostInfo],
        agents: &[Agent],
        mut spawned: Vec<(String, Pid)>,
    ) -> Vec<(String, Pid)> {
        let by_id: HashMap<&str, &Agent> = agents
            .iter()
            .filter(|a| a.rush)
            .map(|a| (a.job.id.as_str(), a))
            .collect();
        for info in hosted {
            let (Some(by), Some(a)) = (info.meta.get("spawnedBy"), by_id.get(info.id.as_str()))
            else {
                continue;
            };
            if let Some(p) = by_id.get(by.as_str()) {
                let (k, pk) = (a.key.clone(), p.key.clone());
                self.link(&k, &pk, "spawnedBy");
            } else if a.pid != 0 {
                spawned.push((a.key.clone(), a.pid));
            }
        }
        spawned
    }

    /// Takes the agents sessions' shells ran out of the list, each counted
    /// with the subagents of the session that ran it. One running is found
    /// by its process being under that session's; one finished, by that
    /// having been seen before. spawned are those left out already.
    pub(crate) fn fold_spawns(
        &mut self,
        tab: Option<&Table>,
        mut agents: Vec<Agent>,
        mut spawned: Vec<(String, Pid)>,
        parents: &mut HashSet<Pid>,
    ) -> Vec<Agent> {
        let mut by_key: HashMap<String, usize> = HashMap::new();
        let mut by_pid: HashMap<Pid, usize> = HashMap::new();
        let mut pids: Vec<Pid> = vec![0; agents.len()];
        {
            let mut progs = self.prog_procs(tab);
            for (i, a) in agents.iter().enumerate() {
                by_key.insert(a.key.clone(), i);
                let mut pid = a.pid;
                // Another agent's session says no process: a codex exec
                // quiet for a while is listed as past while it still runs.
                if pid == 0 && (a.interactive && !a.past || a.headless) && !a.remote && !a.rush {
                    pid = progs.proc_of(a.kind.as_str(), a.job.created_at);
                }
                if pid != 0 {
                    pids[i] = pid;
                    by_pid.insert(pid, i);
                    parents.insert(pid);
                }
            }
        }
        for (i, a) in agents.iter().enumerate() {
            if a.headless && pids[i] != 0 && spawn_of(tab, pids[i], parents) != 0 {
                spawned.push((a.key.clone(), pids[i]));
            }
        }
        self.look_for_askers(&agents);
        let mut gone: HashMap<String, usize> = HashMap::new(); // by key, the session each ran under
        for (key, pid) in &spawned {
            if let Some(p) = ran_by(tab, *pid, &by_pid) {
                agents[p].subs.direct += 1;
                agents[p].subs.spawned += 1;
                gone.insert(key.clone(), p);
                let pk = agents[p].key.clone();
                self.link(key, &pk, "process");
            }
        }
        let mut keep = vec![true; agents.len()];
        for i in 0..agents.len() {
            let a = &agents[i];
            // One asking you something is never folded out of sight:
            // answering it is yours, and its parent is held until you do.
            if a.job.state == State::Blocked && !a.checking {
                continue;
            }
            let (cost, today, live) = (a.spend.cost, a.spend.today, a.job.live());
            if let Some(&p) = gone.get(&a.key) {
                add_spend(&mut agents[p], cost, today);
                keep[i] = false;
                continue;
            }
            // ponytail: one level only; a spawn's own spawns fold into it,
            // and out of sight once it has folded too. One a rush session
            // started (spawnedBy) may still be at work.
            if let Some(&p) = self.links.get(&a.key).and_then(|pk| by_key.get(pk))
                && p != i
                && !gone.contains_key(&agents[p].key)
            {
                agents[p].subs.spawned += 1;
                if live {
                    agents[p].subs.direct += 1; // its parent is busy, not your turn, while it works
                }
                add_spend(&mut agents[p], cost, today);
                keep[i] = false;
            }
        }
        let mut k = keep.into_iter();
        agents.retain(|_| k.next().unwrap_or(true));
        agents
    }

    /// Looks, in the background, for the sessions that ran the headless
    /// runs not seen running, by their prompts in those sessions'
    /// transcripts shortly before they began: each is linked when found,
    /// and folded by the next load. Each run is looked for once.
    /// ponytail: matches the prompt's opening words, so a prompt fed from a
    /// file isn't found; the UI links those when their session is opened.
    fn look_for_askers(&mut self, agents: &[Agent]) {
        let t = now();
        let mut want = Vec::new();
        for a in agents {
            let needle = prompt_needle(&a.job.name);
            if !a.headless
                || a.remote
                || self.asked_before(&a.key)
                || needle.is_empty()
                || a.job.created_at == 0
                || t - a.job.created_at > ASK_FOR
            {
                continue;
            }
            if self.links.contains_key(&a.key) {
                continue; // linked, or looked for before
            }
            self.mark_asked(&a.key);
            let mut w = Asking {
                key: a.key.clone(),
                needle: needle.to_string(),
                from: a.job.created_at - ASKED_BEFORE,
                to: a.job.created_at + 5_000,
                keys: Vec::new(),
                paths: Vec::new(),
            };
            for p in agents {
                let path = if p.job.transcript_path.is_empty() {
                    &p.history
                } else {
                    &p.job.transcript_path
                };
                if p.key != a.key
                    && (!p.headless || p.kind != a.kind)
                    && !p.job.name.starts_with(needle)
                    && !path.is_empty()
                    && p.job.created_at <= w.to
                    && p.job.updated_at >= w.from
                {
                    w.keys.push(p.key.clone());
                    w.paths.push(path.clone());
                }
            }
            want.push(w);
        }
        if want.is_empty() {
            return;
        }
        let h = self.handle();
        let work = move || {
            for w in &want {
                let parent = w
                    .paths
                    .iter()
                    .position(|p| wrote(p, w.needle.as_bytes(), w.from, w.to))
                    .map(|j| w.keys[j].clone())
                    .unwrap_or_default(); // none: not looked for again
                h.link_found(&w.key, &parent);
            }
        };
        if self.ask_inline {
            work();
        } else {
            let _ = std::thread::Builder::new()
                .name("rush-askers".into())
                .spawn(work);
        }
    }
}

/// The opening of a prompt as a transcript would hold it, up to anything
/// JSON would escape; empty when that's too short to tell by.
pub(crate) fn prompt_needle(prompt: &str) -> &str {
    let n = prompt
        .find(['"', '\\', '<', '>', '&', '…'])
        .unwrap_or(prompt.len())
        .min(48);
    if n < 20 {
        return "";
    }
    // At most 48 bytes, cut on a character's edge.
    let mut n = n;
    while !prompt.is_char_boundary(n) {
        n -= 1;
    }
    &prompt[..n]
}

/// Whether a transcript has a line holding needle stamped between from
/// and to.
pub(crate) fn wrote(path: &str, needle: &[u8], from: Ms, to: Ms) -> bool {
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    const MARK: &[u8] = b"\"timestamp\":\"";
    let mut r = BufReader::with_capacity(64 << 10, f);
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        if line.len() > 64 << 20 || memchr::memmem::find(&line, needle).is_none() {
            continue;
        }
        let Some(i) = memchr::memmem::find(&line, MARK) else {
            continue;
        };
        let rest = &line[i + MARK.len()..];
        let Some(j) = rest.iter().position(|&c| c == b'"') else {
            continue;
        };
        let Ok(ts) = std::str::from_utf8(&rest[..j]) else {
            continue;
        };
        let t = parse_rfc3339(ts);
        if t != 0 && t >= from && t <= to {
            return true;
        }
    }
}
