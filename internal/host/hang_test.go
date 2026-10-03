package host

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// A turn silent past hangAfter with nothing running is hung; one with a
// tool running, or asking you something, isn't. Stopped, it ends as an
// API error and is retried.
func TestHungTurn(t *testing.T) {
	(&fakeProof{}).install(t)
	s := &server{cfg: Config{ID: "hung"}, conn: &fakeConn{events: make(chan event.Event, 1)}, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	quiet(t, s) // the retry it schedules keeps polling past the test otherwise
	s.info.State = "working"
	now := time.Now()
	call := tool.Call{ID: "c1", Name: "shell"}
	s.heardFrom(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.ToolCall, Call: &call}}})
	if s.hung(now.Add(time.Hour)) {
		t.Fatal("hung while a tool runs")
	}
	s.heardFrom(event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: "c1"}}}})
	if s.hung(now.Add(time.Minute)) {
		t.Fatal("hung a minute after its tool returned")
	}
	if !s.hung(now.Add(hangAfter + time.Second)) {
		t.Fatal("not hung after hangAfter of silence")
	}
	s.pending["ap"] = asked{}
	if s.hung(now.Add(time.Hour)) {
		t.Fatal("hung while it asks you something")
	}
	delete(s.pending, "ap")

	s.mu.Lock()
	s.hangCause = hangReason
	s.onTurnEnd(s.conn, event.TurnEnd{Reason: "interrupted"})
	r := s.info.Retry
	s.mu.Unlock()
	if r == nil || !r.Hung {
		t.Fatalf("a stopped hung turn isn't retried as hung: %+v", r)
	}
}
