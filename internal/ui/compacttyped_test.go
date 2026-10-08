package ui

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A typed /compact on a cold cache is summarised by the fast model when it
// safely can, and is Claude Code's own otherwise: on a warm cache (cheaper
// read from it), given instructions, mid-turn, on an old host, or with no
// fast model for the harness.
func TestTypedCompactTakesTheFastPath(t *testing.T) {
	// Kept lazy: run, the fast path would really summarise.
	defer func(old func(func() tea.Msg) tea.Cmd) { cmdOff = old }(cmdOff)
	cmdOff = func(f func() tea.Msg) tea.Cmd { return f }
	for _, tc := range []struct {
		name, kind, state, arg string
		proto                  int
		warm, fast, handled    bool
	}{
		{"idle Claude, cold", "claude", "idle", "", 9, false, true, true},
		{"idle Claude, warm", "claude", "idle", "", 9, true, false, false},
		{"instructions", "claude", "idle", "keep the API notes", 9, false, false, false},
		{"mid-turn", "claude", "working", "", 9, false, false, false},
		{"old host", "claude", "idle", "", 8, false, false, false},
		{"no fast model", "codex", "idle", "", 9, false, false, false},
		{"asked for native", "claude", "idle", "native", 9, false, false, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			m := &Model{store: &state.Store{}}
			c := &hostConn{key: "k", client: &host.Client{}, sess: &convo.Session{Info: host.Info{Kind: tc.kind, Proto: tc.proto, State: tc.state}}}
			last := time.Now().Add(-3 * time.Hour)
			if tc.warm {
				last = time.Now().Add(-5 * time.Minute)
			}
			c.sess.Requests = []convo.Request{{At: last}}
			m.host = c
			cmd, ok := m.compactTyped(c, &fleet.Agent{Kind: tc.kind, ID: "k"}, tc.arg)
			if ok != tc.handled || ok && cmd == nil {
				t.Fatalf("handled %v (cmd %v), want %v", ok, cmd != nil, tc.handled)
			}
			if c.sess.Compacting() != tc.fast {
				t.Fatalf("fast path %v, want %v", c.sess.Compacting(), tc.fast)
			}
		})
	}
}
