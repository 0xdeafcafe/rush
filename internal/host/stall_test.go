package host

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// A turn silent for stallAfter with nothing running is marked stalled and
// never stopped; a tool running, a question asked, or a word from the
// agent keeps it from showing, and the next word clears it.
func TestStalledTurnIsShownNeverStopped(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	c := &stopWatchConn{fakeConn: fakeConn{events: make(chan event.Event)}}
	s := &server{cfg: Config{ID: "stall"}, conn: c, info: Info{State: "working"}, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now()

	call := tool.Call{ID: "c1", Name: "shell"}
	s.heardFrom(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.ToolCall, Call: &call}}})
	if s.markStall(now.Add(time.Hour)); !s.info.Stalled.IsZero() {
		t.Fatal("stalled while a tool runs")
	}
	s.heardFrom(event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: "c1"}}}})
	if s.markStall(now.Add(time.Minute)); !s.info.Stalled.IsZero() {
		t.Fatal("stalled a minute after its tool returned")
	}
	s.pending["ap"] = asked{}
	if s.markStall(now.Add(time.Hour)); !s.info.Stalled.IsZero() {
		t.Fatal("stalled while it asks you something")
	}
	delete(s.pending, "ap")

	if s.markStall(now.Add(stallAfter + time.Second)); s.info.Stalled.IsZero() {
		t.Fatal("not stalled after stallAfter of silence")
	}
	if c.interrupted || c.stoppedTask {
		t.Fatalf("rush stopped a stalled turn on its own: interrupted %v, task stopped %v", c.interrupted, c.stoppedTask)
	}

	// A word from the agent ends the stall, as does the turn's end.
	s.heardFrom(event.Message{Role: "assistant", ID: "m2", Parts: []event.Part{{Kind: event.Text, Text: "back"}}})
	if !s.info.Stalled.IsZero() {
		t.Fatal("still stalled once the agent spoke")
	}
	s.heard = now.Add(-time.Hour)
	if s.markStall(now); s.info.Stalled.IsZero() {
		t.Fatal("not stalled again after another silence")
	}
	s.quietTurn()
	if !s.info.Stalled.IsZero() {
		t.Fatal("a new turn starts stalled")
	}
}
