//! What a reading makes of a session whose host has gone, and what a
//! session brought back is told: carry on its turn, or start again the
//! work of the background agents that went with its host.
//!
//! Ported from internal/host/revive.go:11-77 (reviveNote, cutNote,
//! cutAgentsNote, IsRushNote, midTurn, agentTask, lose, reviveText); the
//! revive itself is hosts.rs.

use crate::wire::{Info, Task, mid_turn};

/// The message a revived agent gets: its turn was cut off, not finished.
pub const REVIVE_NOTE: &str =
    "Your session was stopped mid-turn when rush restarted. Carry on where you left off.";

/// The message an agent gets when rush restarted it just as it took up
/// something on its own (a subagent's report, say).
pub const CUT_NOTE: &str =
    "Your session was restarted just as you began a turn. Carry on where you left off.";

/// The start of the message a revived agent gets when its turn had ended
/// but its background agents were still at work: they went with its host,
/// so their work stopped where it was.
pub const CUT_AGENTS_NOTE: &str = "Your session was stopped when rush restarted, and the background agents still running went with it: ";

/// Whether a session's prompt is rush's own word to carry on after a
/// restart, not anything it was asked.
pub fn is_rush_note(prompt: &str) -> bool {
    prompt == REVIVE_NOTE || prompt == CUT_NOTE || prompt.starts_with(CUT_AGENTS_NOTE)
}

/// Whether a background task is an agent at work, not a shell or a
/// monitor it watches.
pub fn agent_task(t: &Task) -> bool {
    matches!(
        t.kind.as_str(),
        "local_agent" | "remote_agent" | "in_process_teammate"
    )
}

/// What a reading makes of a session whose host has gone: stopped, unless
/// it went to sleep, and with nothing running, as what it ran went with
/// it. It's lost (to be brought back) when it went mid-turn, or when its
/// turn had ended with background agents still at work (Info::cut_agents).
pub fn lose(i: &mut Info) {
    i.claude_pid = 0;
    if !i.is_remote() {
        // nothing here to bring back
        i.lost = mid_turn(i);
        for t in &i.background {
            if agent_task(t) && !i.lost && !i.sleeping {
                i.cut_agents.push(t.label.clone());
            }
        }
        i.lost = i.lost || !i.cut_agents.is_empty();
    }
    i.background = Vec::new();
    i.tasks = 0;
    if !i.sleeping {
        i.state = "stopped".into();
    }
}

/// What a revived session is told: to carry on its turn, or to start
/// again the work of the background agents that went with it.
pub fn revive_text(i: &Info) -> String {
    if i.cut_agents.is_empty() {
        return REVIVE_NOTE.into();
    }
    format!(
        "{CUT_AGENTS_NOTE}{}. Start again what they hadn't finished.",
        i.cut_agents.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, kind: &str, label: &str) -> Task {
        Task {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    fn info(id: &str, state: &str, background: Vec<Task>) -> Info {
        Info {
            id: id.into(),
            state: state.into(),
            background,
            ..Default::default()
        }
    }

    // revive_test.go:53 (TestLoseCutAgents): a host gone after its turn
    // ended, its background agents still at work, is lost too: they went
    // with it, and it's told to start their work again. Its shells and
    // monitors alone don't bring it back, and a reading of it shows nothing
    // running.
    #[test]
    fn lose_cut_agents() {
        let mut agents = info(
            "a",
            "idle",
            vec![
                task("1", "local_agent", "Merge the branches"),
                task("2", "local_bash", "cargo test"),
            ],
        );
        agents.tasks = 2;
        lose(&mut agents);
        assert!(
            agents.lost
                && agents.state == "stopped"
                && agents.background.is_empty()
                && agents.tasks == 0,
            "idle with an agent at work: {agents:?}"
        );
        let got = revive_text(&agents);
        assert!(
            got.starts_with(CUT_AGENTS_NOTE)
                && got.contains("Merge the branches")
                && !got.contains("cargo test")
                && is_rush_note(&got),
            "told: {got:?}"
        );

        let mut shells = info(
            "b",
            "idle",
            vec![task("1", "monitor_mcp", ""), task("2", "local_bash", "")],
        );
        lose(&mut shells);
        assert!(
            !shells.lost && shells.background.is_empty(),
            "idle with only a shell and a monitor: {shells:?}"
        );

        let mut turn = info("c", "working", vec![task("1", "local_agent", "x")]);
        lose(&mut turn);
        assert!(
            turn.lost && revive_text(&turn) == REVIVE_NOTE,
            "mid-turn with an agent: {turn:?}"
        );

        let mut asleep = info("d", "idle", vec![task("1", "local_agent", "")]);
        asleep.sleeping = true;
        lose(&mut asleep);
        assert!(!asleep.lost && asleep.state == "idle", "asleep: {asleep:?}");

        let mut remote = info("e", "idle", vec![task("1", "local_agent", "")]);
        remote.remote = "other".into();
        lose(&mut remote);
        assert!(!remote.lost, "another machine's: {remote:?}");
    }

    #[test]
    fn rush_notes() {
        assert!(is_rush_note(REVIVE_NOTE) && is_rush_note(CUT_NOTE));
        assert!(!is_rush_note("fix the build"));
    }
}
