package host

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// reviveNote is the message a revived agent gets: its turn was cut off, not finished.
const reviveNote = "Your session was stopped mid-turn when rush restarted. Carry on where you left off."

// cutNote is the message an agent gets when rush restarted it just as it
// took up something on its own (a subagent's report, say).
const cutNote = "Your session was restarted just as you began a turn. Carry on where you left off."

// cutAgentsNote begins the message a revived agent gets when its turn had
// ended but its background agents were still at work: they went with its
// host, so their work stopped where it was.
const cutAgentsNote = "Your session was stopped when rush restarted, and the background agents still running went with it: "

// IsRushNote is whether a session's prompt is rush's own word to carry on
// after a restart, not anything it was asked.
func IsRushNote(prompt string) bool {
	return prompt == reviveNote || prompt == cutNote || strings.HasPrefix(prompt, cutAgentsNote)
}

// reviveWithin is how recently a lost session must have been heard from to
// be brought back: older ones were left that way, and stay stopped.
const reviveWithin = 24 * time.Hour

// midTurn is whether a session's last word said it was in a turn: working,
// or asking something. A host that said so and is gone was killed.
func midTurn(i Info) bool {
	return !i.Sleeping && (i.State == "working" || i.State == "blocked")
}

// agentTask is whether a background task is an agent at work, not a shell
// or a monitor it watches.
func agentTask(t Task) bool {
	switch t.Type {
	case "local_agent", "remote_agent", "in_process_teammate":
		return true
	}
	return false
}

// lose is what a reading makes of a session whose host has gone: stopped,
// unless it went to sleep, and with nothing running, as what it ran went
// with it. It's lost (to be brought back) when it went mid-turn, or when
// its turn had ended with background agents still at work (Info.CutAgents).
func lose(i *Info) {
	i.ClaudePID = 0
	if !i.IsRemote() { // nothing here to bring back
		i.Lost = midTurn(*i)
		for _, t := range i.Background {
			if agentTask(t) && !i.Lost && !i.Sleeping {
				i.CutAgents = append(i.CutAgents, t.Label)
			}
		}
		i.Lost = i.Lost || len(i.CutAgents) > 0
	}
	i.Background, i.Tasks = nil, 0
	if !i.Sleeping {
		i.State = "stopped"
	}
}

// reviveText is what a revived session is told: to carry on its turn, or
// to start again the work of the background agents that went with it.
func reviveText(i Info) string {
	if len(i.CutAgents) == 0 {
		return reviveNote
	}
	return cutAgentsNote + strings.Join(i.CutAgents, "; ") + ". Start again what they hadn't finished."
}

// Revive starts a lost session's host again (Info.Lost) on the same
// conversation and tells it to carry on, so it never sits stopped mid-turn.
// At most once every ten minutes per session, so an agent that keeps taking
// its host down isn't restarted in a loop. Never on the UI goroutine.
func Revive(info Info) error {
	if !info.Lost || time.Since(info.UpdatedAt) > reviveWithin || testing.Testing() {
		return nil // a test's records are never started with the test binary
	}
	if cfg, err := ReadConfig(info.ID); err != nil || cfg.Owner > 0 {
		return err // a stand-in's run ended with the program that started it
	}
	mark := filepath.Join(dir(info.ID), "revived")
	if st, err := os.Stat(mark); err == nil && time.Since(st.ModTime()) < 10*time.Minute {
		return nil
	}
	if err := os.WriteFile(mark, nil, 0o600); err != nil {
		return err
	}
	logEvent(info.ID, "revived", map[string]any{"lastState": info.State, "agents": info.CutAgents})
	return ensure(info.ID, reviveText(info), "")
}
