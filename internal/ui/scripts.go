package ui

import (
	"slices"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/script"
)

// scriptSeen is a shell call's script as last read: its files' key, what
// its trace said, and when that was.
type scriptSeen struct {
	read    atomic.Pointer[scriptRead] // nil until first read
	reading atomic.Bool                // a read is under way, off the UI
	at      time.Time
	live    bool // the call still runs: only then can it be stopped
	// view is scriptView's last answer, for the read, status and end it
	// was worked out from: a frame asks for every script's.
	view   *convo.ScriptView
	viewOf viewOf
}

type viewOf struct {
	read   *scriptRead
	status convo.Status
	end    time.Time
}

// scriptRead is what a call's script files said when last read.
type scriptRead struct {
	key string // "" when the call didn't run as a script
	run script.Run
}

// got is what s last read: the zero read until one is in.
func (s *scriptSeen) got() scriptRead {
	if r := s.read.Load(); r != nil {
		return *r
	}
	return scriptRead{}
}

// scriptEvery is how often a running script's trace is read again.
const scriptEvery = 300 * time.Millisecond

// scriptTurns is how many of the latest turns' calls are looked for as
// scripts: further back, they're folded anyway.
const scriptTurns = 3

// scriptViews are the shell calls of the latest turns rush ran as scripts,
// by tool call ID, as the conversation draws them.
func (c *hostConn) scriptViews(now time.Time) map[string]*convo.ScriptView {
	if c.sess == nil {
		return nil
	}
	var out map[string]*convo.ScriptView
	turns := c.sess.Turns
	for _, t := range turns[max(0, len(turns)-scriptTurns):] {
		for _, it := range t.Items {
			if it.Kind != convo.KStep || it.Step == nil || it.Step.Tool != "Bash" || it.Step.ID == "" {
				continue
			}
			st := it.Step
			s := c.script(st, now)
			r := s.read.Load()
			if r == nil || r.key == "" {
				continue
			}
			if of := (viewOf{r, st.Status, st.End}); s.view == nil || s.viewOf != of {
				s.view, s.viewOf = scriptView(r.run, st), of
			}
			if out == nil {
				out = map[string]*convo.ScriptView{}
			}
			out[st.ID] = s.view
		}
	}
	return out
}

// script is call st's script as last read, read again off the UI when it
// may have moved on: the trace is a line per command run, and the disk
// mustn't hold a frame. What's read shows from the next frame.
func (c *hostConn) script(st *convo.Step, now time.Time) *scriptSeen {
	if c.scripts == nil {
		c.scripts = map[string]*scriptSeen{}
	}
	s := c.scripts[st.ID]
	running := st.Status == convo.Running
	if s != nil {
		s.live = running
	}
	if s != nil && (now.Sub(s.at) < scriptEvery || !running && !s.at.IsZero() && s.at.After(st.End)) {
		return s
	}
	if s == nil {
		s = &scriptSeen{live: running}
		c.scripts[st.ID] = s
	}
	if !s.reading.CompareAndSwap(false, true) {
		return s
	}
	s.at = now
	id, cmd := st.ID, strings.TrimSpace(st.Call().Input.Command)
	go func() {
		defer s.reading.Store(false)
		if !script.Long(cmd) {
			return
		}
		for _, k := range []string{script.Key(id, cmd), script.Key("", cmd)} {
			if r, ok := script.Read(k); ok && strings.TrimSpace(r.Source) == cmd {
				s.read.Store(&scriptRead{k, r})
				return
			}
		}
	}()
	return s
}

// scriptView is what the conversation shows of run r of call st.
func scriptView(r script.Run, st *convo.Step) *convo.ScriptView {
	v := &convo.ScriptView{Took: map[int]time.Duration{}, Breaks: map[int]bool{}}
	running := st.Status == convo.Running
	if running { // a finished script's breakpoints stop nothing
		for _, n := range r.Breaks {
			v.Breaks[n] = true
		}
	}
	for i, h := range r.Hits {
		end := st.End
		if i+1 < len(r.Hits) {
			end = r.Hits[i+1].At
		} else if running || end.IsZero() {
			v.At, v.Since = h.Line, h.At
			continue
		}
		if end.After(h.At) {
			v.Took[h.Line] += end.Sub(h.At)
		}
	}
	if n := len(r.Hits); !running && n > 0 && st.Status == convo.Failed {
		v.FailedAt = r.Hits[n-1].Line
	}
	v.Held = running && r.HeldAt != 0
	if v.Held {
		v.At = r.HeldAt
	}
	return v
}

// toggleScriptBreak sets a breakpoint at the line of a script ref, or
// takes it away.
func (m *Model) toggleScriptBreak(c *hostConn, ref string) bool {
	stepRef, n, ok := convo.ScriptLine(ref)
	if !ok {
		return false
	}
	_, id, _ := strings.Cut(stepRef, ":s:")
	s := c.scripts[id]
	if s == nil || s.got().key == "" {
		return false
	}
	got := s.got()
	c.scrollOnly = false // drawn again, with its dot
	if !s.live {
		m.flash("the script has finished · a breakpoint only stops one still running", true)
		return true
	}
	if at := scriptLine(got.run); at >= n && !slices.Contains(got.run.Breaks, n) {
		m.flash("line "+strconv.Itoa(n)+" has already run · pick a line below "+strconv.Itoa(at), true)
		return true
	}
	on, err := script.Toggle(got.key, n)
	switch {
	case err != nil:
		m.flash("couldn't set the breakpoint: "+err.Error(), true)
	case on:
		m.flash("breakpoint at line "+strconv.Itoa(n)+" · the script stops when it gets there", false)
	default:
		m.flash("breakpoint at line "+strconv.Itoa(n)+" taken away", false)
	}
	s.at = time.Time{} // read again
	return true
}

// scriptLine is the line run r is on, 0 before it begins.
func scriptLine(r script.Run) int {
	if len(r.Hits) == 0 {
		return 0
	}
	return r.Hits[len(r.Hits)-1].Line
}

// goOnFromBreak lets a script stopped at a breakpoint go on: the shell
// stopped itself, so it's started again.
func (m *Model) goOnFromBreak(c *hostConn, id string) (tea.Cmd, bool) {
	s := c.scripts[id]
	if s == nil {
		return nil, false
	}
	r := s.got().run
	if r.HeldAt == 0 || r.PID == 0 {
		return nil, false
	}
	pid, line := r.PID, r.HeldAt
	s.at = time.Time{}
	m.flash("going on from line "+strconv.Itoa(line), false)
	return hostCmd(func() error { return syscall.Kill(pid, syscall.SIGCONT) }), true
}
