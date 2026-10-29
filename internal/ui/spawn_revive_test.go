package ui

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A spawned agent revived after a restart has rush's note to carry on as
// its prompt: its step still says what it was asked, not the note.
func TestRevivedSpawnKeepsWhatItWasAsked(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir()) // records only: no TUI starts on it
	d := filepath.Join(host.Root(), "kid")
	if err := os.MkdirAll(d, 0o700); err != nil {
		t.Fatal(err)
	}
	cfg, _ := jsonx.Marshal(host.Config{ID: "kid", Kind: "claude", Cwd: t.TempDir(), Prompt: "Your session was stopped mid-turn when rush restarted. Carry on where you left off.", Meta: map[string]string{"spawnedBy": "mum", "spawnStep": "s1"}})
	ex, _ := jsonx.Marshal(event.Exchange{ID: "x1", Phase: "request", Text: "review the rust port", At: time.Now()})
	if os.WriteFile(filepath.Join(d, "config.json"), cfg, 0o600) != nil || os.WriteFile(filepath.Join(d, "exchanges.jsonl"), append(ex, '\n'), 0o600) != nil {
		t.Fatal("writing records")
	}
	runs, _ := hostedRuns([]host.Info{{ID: "kid", SessionID: "s", StartedAt: time.Now(), State: "working", Meta: map[string]string{"spawnedBy": "mum", "spawnStep": "s1"}}}, "mum", nil)
	if len(runs) != 1 || runs[0].sp.Prompt != "review the rust port" || runs[0].s.Name != "review the rust port" {
		t.Fatalf("runs %+v", runs)
	}
}
