package host

import (
	"os"
	"strings"
	"testing"
	"time"

	// Kimi and the other ACP agents, whose turn can't take a message.
	_ "github.com/0xdeafcafe/rush/internal/adapters/acp"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// stopConn is a turn that never ends by itself: it takes what it's sent
// and counts the times it was asked to stop.
type stopConn struct {
	inputConn
	stops int
}

func (c *stopConn) Interrupt() error { c.stops++; return nil }

// Kimi's turn refuses a second prompt, so what has waited queueLate behind
// one isn't handed to it as Claude Code's is: the session waits on you,
// and sending it now stops the turn, after which it goes.
func TestLateQueueWaitsOnYouWhenTheTurnCantTakeIt(t *testing.T) {
	setup(t)
	if takesMidTurn("kimi") || !takesMidTurn("claude") || !takesMidTurn("") {
		t.Fatal("Kimi can't take a message mid-turn; Claude Code can")
	}
	was := queueLate
	queueLate = 50 * time.Millisecond
	defer func() { queueLate = was }()
	c := &stopConn{}
	s := &server{cfg: Config{ID: "kq", Kind: "kimi"}, conn: c, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	s.info.State = "working"
	if err := s.send("stop now", nil, false); err != nil {
		t.Fatal(err)
	}
	time.Sleep(150 * time.Millisecond)
	s.mu.Lock()
	if len(c.got) != 0 || len(s.info.Queue) != 1 || s.info.QueueStuck.IsZero() {
		t.Fatalf("sent %v into a turn that refuses it; queue %q, stuck %v", c.got, s.info.Queue, s.info.QueueStuck)
	}
	s.mu.Unlock()

	if err := s.do(op{Op: "queue_send", Index: 0, Was: "stop now"}); err != nil {
		t.Fatal(err)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if c.stops != 1 || len(c.got) != 0 {
		t.Fatalf("send now: %d stops, sent %v mid-turn", c.stops, c.got)
	}
	s.onTurnEnd(c, event.TurnEnd{Reason: "interrupted"})
	if len(c.got) != 1 || c.got[0].Text != "stop now" || len(s.info.Queue) != 0 || !s.info.QueueStuck.IsZero() {
		t.Fatalf("after the stop: sent %v, queue %q, stuck %v", c.got, s.info.Queue, s.info.QueueStuck)
	}
}

// Held on purpose, the queue isn't stuck: it waits on you already.
func TestHeldQueueIsNotStuck(t *testing.T) {
	setup(t)
	s := &server{cfg: Config{ID: "kh", Kind: "kimi"}, conn: &stopConn{}, clients: map[*conn]struct{}{}}
	s.info.State, s.info.Queue, s.info.QueueStuck = "working", []string{"a"}, time.Now()
	s.info.QueueHeld = true
	s.publish()
	if !s.info.QueueStuck.IsZero() {
		t.Fatal("a held queue shows as stuck")
	}
}

// Rush's own check-in stops a turn that can't take it, rather than
// waiting in the queue behind the very turn it asks about; Claude Code's
// turn is steered with it, nothing stopped.
func TestCheckInStopsATurnThatCantTakeIt(t *testing.T) {
	setup(t)
	c := &stopConn{}
	s := &server{cfg: Config{ID: "kc", Kind: "kimi"}, conn: c, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	s.info.State = "working"
	cut, err := s.checkIn("these have gone quiet")
	if err != nil || !cut || c.stops != 1 || len(s.info.Queue) != 1 {
		t.Fatalf("cut %v err %v stops %d queue %q", cut, err, c.stops, s.info.Queue)
	}

	cc := &stopConn{}
	s = &server{cfg: Config{ID: "cc", Kind: "claude"}, conn: cc, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	s.info.State = "working"
	if cut, err := s.checkIn("these have gone quiet"); err != nil || cut || cc.stops != 0 || len(cc.got) != 1 {
		t.Fatalf("claude: cut %v err %v stops %d got %v", cut, err, cc.stops, cc.got)
	}
}

// A message to a session whose turn can't take it says so to its sender,
// not just "Sent".
func TestStuckBehindTellsTheSender(t *testing.T) {
	setup(t)
	for _, kind := range []string{"kimi", "claude"} {
		s := &server{cfg: Config{ID: "sb" + kind, Kind: kind}, clients: map[*conn]struct{}{}}
		if err := os.MkdirAll(dir(s.cfg.ID), 0o700); err != nil {
			t.Fatal(err)
		}
		s.info = Info{ID: s.cfg.ID, Kind: kind, Name: "lanes", HostPID: os.Getpid(), State: "working", Queue: []string{"stop now"}}
		s.publish()
		note := stuckBehind(s.cfg.ID)
		if want := kind == "kimi"; (note != "") != want || want && !strings.Contains(note, "lanes is mid-turn") {
			t.Errorf("%s: %q", kind, note)
		}
	}
}
