package host

import (
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/events"
)

// sent is a message as it went to the agent, and when.
type sent struct {
	in    agent.Input
	at    time.Time
	again bool // sent a second time already
}

// heardWait is how long after an empty turn the agent's own record is read
// for the message: long enough for it to have written it, had it taken it.
var heardWait = time.Second

// checkHeard looks again at a message the agent ended a turn without
// reading (no request made, nothing said). A Claude Code just resumed on a
// turn a restart cut off can end that turn and drop what came in as it
// started: unless its transcript has the message, it goes again, once.
// Called with mu held; the transcript is read off it.
func (s *server) checkHeard(conn agent.Conn, e event.TurnEnd) {
	u := s.unheard
	h, ok := conn.(agent.Hearer)
	if u == nil || u.again || !ok || e.Turns != 0 || e.Err != "" || strings.TrimSpace(e.Text) != "" {
		return
	}
	go func() {
		time.Sleep(heardWait)
		heard, known := h.Heard(u.in.Text, u.at)
		s.mu.Lock()
		defer s.mu.Unlock()
		if heard || !known || s.conn != conn || s.unheard != u || s.info.State != "idle" || s.info.Sleeping {
			return
		}
		u.again = true
		ev := events.ID()
		s.turn++
		s.info.Turn, s.turnEv = s.turn, ev
		s.event("redelivered", map[string]any{"ev": ev, "turn": s.turn, "text": clip(u.in.Text), "images": len(u.in.Images),
			"why": "the agent ended a turn without reading it"})
		s.disarmIdle()
		if err := conn.Send(u.in); err != nil {
			s.event("deliver-failed", map[string]any{"ev": ev, "text": clip(u.in.Text), "err": err.Error()})
			s.armIdle()
			return
		}
		s.info.State, s.info.Detail = "working", ""
		s.waiting = time.Now()
		s.publish()
	}()
}
