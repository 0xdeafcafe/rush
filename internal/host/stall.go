package host

import (
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// A turn can stall: the model's stream goes silent and never ends, so no
// error comes for retry to act on. rush never ends an agent on its own, so
// it only says so: a turn that has heard nothing from its agent for
// stallAfter, with no tool running and nothing asked, is marked stalled
// (Info.Stalled), which puts it under Stuck? and tells you once. The next
// word from the agent, its turn's end or your message clears it.
const stallAfter = 5 * time.Minute

// heardFrom notes that the agent said something, and which tool calls it
// has open: one running can be silent as long as it likes. Only the main
// agent's count (a subagent runs inside its parent's call), and a new
// message from the model closes those of earlier ones: it only answers
// once it has every result, so a result rush never saw can't keep a
// stalled turn from showing. Called with mu held.
func (s *server) heardFrom(ev event.Event) {
	s.heard = time.Now()
	if !s.info.Stalled.IsZero() {
		s.info.Stalled = time.Time{}
		s.event("stall-ended", nil)
		s.publish()
	}
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

// quietTurn ends what heardFrom keeps for a turn: a new one starts
// unheard and unstalled. Called with mu held.
func (s *server) quietTurn() {
	s.heard, s.open, s.info.Stalled = time.Now(), nil, time.Time{}
}

// silent is whether the turn has heard nothing for stallAfter with
// nothing running or asked. Called with mu held.
func (s *server) silent(now time.Time) bool {
	return s.conn != nil && s.info.State == "working" && len(s.open) == 0 && len(s.pending) == 0 &&
		!s.heard.IsZero() && now.Sub(s.heard) >= stallAfter
}

// watchStalls marks a stalled turn, once a minute looking. It never stops
// the turn or its agent: you decide.
func (s *server) watchStalls() {
	t := time.NewTicker(time.Minute)
	defer t.Stop()
	for {
		select {
		case <-s.quit:
			return
		case now := <-t.C:
			s.mu.Lock()
			s.markStall(now)
			s.mu.Unlock()
		}
	}
}

// markStall marks the turn stalled once it has gone silent. Called with
// mu held.
func (s *server) markStall(now time.Time) {
	if s.info.Stalled.IsZero() && s.silent(now) {
		s.info.Stalled = s.heard
		s.event("stalled", map[string]any{"quiet": now.Sub(s.heard).Round(time.Second).String()})
		s.publish()
	}
}
