package fleet

import (
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
	"strings"
	"testing"
	"time"
)

func TestStoppedRowsRefreshHostStateAndOverlay(t *testing.T) {
	s := &state.Store{}
	s.Overlay.Names = map[string]string{}
	l := NewLoader(s)
	p := agent.Profile{Name: "test"}
	now := time.Now()
	info := host.Info{ID: "idle", Kind: "unknown-test", Name: "Original", State: "stopped", Cwd: t.TempDir(), UpdatedAt: now}
	a := l.hostedAgent(p, info, nil, now)
	a.DisplayName = "caller mutation"
	key := state.Key(p.Name, "a:"+info.ID)
	s.Overlay.Names[key] = "Renamed"
	if got := l.hostedAgent(p, info, nil, now.Add(time.Second)); got.DisplayName != "Renamed" {
		t.Fatal(got.DisplayName)
	}
	delete(s.Overlay.Names, key)
	l.spend[key] = Spend{Cost: 42, Ready: true}
	if got := l.hostedAgent(p, info, nil, now.Add(2*time.Second)); got.DisplayName != "Original" || got.Spend.Cost != 42 {
		t.Fatalf("stale overlay/spend: %+v", got)
	}
	info.UpdatedAt = now.Add(3 * time.Second)
	info.Name = "Changed by host"
	if got := l.hostedAgent(p, info, nil, now.Add(3*time.Second)); got.DisplayName != info.Name {
		t.Fatal("host update delayed")
	}
	info.State = "working"
	info.Detail = "running a task"
	if got := l.hostedAgent(p, info, nil, now.Add(4*time.Second)); got.State != "working" || got.Detail != info.Detail {
		t.Fatal("waking session cached")
	}
	if len(l.idleRows) != 0 {
		t.Fatal("live row retained in idle cache")
	}
}

func TestStoppedRowsExpireAndQuickLoadDoesNotFillCache(t *testing.T) {
	l := NewLoader(&state.Store{})
	p := agent.Profile{Name: "test"}
	now := time.Now()
	info := host.Info{ID: "idle", Kind: "unknown-test", Name: "Original", State: "stopped", Cwd: t.TempDir(), UpdatedAt: now}
	l.quick = true
	l.hostedBase(p, info, nil, now)
	if len(l.idleRows) != 0 {
		t.Fatal("quick incomplete reading cached")
	}
	l.quick = false
	l.hostedBase(p, info, nil, now)
	info.Name = "updated external metadata"
	if got := l.hostedBase(p, info, nil, now.Add(pastEvery)); got.DisplayName != info.Name {
		t.Fatal("cache outlived refresh interval")
	}
}

// A sleeping host's pid is the one it had: a load that doesn't sample
// processes mustn't take it as running, or the row jumps into Active.
func TestSleepingRowHasNoProcess(t *testing.T) {
	l := NewLoader(&state.Store{})
	info := host.Info{ID: "zz", Kind: "unknown-test", State: "idle", Sleeping: true, HostPID: 1, Cwd: t.TempDir(), UpdatedAt: time.Now()}
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, time.Now()); a.PID != 0 {
		t.Fatalf("sleeping row has pid %d", a.PID)
	}
}

// A new agent idle before its first turn has nothing finished: it isn't
// your turn, so it doesn't sit under Needs you.
func TestNewHostedAgentIsNotYourTurn(t *testing.T) {
	l := NewLoader(&state.Store{})
	now := time.Now()
	info := host.Info{ID: "nw", Kind: "unknown-test", State: "idle", Cwd: t.TempDir(), StartedAt: now, UpdatedAt: now}
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); a.YourTurn(now) {
		t.Fatal("a new agent is your turn")
	}
	info.IdleSince = now
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); !a.YourTurn(now) {
		t.Fatal("a finished turn isn't your turn")
	}
}

// Idle while you're away, it's told to keep going at its check-in: not
// your turn. Asking something, it still needs you.
func TestAwayIsNotYourTurn(t *testing.T) {
	l := NewLoader(&state.Store{})
	now := time.Now()
	info := host.Info{ID: "aw", Kind: "unknown-test", State: "idle", Cwd: t.TempDir(), StartedAt: now, UpdatedAt: now, IdleSince: now,
		Away: &host.Away{From: now, Until: now.Add(time.Hour), Next: now.Add(time.Minute)}}
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); a.YourTurn(now) || !strings.HasPrefix(a.Detail, "✈ away 1h00m") {
		t.Fatalf("away row: your turn %v, detail %q", a.YourTurn(now), a.Detail)
	}
	info.State = "blocked"
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); a.Seen {
		t.Fatal("a question while away is seen")
	}
	// Over, it's your turn, and says what's waiting for you.
	info.State, info.Detail = "idle", "All done: shipped"
	info.Away.Ended, info.Away.Held = now, []host.Held{{Kind: "question", Text: "Which DB?"}}
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); !a.YourTurn(now) || a.Detail != "✈ back from away · 1 saved for you · All done: shipped" {
		t.Fatalf("back: your turn %v, detail %q", a.YourTurn(now), a.Detail)
	}
	info.Away = &host.Away{Loop: true, Next: now}
	if a := l.hosted(agent.Profile{Name: "test"}, info, nil, now); a.YourTurn(now) || !strings.HasPrefix(a.Detail, "⟳ loop · next check-in") {
		t.Fatalf("loop: your turn %v, detail %q", a.YourTurn(now), a.Detail)
	}
}
