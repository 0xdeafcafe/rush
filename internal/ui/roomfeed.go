package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/room"
)

// roomFeed is the room the pane shows: its log, read off the UI goroutine
// and folded into the pane's session, so a room is drawn as any agent's
// conversation is. See docs/room.md.
type roomFeed struct {
	r       room.Room
	off     int64
	running bool
	paused  bool
	fold    roomFold
}

// syncRoom opens room row a in the pane, as syncHost opens an agent.
func (m *Model) syncRoom(a *fleet.Agent) tea.Cmd {
	if c := m.host; c != nil && c.key == a.Key || m.hostOpening == a.Key {
		return nil
	}
	s, ok := m.roomOf(a)
	if !ok {
		return nil
	}
	m.dropHost()
	m.hostOpening = a.Key
	f := &roomFeed{r: s.Room}
	m.roomFeed = f
	key, o := a.Key, m.warmOpts()
	type opened struct {
		c       *hostConn
		fold    roomFold
		off     int64
		running bool
	}
	return sheetDo(func() (opened, error) {
		es, off, err := room.Read(f.r.ID, 0)
		sess, fold := convo.New(), newRoomFold(f.r)
		for _, e := range es {
			fold.apply(sess, e)
		}
		running := room.Running(f.r.ID)
		if !running {
			fold.end(sess, time.Now())
		}
		if o.Width > 0 {
			o.Now = time.Now()
			sess.Tail(o, warmRows) // drawn once here, as warmed does
		}
		return opened{&hostConn{key: key, sess: sess, open: map[string]bool{}, ready: true}, fold, off, running}, err
	}, func(m *Model, op opened, err error) tea.Cmd {
		if m.roomFeed != f || m.hostOpening != key {
			return nil
		}
		if err != nil {
			return m.onHostOpen(hostOpenMsg{key: key, err: err})
		}
		f.fold, f.off, f.running, f.paused = op.fold, op.off, op.running, op.fold.paused
		return tea.Batch(m.onHostOpen(hostOpenMsg{key: key, c: op.c}), f.poll(key))
	})
}

// poll folds what's new in the room's log into the pane, and comes back
// for more while the room runs and the pane still shows it.
func (f *roomFeed) poll(key string) tea.Cmd {
	id, off := f.r.ID, f.off
	type read struct {
		es      []room.Entry
		off     int64
		running bool
	}
	return sheetDo(func() (read, error) {
		time.Sleep(300 * time.Millisecond)
		es, next, err := room.Read(id, off)
		return read{es, next, room.Running(id)}, err
	}, func(m *Model, r read, err error) tea.Cmd {
		c := m.host
		if m.roomFeed != f || c == nil || c.key != key {
			return nil // the pane moved on
		}
		for _, e := range r.es {
			f.fold.apply(c.sess, e)
		}
		f.off, f.running, f.paused = r.off, r.running, f.fold.paused
		if !r.running {
			f.fold.end(c.sess, time.Now())
			if len(r.es) == 0 && err == nil {
				return nil // over, and all of it read
			}
		}
		return f.poll(key)
	})
}

// sendRoom says what's in the box in the room: a message, @name, or
// /pause, /resume, /verdict, /stop.
func (m *Model) sendRoom(c *hostConn) tea.Cmd {
	f := m.roomFeed
	text := c.pastes.out(c.input, true)
	if f == nil || c.key != roomKeyPrefix+f.r.ID || text == "" {
		return nil
	}
	if !f.running {
		m.flash("this room is over · #room new opens another", false)
		return nil
	}
	c.input, c.back, c.pastes, c.scroll = c.input[:0], 0, pastes{}, 0
	id, e := f.r.ID, room.Parse(text, f.r.Members)
	return cmdErr("", func() error { return room.Post(id, e) })
}

// roomPaneKey is a room pane's own keys: ctrl+x pauses the room, as it
// stops an agent's turn, and alt+1…9 opens a member's own session.
func (m *Model) roomPaneKey(c *hostConn, s string) (tea.Cmd, bool) {
	f := m.roomFeed
	if f == nil || c.key != roomKeyPrefix+f.r.ID {
		return nil, false
	}
	if s == "ctrl+x" {
		if f.paused || !f.running {
			return nil, true
		}
		return cmdErr("paused · /resume carries on", func() error {
			return room.Post(f.r.ID, room.Entry{From: room.User, Kind: room.Control, Text: "pause"})
		}), true
	}
	if len(s) != 5 || !strings.HasPrefix(s, "alt+") || s[4] < '1' || s[4] > '9' {
		return nil, false
	}
	i := int(s[4] - '1')
	if i >= len(f.r.Members) {
		return nil, true
	}
	name := f.r.Members[i].Name
	for _, a := range m.rooms.members[f.r.ID] {
		if strings.HasPrefix(a.DisplayName, "Room · "+name+" ·") {
			m.sel = a.Key // syncHost opens it in this pane
			return nil, true
		}
	}
	m.flash(name+"'s session isn't in the list", false)
	return nil, true
}

