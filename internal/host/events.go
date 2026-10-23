package host

import (
	"path/filepath"
	"slices"

	"github.com/0xdeafcafe/rush/internal/events"
)

// A session's events.jsonl is what rush did with it and why, a line each:
// the state it went into, the messages it was given and by whom, the
// check-ins, the turns' ends, its agent starting and exiting, a command
// stopped as stuck. It's there to debug rush by, not to show: when a
// session did something odd, it says what rush decided and when. Each
// line is in rush's own events.jsonl too (see package events).

// EventsPath is where a session's events are kept.
func EventsPath(id string) string { return filepath.Join(dir(id), "events.jsonl") }

// logEvent records an event of kind of the session's, with fields. It
// never waits: any process may log, the UI's too (a revive comes from it).
func logEvent(id, kind string, fields map[string]any) {
	if id == "" {
		return
	}
	events.Session(id, EventsPath(id), kind, fields)
}

// LogEvent is logEvent for another process: the UI's, of what it saw of a
// session (a pull request it opened, say).
func LogEvent(id, kind string, fields map[string]any) { logEvent(id, kind, fields) }

// event logs an event of this session's.
func (s *server) event(kind string, fields map[string]any) { logEvent(s.cfg.ID, kind, fields) }

// shown is what of Info the events follow: a change in any is logged.
type shown struct {
	State    string   `json:"state"`
	Needs    string   `json:"needs,omitempty"`
	Error    string   `json:"error,omitempty"`
	Sleeping bool     `json:"sleeping,omitempty"`
	Limit    *Limit   `json:"limit,omitempty"`
	Retry    *Retry   `json:"retry,omitempty"`
	Tasks    []string `json:"background,omitempty"`
	Queue    int      `json:"queue,omitempty"`
	Held     bool     `json:"queueHeld,omitempty"`
	Away     bool     `json:"away,omitempty"`
}

func shownOf(i *Info) shown {
	sh := shown{State: i.State, Needs: i.Needs, Error: i.Error, Sleeping: i.Sleeping, Queue: len(i.Queue), Held: i.QueueHeld, Away: i.Away.On()}
	if i.Limit != nil {
		l := *i.Limit
		sh.Limit = &l
	}
	if i.Retry != nil {
		r := *i.Retry
		sh.Retry = &r
	}
	for _, t := range i.Background {
		sh.Tasks = append(sh.Tasks, t.ID)
	}
	return sh
}

func (a shown) same(b shown) bool {
	return a.State == b.State && a.Needs == b.Needs && a.Error == b.Error && a.Sleeping == b.Sleeping &&
		a.Queue == b.Queue && a.Held == b.Held && a.Away == b.Away && slices.Equal(a.Tasks, b.Tasks) &&
		(a.Limit == nil) == (b.Limit == nil) && (a.Limit == nil || *a.Limit == *b.Limit) &&
		(a.Retry == nil) == (b.Retry == nil) && (a.Retry == nil || *a.Retry == *b.Retry)
}

// noteState logs the session's state when it changed since last logged.
// Called with mu held, from publish.
func (s *server) noteState() {
	now := shownOf(&s.info)
	if s.logged != nil && s.logged.same(now) {
		return
	}
	s.logged = &now
	s.event("state", map[string]any{"info": now})
}

// clip is the start of a message, enough to know it by.
func clip(text string) string {
	r := []rune(text)
	if len(r) <= 300 {
		return text
	}
	return string(r[:300]) + "…"
}

// errText is err's message, or none.
func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// quietOps are asked of a session all the time, and change nothing.
var quietOps = []string{"send", "exchange", "hello", "context"}

// noteOp logs an op a client asked of the session (a setting changed, a
// stop, a rewind, an account move) and what came of it.
func (s *server) noteOp(o op, err error) {
	if slices.Contains(quietOps, o.Op) {
		return
	}
	f := map[string]any{"op": o.Op}
	put := func(k string, v any, set bool) {
		if set {
			f[k] = v
		}
	}
	put("id", o.ID, o.ID != "")
	put("mode", o.Mode, o.Mode != "")
	put("model", o.Model, o.Model != "")
	put("effort", o.Effort, o.Effort != "")
	put("text", clip(o.Text), o.Text != "")
	put("message", clip(o.Message), o.Message != "")
	put("index", o.Index, o.Index != 0)
	put("to", o.To, o.To != 0)
	put("without", o.Without, len(o.Without) > 0)
	put("away", o.Away, o.Op == "away")
	put("interrupt", o.Interrupt, o.Interrupt)
	put("always", o.Always, o.Always)
	put("err", errText(err), err != nil)
	put("cause", o.Cause, o.Cause != "")
	s.event("setting", f)
}
