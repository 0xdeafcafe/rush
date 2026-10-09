package host

import (
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// A turn can hang: the model's stream goes silent and never ends, so no
// error comes for retry to act on, and the session sits "working" for
// hours with nobody at the screen to stop it. Rush stops such a turn
// itself and tries again as it would after an API error that dropped the
// stream; a subagent's waiting parent is let go meanwhile (awaitAnswer).
const (
	hangAfter  = 5 * time.Minute // nothing from the model, no tool running, this long
	hangGrace  = time.Minute     // for the stopped turn to end before its agent is ended
	hangReason = "API Error: the response stopped arriving: nothing from the model for 5m"
)

// heardFrom notes that the agent said something, and which tool calls it
// has open: one running can be silent as long as it likes. Only the main
// agent's count (a subagent runs inside its parent's call), and a new
// message from the model closes those of earlier ones: it only answers
// once it has every result, so a result rush never saw can't keep a hung
// turn from being stopped. Called with mu held.
func (s *server) heardFrom(ev event.Event) {
	s.heard = time.Now()
	m, ok := ev.(event.Message)
	if !ok || m.Parent != "" {
		return
	}
	if m.Role == "assistant" && m.ID != "" {
		for id, by := range s.open {
			if by != "" && by != m.ID {
				delete(s.open, id)
			}
		}
	}
	for _, p := range m.Parts {
		switch {
		case p.Kind == event.ToolCall && p.Call != nil && p.Call.ID != "":
			if s.open == nil {
				s.open = map[string]string{}
			}
			by := m.ID
			if by == p.Call.ID {
				by = "" // a harness giving each call a message of its own: parallel ones overlap
			}
			s.open[p.Call.ID] = by
		case p.Kind == event.ToolResult && p.Output != nil:
			delete(s.open, p.Output.CallID)
		}
	}
}

// hung is whether the turn has heard nothing for hangAfter with nothing
// running or asked. Called with mu held.
func (s *server) hung(now time.Time) bool {
	return s.conn != nil && s.info.State == "working" && s.hangCause == "" && len(s.open) == 0 &&
		len(s.pending) == 0 && !s.heard.IsZero() && now.Sub(s.heard) >= hangAfter
}

// watchHangs stops a hung turn, once a minute looking: the turn ends as an
// API error (onTurnEnd) and is retried. A turn the stop can't end has its
// agent ended and retried all the same.
func (s *server) watchHangs() {
	t := time.NewTicker(time.Minute)
	defer t.Stop()
	for {
		select {
		case <-s.quit:
			return
		case now := <-t.C:
			s.mu.Lock()
			if !s.hung(now) {
				s.mu.Unlock()
				continue
			}
			conn := s.conn
			s.hangCause = hangReason
			s.mu.Unlock()
			_ = conn.Interrupt()
			time.AfterFunc(hangGrace, func() {
				s.mu.Lock()
				defer s.mu.Unlock()
				if s.conn != conn || s.hangCause == "" {
					return // it ended, or you stepped in
				}
				s.hangCause = ""
				s.retire(s.detach())
				s.retry(hangReason, false)
				s.publish()
			})
		}
	}
}