// roomFold turns a room's log into the events a session draws: you as
// your messages, each agent's words and tool calls as a turn of its own
// headed by its name, the verdict as the last turn.
type roomFold struct {
	r       room.Room
	speaker string
	calls   map[string]bool
	uses    map[string]string // a member: the model its session says it runs
	rounds  map[string]int    // a member: the round of its turn under way
	paused  bool
}

func newRoomFold(r room.Room) roomFold {
	return roomFold{r: r, calls: map[string]bool{}, uses: map[string]string{}, rounds: map[string]int{}}
}

// end closes the turn being drawn.
func (f *roomFold) end(s *convo.Session, now time.Time) {
	if s.Live() != nil {
		s.Apply(event.TurnEnd{Reason: "done"}, now)
	}
	f.speaker = ""
}

func (f *roomFold) apply(s *convo.Session, e room.Entry) {
	now := e.At
	say := func(text string) {
		s.Apply(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: text}}}, now)
	}
	// open starts a turn that isn't yours: from, saying what about.
	open := func(from, about string) {
		f.end(s, now)
		s.Apply(host.Sent{Text: about}, now)
		s.Turns[len(s.Turns)-1].From = from
	}
	speak := func() { // an agent's words: in a turn of its own, under its name
		if f.speaker != e.From {
			open(e.From, f.header(e))
			f.speaker = e.From
		}
	}
	if e.From == room.User {
		text := e.Text
		if e.Kind == room.Control {
			text = "/" + e.Text
			f.paused = e.Text == "pause"
		}
		f.end(s, now)
		s.Apply(host.Sent{Text: text}, now)
		return
	}
	switch e.Kind {
	case room.Uses:
		f.uses[e.From] = e.Text
	case room.Turn:
		f.rounds[e.From] = e.Round
	case room.Note:
		speak()
		say(e.Text)
	case room.Tool:
		speak()
		c := roomCall(e)
		if f.calls[e.ID] {
			s.Apply(event.CallUpdated{Call: c}, now)
			return
		}
		f.calls[e.ID] = true
		s.Apply(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.ToolCall, Call: &c}}}, now)
	case room.Done:
		s.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: e.ID, IsError: e.Failed}}}}, now)
	case room.Say:
		speak()
		switch e.Ready {
		case "yes":
			e.Text += "\n\n✓ ready to sign"
		case "no":
			e.Text += "\n\n… not settled"
		}
		say(e.Text)
		f.end(s, now)
	case room.Paused:
		speak()
		say("⏸ " + e.Text)
		f.end(s, now)
	case room.Verdict:
		open("Verdict", strings.TrimSuffix("each agent's final position · "+e.Detail, " · "))
		var b strings.Builder
		for _, line := range strings.Split(e.Text, "\n") {
			name, pos, _ := strings.Cut(line, ": ")
			b.WriteString("- **" + name + ":** " + pos + "\n")
		}
		say(b.String())
		f.end(s, now)
		f.paused = false
	case room.Info, room.End:
		open("room", e.Text)
		f.end(s, now)
		if e.Kind == room.End {
			f.paused = false
		}
	}
}

// header is what a member's turn is headed with, beside its name: its
// agent and the round.
func (f *roomFold) header(e room.Entry) string {
	var h []string
	for _, mb := range f.r.Members {
		if mb.Name == e.From {
			if md := f.uses[mb.Name]; md != "" {
				mb.Config.Model = md
			}
			h = append(h, mb.Label())
		}
	}
	if n := f.rounds[e.From]; n > 0 {
		h = append(h, fmt.Sprintf("round %d", n))
	}
	return strings.Join(h, " · ")
}

// roomCall is the call a tool entry is: as made, or rebuilt from its words
// for a log written before entries kept it.
func roomCall(e room.Entry) tool.Call {
	if e.Call != nil {
		return *e.Call
	}
	return tool.Call{ID: e.ID, Name: e.Text, Title: e.Text, Input: tool.Input{Command: e.Detail}}
}
