package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A stalled agent is noted once a stall, however often the list is read,
// and again only when a new stall begins.
func TestStallNotedOnce(t *testing.T) {
	m := &Model{store: &state.Store{}, lastState: map[string]string{}}
	m.store.Config.Quiet = true // no notification from the test
	now := time.Now()
	a := &fleet.Agent{Key: "k", DisplayName: "kimi"}
	a.State = "working"
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{a}}
	m.notify() // the first read only learns what's there

	stalls := func() int {
		n := 0
		for _, e := range m.events {
			if strings.HasPrefix(e.what, "stalled") {
				n++
			}
		}
		return n
	}
	a.Stalled = now.Add(-5 * time.Minute)
	m.notify()
	m.notify()
	if n := stalls(); n != 1 {
		t.Fatalf("one stall noted %d times", n)
	}
	a.Stalled = time.Time{}
	m.notify()
	a.Stalled = now
	m.notify()
	if n := stalls(); n != 2 {
		t.Fatalf("a second stall: %d notes, want 2", n)
	}
}
