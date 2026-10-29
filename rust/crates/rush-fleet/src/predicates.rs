//! What a row's state means: needs you, your turn, halted, stuck. Port
//! of internal/fleet/fleet.go:115-190 (NeedsYou, Waiting, JustFinished,
//! Busy, Age, Elapsed), 1551 (Where), fleet/stall.go, fleet/stuck.go,
//! worktree.go:215 (InUse) and pr.go:19 (PRBranch). Errors are sorted by
//! rush_core::failure, as Go's agent/failure.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

use rush_core::agent::{Agent, State, SubagentTile};
use rush_core::failure;
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::value::RawValue;

const MINUTE: Ms = 60_000;
const HOUR: Ms = 60 * MINUTE;

/// How long until an agent at work that has written nothing shows as
/// stuck; the same for a subagent run, whose prompt cache lasts five
/// minutes, so a longer wait buys nothing.
pub const STUCK_AFTER: Ms = 30 * MINUTE;
pub const STUCK_SUB_AFTER: Ms = 5 * MINUTE;

/// The logins agent's kind: Claude Code (state.LoginsKind).
const LOGINS_KIND: &str = "claude";

/// What a row's state means.
pub trait Predicates {
    /// A live agent asking something the user has not looked at yet.
    fn needs_you(&self) -> bool;
    /// A question the user has seen and left for later.
    fn waiting(&self) -> bool;
    /// A finished turn whose background work is still running in a live
    /// process; subagents still writing count too.
    fn busy(&self) -> bool;
    /// A turn that ended moments ago; it lingers in Working so a finish is
    /// noticed rather than vanishing into history.
    fn just_finished(&self, now: Ms) -> bool;
    /// Its last turn ended on an error: it sits there until told to go on.
    fn halted(&self) -> bool;
    /// A halt in the last day, of a session rush isn't hosting, that
    /// trying again gets past.
    fn continues(&self, now: Ms) -> bool;
    /// A halt trying again gets past that isn't the network.
    fn retryable(&self) -> bool;
    /// A halt the network being down caused.
    fn offline(&self) -> bool;
    /// It finished its turn without asking anything and hasn't been looked
    /// at since: waiting for the next thing to do.
    fn your_turn(&self, now: Ms) -> bool;
    /// A halt said briefly, for a row.
    fn halt_reason(&self, now: Ms) -> String;
    /// What going on means for it: "continue" after an error, else "keep
    /// going".
    fn continue_text(&self) -> &'static str;
    /// At work and quiet, its subagents too, for STUCK_AFTER; or one its
    /// host marked stalled.
    fn stuck(&self, now: Ms) -> bool;
    /// How long since it, or any subagent of it, last wrote.
    fn quiet(&self, now: Ms) -> Ms;
    /// Its subagent runs quiet for STUCK_SUB_AFTER, waiting on the model:
    /// one running a command is stuck only after STUCK_AFTER.
    fn stuck_subs(&self, now: Ms) -> Vec<SubagentTile>;
    /// Time since its last change: what the list prints on the right.
    fn age(&self, now: Ms) -> Ms;
    /// How long it has existed, frozen once it stops.
    fn elapsed(&self, now: Ms) -> Ms;
    /// A session working, or open and not put away: its worktree stays.
    fn in_use(&self) -> bool;
    /// Where an interactive-kind agent is being driven from.
    fn where_from(&self) -> &'static str;
    /// The branch its pull request is found by: its workspace's, or a
    /// linked worktree's it works in; "" in a main checkout.
    fn pr_branch(&self) -> &str;
}

impl Predicates for Agent {
    fn needs_you(&self) -> bool {
        self.job.state == State::Blocked
            && !self.checking
            && (self.pid != 0 || self.decides)
            && !self.seen
    }

    fn waiting(&self) -> bool {
        self.job.state == State::Blocked
            && !self.checking
            && (self.pid != 0 || self.decides)
            && self.seen
    }

    fn busy(&self) -> bool {
        self.pid != 0 && self.job.busy()
            || !self.job.live() && self.subs.direct + self.subs.nested > 0
    }

    fn just_finished(&self, now: Ms) -> bool {
        self.job.state == State::Done && !self.busy() && now - self.job.updated_at < 2 * MINUTE
    }

    fn halted(&self) -> bool {
        self.spend.halt.is_some()
            && !self.job.live()
            && !self.busy()
            && !self.checking
            && !self.past
            && !self.done
            && !self.interactive
            && self.job.state != State::Stopped
    }

    fn continues(&self, now: Ms) -> bool {
        match &self.spend.halt {
            Some(h) if self.halted() && !self.rush && now - h.at < 24 * HOUR => {
                failure::kind_of(&h.text).retries()
            }
            _ => false,
        }
    }

    fn retryable(&self) -> bool {
        self.spend.halt.as_ref().is_some_and(|h| {
            matches!(
                failure::kind_of(&h.text),
                failure::Kind::Transient | failure::Kind::Capacity
            )
        })
    }

    fn offline(&self) -> bool {
        self.spend
            .halt
            .as_ref()
            .is_some_and(|h| failure::kind_of(&h.text) == failure::Kind::Offline)
    }

    fn your_turn(&self, now: Ms) -> bool {
        self.job.state == State::Done
            && !self.checking
            && !self.busy()
            && !self.past
            && !self.done
            && !self.interactive
            && !self.seen
            && self.spend.halt.is_none()
            && now - self.job.updated_at < 24 * HOUR
    }

