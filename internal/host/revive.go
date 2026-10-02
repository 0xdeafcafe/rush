package host

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// reviveNote is the message a revived agent gets: its turn was cut off, not finished.
const reviveNote = "Your session was stopped mid-turn when rush restarted. Carry on where you left off."

// reviveWithin is how recently a lost session must have been heard from to
// be brought back: older ones were left that way, and stay stopped.
const reviveWithin = 24 * time.Hour

// midTurn is whether a session's last word said it was in a turn: working,
// or asking something. A host that said so and is gone was killed.
func midTurn(i Info) bool {
	return !i.Sleeping && (i.State == "working" || i.State == "blocked")
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
	return ensure(info.ID, reviveNote)
}
