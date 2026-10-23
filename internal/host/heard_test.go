package host

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// hearConn is an agent that says whether it took a message in.
type hearConn struct {
	inputConn
	heard bool
}

func (c *hearConn) Heard(string, time.Time) (bool, bool) { return c.heard, true }

// A just-resumed agent that ends a turn without a request and without
// having the message in its record dropped it: it goes again, once. One
// it has, it only hadn't reached yet, doesn't.
func TestDroppedMessageGoesAgain(t *testing.T) {
	setup(t)
	was := heardWait
	heardWait = 0
	t.Cleanup(func() { heardWait = was })
	for _, heard := range []bool{false, true} {
		c := &hearConn{inputConn: inputConn{fakeConn: fakeConn{events: make(chan event.Event, 8)}}, heard: heard}
		s := &server{cfg: Config{ID: "heard"}, conn: c, clients: map[*conn]struct{}{}, pending: map[string]asked{}, options: map[string][]event.Option{}}
		s.info.State = "idle"
		s.mu.Lock()
		if err := s.sendLocked("[Image #1] sometimes sub agent just doesn't show"); err != nil {
			t.Fatal(err)
		}
		s.onAgentEvent(c, event.TurnEnd{Reason: "done", Turns: 0})
		s.mu.Unlock()
		want := 1
		if !heard {
			want = 2
		}
		var got []agent.Input
		for end := time.Now().Add(2 * time.Second); time.Now().Before(end); time.Sleep(10 * time.Millisecond) {
			s.mu.Lock()
			got = append([]agent.Input(nil), c.got...)
			s.mu.Unlock()
			if len(got) == want && heard {
				time.Sleep(50 * time.Millisecond) // and nothing more comes
			}
			if len(got) >= want {
				break
			}
		}
		s.mu.Lock()
		st, n := s.info.State, len(c.got)
		s.mu.Unlock()
		if n != want || got[len(got)-1].Text != got[0].Text {
			t.Fatalf("heard %v: sent %d times, want %d", heard, n, want)
		}
		if !heard && st != "working" {
			t.Fatalf("sent again, the session is %q", st)
		}
		// A second empty turn doesn't send it a third time.
		s.mu.Lock()
		s.onAgentEvent(c, event.TurnEnd{Reason: "done", Turns: 0})
		s.mu.Unlock()
		time.Sleep(50 * time.Millisecond)
		s.mu.Lock()
		if len(c.got) != want {
			t.Fatalf("heard %v: sent %d times after a second empty turn", heard, len(c.got))
		}
		s.mu.Unlock()
	}
}
