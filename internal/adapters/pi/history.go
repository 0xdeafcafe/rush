package pi

import (
	"fmt"
	"io"
	"os"
	"slices"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

var _ agent.HistoryReader = Adapter{}

// History reads the session file at s.Transcript, or else the one of
// session s.ID in s.Profile, as the events a live session sends: the branch
// of its tree it's on now. When before isn't zero, it stops at the first
// entry written at or after it.
func (Adapter) History(s agent.Session, before time.Time) ([]event.Event, error) { //nolint:gocritic // HistoryReader takes the session by value
	if s.Transcript == "" && s.ID != "" {
		s.Transcript = sessionFile(s.Profile, s.ID)
	}
	if s.Transcript == "" {
		return nil, fmt.Errorf("pi: session %s has no file", s.ID)
	}
	f, err := os.Open(s.Transcript)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	return readSession(f, before)
}

func readSession(r io.Reader, before time.Time) ([]event.Event, error) {
	var header entry
	var all []entry
	byID := map[string]int{}
	err := readLines(r, func(b []byte) bool {
		var e entry
		if jsonx.Unmarshal(b, &e) != nil {
			return true // a line cut short while pi writes it
		}
		if e.Type == "session" {
			header = e
			return true
		}
		if !before.IsZero() {
			if t := e.time(); !t.IsZero() && !t.Before(before) {
				return false
			}
		}
		if e.ID != "" {
			byID[e.ID] = len(all)
		}
		all = append(all, e)
		return true
	})
	return replay(header, branch(all, byID)), err
}

// branch is the entries from the root to the last one written, the tip of
// the branch the session is on: those it left by going back in its tree
// aren't in the conversation any more.
func branch(all []entry, byID map[string]int) []entry {
	if len(all) == 0 {
		return nil
	}
	var rev []entry
	seen := map[string]bool{}
	for i := len(all) - 1; i >= 0; {
		e := all[i]
		if seen[e.ID] {
			break
		}
		seen[e.ID] = true
		rev = append(rev, e)
		j, ok := byID[e.ParentID]
		if e.ParentID == "" || !ok {
			break
		}
		i = j
	}
	slices.Reverse(rev)
	return rev
}

// replay is a branch's entries as events: an Init, then each message, a
// TurnEnd wherever the agent stopped to wait on the user.
func replay(header entry, entries []entry) []event.Event {
	init := event.Init{SessionID: header.ID, Cwd: header.Cwd}
	out := []event.Event{init}
	var r run
	for i := range entries {
		e := &entries[i]
		switch e.Type {
		case "model_change":
			if init.Model == "" {
				init.Model = e.ModelID
				out[0] = init
			}
		case "compaction":
			out = append(out, event.Compacted{Before: e.TokensBefore})
		case "message":
			if e.Message != nil {
				out = append(out, r.message(e)...)
			}
		}
	}
	return out
}

// run is what a replay keeps of the run being read: from a message of
// yours to the model's last word.
type run struct {
	open   bool
	start  time.Time
	tokens usage.TokenUsage
	cost   float64 // the session's, all told
	last   string
}

// message is an entry's message as events, and the run's end when it's
// the model's last word.
func (r *run) message(e *entry) []event.Event {
	m := e.Message
	id := e.ID
	if m.Role == "assistant" {
		id = m.id()
	}
	if !r.open && (m.Role == "user" || m.Role == "assistant") {
		r.open, r.start, r.tokens, r.last = true, e.time(), usage.TokenUsage{}, ""
	}
	out := m.events(id)
	if m.Role != "assistant" {
		return out
	}
	if m.Usage != nil {
		r.tokens.Add(m.Usage.usage())
		r.cost += m.Usage.Cost.Total
	}
	if t := m.text(); t != "" {
		r.last = t
	}
	if m.StopReason == "toolUse" || m.StopReason == "" {
		return out
	}
	end := event.TurnEnd{Reason: reason(m.StopReason), Text: r.last, Cost: r.cost, Tokens: r.tokens, Turns: 1}
	if m.StopReason == "error" {
		end.Err = m.ErrorMessage
	}
	if t := e.time(); !r.start.IsZero() && t.After(r.start) {
		end.Duration = t.Sub(r.start)
	}
	r.open = false
	return append(out, end)
}