    fn halt_reason(&self, now: Ms) -> String {
        let Some(h) = &self.spend.halt else {
            return String::new();
        };
        let t = h.text.strip_prefix("API Error: ").unwrap_or(&h.text);
        if t.is_empty() {
            return h.kind.replace('_', " ");
        }
        if h.kind == "rate_limit" {
            // "You've hit your session limit · resets 5am (Europe/London)"
            let t = t.strip_prefix("You've hit your ").unwrap_or(t);
            return match t.find(" (") {
                Some(i) if i > 0 => t[..i].to_string(),
                _ => t.to_string(),
            };
        }
        if self.continues(now) && self.offline() {
            return "offline · continues when the network is back".into();
        }
        if self.continues(now) {
            return format!("{t} · continues by itself");
        }
        t.to_string()
    }

    fn continue_text(&self) -> &'static str {
        if self.spend.halt.is_some() {
            "continue"
        } else {
            "keep going"
        }
    }

    fn stuck(&self, now: Ms) -> bool {
        if self.past
            || self.done
            || self.checking
            || !(self.job.state == State::Working || self.busy())
        {
            return false;
        }
        self.stalled != 0 || self.quiet(now) >= STUCK_AFTER
    }

    fn quiet(&self, now: Ms) -> Ms {
        let mut last = self.job.updated_at.max(self.job.mod_time);
        for s in &self.subagents {
            last = last.max(s.mod_time);
        }
        now - last
    }

    fn stuck_subs(&self, now: Ms) -> Vec<SubagentTile> {
        self.subagents
            .iter()
            .filter(|s| {
                if s.mod_time == 0 || now - s.mod_time < STUCK_SUB_AFTER {
                    return false;
                }
                !(now - s.mod_time < STUCK_AFTER && in_tool(&s.path, s.mod_time))
            })
            .cloned()
            .collect()
    }

    fn age(&self, now: Ms) -> Ms {
        now - self.job.updated_at.max(self.job.mod_time)
    }

    fn elapsed(&self, now: Ms) -> Ms {
        let mut end = now;
        if !self.job.live() {
            end = self.job.updated_at;
            if self.spend.last != 0 && self.spend.last < end {
                end = self.spend.last;
            }
        }
        (end - self.job.created_at).max(0)
    }

    fn in_use(&self) -> bool {
        self.job.live() || self.busy() || self.pid != 0 && !self.done
    }

    fn where_from(&self) -> &'static str {
        if self.headless && self.kind.as_str() == LOGINS_KIND {
            return "run by another program (claude -p)";
        }
        if self.headless {
            return "run by another program";
        }
        if self.remote {
            return "on GitHub";
        }
        "open in a terminal"
    }

    fn pr_branch(&self) -> &str {
        if let Some(w) = &self.workspace {
            return &w.branch;
        }
        if !self.repo.is_empty() && !self.root.is_empty() && self.repo != self.root {
            return &self.branch;
        }
        ""
    }
}

/// What in_tool last found for each transcript, as of when it was last
/// written: a quiet one isn't read again (stuck.go:67).
static IN_TOOL_SEEN: Mutex<Option<HashMap<String, (Ms, bool)>>> = Mutex::new(None);

/// Whether a subagent's transcript, last written at modt, has a tool call
/// still without its result: it's running a command.
fn in_tool(path: &str, modt: Ms) -> bool {
    if path.is_empty() {
        return false;
    }
    let mut seen = IN_TOOL_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let m = seen.get_or_insert_with(HashMap::new);
    if let Some(&(at, inside)) = m.get(path)
        && at == modt
    {
        return inside;
    }
    let inside = ends_in_tool(path);
    if m.len() > 4096 {
        m.clear(); // bounded: a few thousand runs' answers at most
    }
    m.insert(path.to_string(), (modt, inside));
    inside
}

fn ends_in_tool(path: &str) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(md) = f.metadata() else { return false };
    const WINDOW: u64 = 256 << 10;
    let off = md.len().saturating_sub(WINDOW);
    let mut b = Vec::with_capacity((md.len() - off) as usize);
    if f.seek(SeekFrom::Start(off)).is_err() || f.read_to_end(&mut b).is_err() {
        return false;
    }
    #[derive(Deserialize)]
    struct Line<'a> {
        #[serde(borrow, default)]
        message: Option<Msg<'a>>,
    }
    #[derive(Deserialize)]
    struct Msg<'a> {
        #[serde(borrow, default)]
        content: Option<&'a RawValue>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Block {
        #[serde(rename = "type")]
        kind: String,
        id: String,
        tool_use_id: String,
    }
    // Each call is its own line, and each result: one still open, of
    // several made at once, is a command still running.
    let mut open: Vec<String> = Vec::new();
    for ln in b.split(|&c| c == b'\n') {
        let has = |n: &[u8]| memchr::memmem::find(ln, n).is_some();
        if !has(b"\"tool_use\"") && !has(b"\"tool_result\"") {
            continue;
        }
        let Ok(l) = serde_json::from_slice::<Line>(ln) else {
            continue;
        };
        let Some(c) = l.message.and_then(|m| m.content) else {
            continue;
        };
        let Ok(blocks) = serde_json::from_str::<Vec<Block>>(c.get()) else {
            continue;
        };
        for bl in blocks {
            match bl.kind.as_str() {
                "tool_use" => {
                    if !open.contains(&bl.id) {
                        open.push(bl.id)
                    }
                }
                "tool_result" => open.retain(|x| *x != bl.tool_use_id),
                _ => {}
            }
        }
    }
    !open.is_empty()
}
