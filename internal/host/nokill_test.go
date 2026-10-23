package host

import (
	"os"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// stopWatchConn records any stop rush sends on its own.
type stopWatchConn struct {
	fakeConn
	interrupted, stoppedTask bool
}

func (c *stopWatchConn) Interrupt() error         { c.interrupted = true; return nil }
func (c *stopWatchConn) StopTask(id string) error { c.stoppedTask = true; return nil }

// A spawned agent past any token or tool-call count, and a subagent past
// any token count, are never stopped by rush: only you or the agent end them.
func TestSpawnedAgentPastOldCeilingIsNotStopped(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	// The answer is written in the background, and would race the folder's removal.
	defer func(f func(string, time.Time)) { netproofAnswer = f }(netproofAnswer)
	netproofAnswer = func(string, time.Time) {}
	c := &stopWatchConn{fakeConn: fakeConn{events: make(chan event.Event)}}
	s := &server{cfg: Config{ID: "spawned", Meta: map[string]string{"spawnedBy": "parent"}}, conn: c,
		info: Info{State: "working"}, clients: map[*conn]struct{}{}, options: map[string][]event.Option{}, pending: map[string]asked{}}
	if err := os.MkdirAll(dir(s.cfg.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	var parts []event.Part
	for i := range 60 {
		parts = append(parts, event.Part{Kind: event.ToolCall, Call: &tool.Call{ID: string(rune('a' + i))}})
	}
	s.onMessage(c, event.Message{ID: "m1", Role: "assistant", Parts: parts, Tokens: &usage.TokenUsage{Input: 1_000_000, CacheRead: 9_000_000}})
	s.onTask(event.TaskStarted{ID: "child", Kind: event.SubagentTask})
	s.onAgentEvent(c, event.TaskProgress{ID: "child", Tokens: 20_000_000, ToolUses: 200})
	if c.interrupted || c.stoppedTask {
		t.Fatalf("rush stopped work on its own: interrupted %v, task stopped %v", c.interrupted, c.stoppedTask)
	}
}
