package host

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// A background subagent that ends while the session is idle starts a turn
// of the agent's own, which rush didn't send: until the agent is seen to
// begin it, and until it ends, a reload doesn't restart the host under it.
func TestNoticeKeepsItFromRestart(t *testing.T) {
	setup(t)
	c := &fakeConn{events: make(chan event.Event, 8)}
	s := &server{cfg: Config{ID: "wake", Kind: "fake"}, conn: c, clients: map[*conn]struct{}{},
		pending: map[string]asked{}, options: map[string][]event.Option{}}
	s.info = Info{State: "idle", Exe: BinStamp{Mod: time.Now().Add(-time.Hour)}}
	installed := BinStamp{Mod: time.Now()}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.onAgentEvent(c, event.TaskStarted{ID: "a1"})
	s.onAgentEvent(c, event.TaskDone{ID: "a1"})
	s.onAgentEvent(c, event.Background{Tasks: []event.BackgroundTask{}})
	if got := s.info; RestartDue(got, installed) {
		t.Fatalf("restart due as the agent takes up the subagent's report: %+v", got)
	}
	// A subagent still writing isn't the agent beginning its turn.
	s.onAgentEvent(c, event.Message{Role: "assistant", Parent: "toolu_x", Parts: []event.Part{{Kind: event.Text, Text: "still going"}}})
	if got := s.info; RestartDue(got, installed) {
		t.Fatal("a subagent's message made it quiet")
	}
	s.onAgentEvent(c, event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{{Kind: event.Text, Text: "The lane's back"}}})
	if got := s.info; got.State != "working" || RestartDue(got, installed) {
		t.Fatalf("mid-turn: %+v", got)
	}
	s.onAgentEvent(c, event.TurnEnd{Reason: "done", Turns: 1, Text: "Merged."})
	if got := s.info; !RestartDue(got, installed) {
		t.Fatalf("its turn over, nothing else going: %+v", got)
	}
	// One that never begins on it (Claude Code took nothing up) holds
	// the restart only so long.
	if i := (Info{State: "idle", Waking: time.Now().Add(-2 * unansweredFor)}); !i.Quiet() {
		t.Fatal("a notice long past keeps it from restarting for good")
	}
}
