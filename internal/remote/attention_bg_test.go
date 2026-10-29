package remote

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
)

// Idle with work still running in the background isn't finished, so it
// isn't pushed as finished until that work is done.
func TestAttentionWaitsForBackgroundWork(t *testing.T) {
	i := &host.Info{State: "idle", IdleSince: time.Now(), Background: []host.Task{{ID: "wf", Type: "local_workflow", Label: "port"}}}
	if got := attention(i); got != "" {
		t.Fatalf("idle with a workflow running: %q, want nothing", got)
	}
	i.Background = nil
	if got := attention(i); got != "finished" {
		t.Fatalf("idle with nothing running: %q, want finished", got)
	}
}
