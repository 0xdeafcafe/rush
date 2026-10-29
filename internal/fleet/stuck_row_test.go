package fleet

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A message waiting on a turn that can't take it puts the row under Needs
// you, saying so, until the turn is stopped or the queue goes.
func TestStuckQueueNeedsYou(t *testing.T) {
	l := NewLoader(&state.Store{})
	now := time.Now()
	info := host.Info{ID: "kq", Kind: "unknown-test", State: "working", Queue: []string{"stop now"}, QueueStuck: now, Cwd: t.TempDir(), UpdatedAt: now}
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); a.State != "blocked" || a.Needs != StuckNeeds {
		t.Fatalf("stuck: state %q needs %q", a.State, a.Needs)
	}
	info.ID, info.QueueStuck = "kr", time.Time{}
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); a.State != "working" || a.Needs != "" {
		t.Fatalf("not stuck: state %q needs %q", a.State, a.Needs)
	}
}
