package convo

import (
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
)

// applyNeutral folds in an event from any agent: Claude Code's come here
// too, read as rush's own.
func (s *Session) applyNeutral(ev event.Event, now time.Time) {
	if t := s.Live(); t != nil && now.After(t.Heard) {
		t.Heard = now
	}
	switch e := ev.(type) {
	case event.Exchange:
		s.exchange(e, now)
	case event.Init:
		s.Model, s.Cwd = e.Model, e.Cwd
		s.Version, s.MCP, s.NTools = e.Version, e.MCP, len(e.Tools)
		if e.Effort != "" {
			s.Info.Effort = e.Effort
		}
	case event.Limited:
		s.Limit = "rejected"
	case event.Context:
		s.Context = e.Tokens
		if e.Window > 0 {
			s.Window = e.Window
		}
	case event.StartNotice:
		s.addHooks([]Hook{{Event: firstNonEmpty(e.Event, "SessionStart"), Text: e.Text}})
	case event.PartStart:
		s.partStart(e.Kind, now)
	case event.Delta:
		s.delta(&e, now)
	case event.Compacted:
		s.compacted(&e, now)
	case event.Message:
		if s.userMessage(&e, now) {
			return
		}
		if t := s.Live(); t != nil && e.Role == "assistant" {
			t.Retry = nil // the model answered
		}
		s.message(&e, now)
	case event.CallUpdated:
		if st := s.byID[e.Call.ID]; st != nil {
			st.Tool, st.Input = stepTool(&e.Call)
			st.setCall(&e.Call)
			s.touchStep(st)
		}
	case event.Approval:
		s.ensureStep(e.Call, now)
		i := slices.IndexFunc(e.Options, func(o event.Option) bool { return o.Kind == event.AllowAlways })
		as := ""
		if i >= 0 {
			as = e.Options[i].Label
		}
		s.ask(e.Call.ID, &Asking{ID: e.ID, Reason: e.Reason, Path: e.Path, Always: i >= 0, AlwaysAs: as})
	case event.Question:
		id := firstNonEmpty(e.CallID, e.ID)
		s.ensureStep(tool.Call{ID: id, Name: "AskUserQuestion", Kind: tool.Question}, now)
		q := e
		s.ask(id, &Asking{ID: e.ID, Question: &q})
	case event.ApprovalCancelled:
		s.settle(e.ID)
	case event.Denied:
		if st := s.byID[e.CallID]; st != nil {
			st.Status, st.Output, st.End = Denied, e.Reason, now
			s.touchStep(st)
		}
	case event.TaskStarted, event.TaskUpdated, event.TaskProgress, event.TaskDone, event.Background:
		s.applyJob(ev, now)
	case event.Plan:
		b, _ := jsonx.Marshal(map[string]any{"todos": claudeTodos(e.Todos)})
		s.tasksFromInput(&Step{Tool: "TodoWrite", Input: b})
	case event.Status:
		switch {
		case e.Text == "compacting" && s.compacting.IsZero():
			s.compacting = now
			if t := s.Live(); t != nil {
				t.touch()
			}
		case e.Text != "compacting":
			s.compacting = time.Time{}
		}
	case event.Retry:
		if t := s.Live(); t != nil {
			r := e
			t.Retry, t.RetryAt = &r, now
			t.touch()
		}
	case event.TurnEnd:
		if t := s.Live(); t != nil {
			t.Retry = nil
		}
		s.turnEnd(&e, now)
	}
}

// partStart opens a part of the message being written: thinking shows as
// it starts, since it often streams nothing readable.
func (s *Session) partStart(k event.PartKind, now time.Time) {
	t := s.turnFor(now)
	t.Thinking, t.Retry = time.Time{}, nil // the model is answering
	if k == event.Thinking {
		t.Thinking = now
		if n := len(t.Items); n == 0 || t.Items[n-1].Kind != KThinking {
			t.Items = append(t.Items, &Item{Kind: KThinking})
		}
	}
	if k != event.Text {
		s.streaming = nil
	}
	t.touch()
}

// delta is a piece of the message being written.
func (s *Session) delta(e *event.Delta, now time.Time) {
	t := s.turnFor(now)
	t.Streamed += len(e.Text)
	switch e.Kind {
	case event.ToolCall:
	case event.Thinking:
		if n := len(t.Items); n == 0 || t.Items[n-1].Kind != KThinking {
			t.Items = append(t.Items, &Item{Kind: KThinking})
		}
		t.Items[len(t.Items)-1].grow(e.Text)
	default:
		if s.streaming == nil {
			s.streaming = &Item{Kind: KText, At: now}
			t.Items = append(t.Items, s.streaming)
		}
		s.streaming.grow(e.Text)
	}
	t.touch()
}

// compacted is a divider in the turn it happened in (or the last one),
// and the context starts again from what the summary left.
func (s *Session) compacted(e *event.Compacted, now time.Time) {
	t := s.Live()
	if t == nil && len(s.Turns) > 0 {
		t = s.Turns[len(s.Turns)-1]
	}
	if t == nil {
		t = s.turnFor(now)
	}
	if !s.compacting.IsZero() && e.Before > 0 {
		s.compactRate = now.Sub(s.compacting) / time.Duration(e.Before)
	}
	s.compacting = time.Time{}
	c := *e
	t.Items = append(t.Items, &Item{Kind: KCompact, Compact: &c})
	if e.After > 0 {
		s.Context = e.After
	}
	t.touch()
}

// turnEnd ends the running turn: stopped by you, done, or gone wrong.
func (s *Session) turnEnd(e *event.TurnEnd, now time.Time) {
	if e.Reason == "interrupted" {
		s.interrupted(now)
		return
	}
	s.endJobs(now)
	if s.woke != nil {
		s.wokeAt = now // held until now: this is when it wakes the agent
	}
	t := s.Live()
	if t == nil {
		return
	}
	s.endTurn(t, now)
	t.Cost = host.TurnCost(&s.spent, e.Cost)
	switch {
	case e.Reason != "" && e.Reason != "done" && e.Reason != "error":
		t.Err = strings.ReplaceAll(e.Reason, "_", " ")
	case e.Err != "" || e.Reason == "error":
		t.Err = firstNonEmpty(firstLine(e.Err), "error")
	}
	// The agent's own copy of the error (a limit's, say) is the ✗ row's.
	if n := len(t.Items); t.Err != "" && n > 0 && t.Items[n-1].Kind == KText && firstLine(strings.TrimSpace(t.Items[n-1].Text)) == t.Err {
		t.Items = t.Items[:n-1]
	}
}

// ensureStep makes a step of a call an approval or question is about,
// when the agent asks before it says it's making the call.
func (s *Session) ensureStep(c tool.Call, now time.Time) {
	if c.ID == "" || s.byID[c.ID] != nil {
		return
	}
	s.message(&event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.ToolCall, Call: &c}}}, now)
}
