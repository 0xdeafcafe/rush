package ui

import (
	"os"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A turn that ends with agents still running in the background hasn't
// finished: it's noted finished once they're done, not when the turn ends.
func TestFinishedWaitsForBackgroundWork(t *testing.T) {
	m := &Model{store: &state.Store{}, lastState: map[string]string{}}
	m.store.Config.Quiet = true // no notification from the test
	a := &fleet.Agent{Key: "k", DisplayName: "porter"}
	a.State, a.PID = "working", os.Getpid()
	m.snap = &fleet.Snapshot{At: time.Now(), Agents: []*fleet.Agent{a}}
	m.notify() // the first read only learns what's there

	finished := func() int {
		n := 0
		for _, e := range m.events {
			if e.what == "finished" {
				n++
			}
		}
		return n
	}
	m.notify()
	a.State, a.InFlight, a.Background = "done", 1, []string{"task\x00port the next round"}
	m.notify()
	if n := finished(); n != 0 {
		t.Fatalf("noted finished %d times while its workflow runs", n)
	}
	a.InFlight, a.Background = 0, nil
	m.notify()
	if n := finished(); n != 1 {
		t.Fatalf("noted finished %d times once its workflow ended, want 1", n)
	}
}
