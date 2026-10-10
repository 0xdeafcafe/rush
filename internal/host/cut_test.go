package host

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// An agent rush let go (to move it to another login) that took up a
// finished subagent's report as it went never leaves the session
// "working" with no agent: its turn is carried on by a new one.
func TestCutAgentsTurnCarriesOn(t *testing.T) {
	setup(t)
	old := &fakeConn{events: make(chan event.Event, 4)}
	s := &server{
		cfg:     Config{ID: "cut", Kind: "fake", SessionID: "s1", Account: agent.Profile{Kind: "fake", Dir: t.TempDir()}},
		info:    Info{State: "idle", Tasks: 1},
		conn:    old,
		clients: map[*conn]struct{}{},
		pending: map[string]asked{},
		options: map[string][]event.Option{},
	}
	s.taskStart = map[string]time.Time{"t1": time.Now()}

	// The subagent ends: its notice is a turn not yet begun.
	s.mu.Lock()
	s.onAgentEvent(old, event.TaskDone{ID: "t1"})
	busy := s.stillWorking()
	s.detach() // rush moves it anyway
	s.mu.Unlock()
	if !busy {
		t.Fatal("a finished subagent's notice left the agent counted quiet")
	}

	done := make(chan struct{})
	go func() { s.watchAgent(old); close(done) }()
	old.events <- event.Message{Role: "assistant", ID: "m9", Parts: []event.Part{{Kind: event.Text, Text: "The mapper's back"}}}
	close(old.events)
	<-done

	s.mu.Lock()
	defer s.mu.Unlock()
	if s.conn == nil {
		t.Fatalf("the cut turn wasn't carried on: state %q with no agent", s.info.State)
	}
	if s.info.State != "working" && s.info.State != "blocked" {
		t.Fatalf("state %q after carrying on", s.info.State)
	}
}
