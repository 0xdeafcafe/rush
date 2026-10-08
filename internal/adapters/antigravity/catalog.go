package antigravity

import (
	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
	"os"
	"path/filepath"
	"time"
)

type runnerSnapshot struct {
	At     time.Time
	Ready  bool
	Models []agent.Choice
}

func runnerSnapshotPath() string { return filepath.Join(state.Dir(), "antigravity-catalog.json") }
func writeRunnerSnapshot(data []byte, ready bool) {
	snap := runnerSnapshot{At: time.Now(), Ready: ready}
	if ready {
		snap.Models = parseModels(string(data))
	}
	// Only model IDs/descriptions and availability are persisted, never CLI output
	// or credentials. WriteJSON atomically replaces the private local snapshot.
	_ = state.WriteJSON(runnerSnapshotPath(), snap)
}
func (Adapter) RunnerSnapshot() ([]agent.Choice, bool) {
	b, err := os.ReadFile(runnerSnapshotPath())
	if err != nil {
		return nil, false
	}
	var s runnerSnapshot
	if jsonx.Unmarshal(b, &s) != nil || !s.Ready || time.Since(s.At) > 24*time.Hour || s.At.After(time.Now()) {
		return nil, false
	}
	return s.Models, true
}

var _ agent.RunnerSnapshot = Adapter{}
