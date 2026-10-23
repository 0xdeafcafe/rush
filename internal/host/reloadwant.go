package host

import (
	"encoding/json"
	"os"
	"path/filepath"

	"github.com/0xdeafcafe/rush/internal/state"
)

// A reload moves the sessions too: those between turns at once, and a
// note (reload-agents in rush's folder) has the leading view restart the
// rest onto the same rush as each turn ends. The note names the rush it
// asked for, so installing another one since ends it.

func reloadNote() string { return filepath.Join(state.Dir(), "reload-agents") }

// WantReload asks for every session's host to be restarted onto the rush
// stamped installed, each once it is between turns.
func WantReload(installed BinStamp) error {
	b, err := json.Marshal(installed)
	if err != nil {
		return err
	}
	return os.WriteFile(reloadNote(), b, 0o600)
}

// ReloadWanted is whether a reload onto the rush stamped installed still
// has sessions to move.
func ReloadWanted(installed BinStamp) bool {
	b, err := os.ReadFile(reloadNote())
	if err != nil {
		return false
	}
	var want BinStamp
	if json.Unmarshal(b, &want) != nil || !want.Mod.Equal(installed.Mod) || want.Size != installed.Size {
		_ = os.Remove(reloadNote())
		return false
	}
	return true
}

// ReloadDone ends the reload: every session is on the rush it asked for.
func ReloadDone() { _ = os.Remove(reloadNote()) }

// ReloadAgents restarts every session's host on an older rush than the
// one stamped installed that is between turns, and asks for the rest to
// follow as their turns end. What Sweep says it did, it returns.
func ReloadAgents(l *Lister, installed BinStamp) (stale, restarted, failed int) {
	stale, restarted, failed = l.Sweep(installed, true)
	if stale > failed {
		_ = WantReload(installed)
	}
	return stale, restarted, failed
}
