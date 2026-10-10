package host

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// Nudged to relogin without a limit, a running agent that would start again
// just where it is (its cache warm on the login it ran as) isn't restarted:
// that would only stop what it has running.
func TestWarmAgentIsntRestartedForNothing(t *testing.T) {
	home := filepath.Dir(setup(t))
	_ = os.WriteFile(filepath.Join(home, "login"), []byte("b"), 0o600)
	c := &homedConn{fakeConn: &fakeConn{events: make(chan event.Event, 4)}, dir: home, home: "a"}
	s := &server{
		cfg:     Config{ID: "warm", Kind: "homed"},
		info:    Info{State: "idle", Home: "a", HomeAt: time.Now(), CacheWarm: time.Now().Add(time.Hour)},
		conn:    c,
		clients: map[*conn]struct{}{},
		pending: map[string]asked{},
	}
	s.mu.Lock()
	s.relogin(c) // unlocks
	s.mu.Lock()
	if s.conn != c || s.info.Relogin || s.moveNow {
		t.Fatalf("a warm agent was set to restart: conn kept %v, relogin %v, moveNow %v", s.conn == c, s.info.Relogin, s.moveNow)
	}
	s.info.CacheWarm = time.Now().Add(-time.Minute) // cooled: now it follows
	s.relogin(c)
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.info.Relogin {
		t.Fatal("a cold agent off the login in use wasn't set to move")
	}
}
