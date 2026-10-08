package ui

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/room"
)

// A room is one row in the list, its members' sessions grouped under it;
// Enter or a click on either opens the room. See docs/room.md.

const roomKeyPrefix = "room:"

// roomRows is what the list knows of rooms: read off the UI goroutine.
type roomRows struct {
	list    []room.Summary
	lister  room.Lister
	loading bool
	rows    map[string]*fleet.Agent // each room's row, kept so selection holds
	members map[string][]*fleet.Agent
	of      map[string]string         // a member's agent key: its room's id
	joined  map[string][]*fleet.Agent // a chat's agent key: the rows of rooms opened from it
}

// loadRooms reads the rooms again, once a reading of the fleet lands.
func (m *Model) loadRooms() tea.Cmd {
	rr := &m.rooms
	if rr.loading {
		return nil
	}
	rr.loading = true
	return sheetDo(func() ([]room.Summary, error) { return rr.lister.List(), nil }, func(m *Model, list []room.Summary, _ error) tea.Cmd {
		rr.loading, rr.list = false, list
		m.verdictsBack(list)
		m.rebuild()
		return nil
	})
}

// roomize is the agents the list lays out: each room as one row in place
// of its members, which roomMembers gives back to go under it.
func (m *Model) roomize(agents []*fleet.Agent) []*fleet.Agent {
	rr := &m.rooms
	if len(rr.list) == 0 {
		return agents
	}
	byID := map[string]*room.Summary{}
	for i := range rr.list {
		for sid := range rr.list[i].Sessions {
			byID[sid] = &rr.list[i]
		}
	}
	if rr.rows == nil {
		rr.rows = map[string]*fleet.Agent{}
	}
	clear(rr.of)
	if rr.of == nil {
		rr.of = map[string]string{}
	}
	clear(rr.members)
	if rr.members == nil {
		rr.members = map[string][]*fleet.Agent{}
	}
	clear(rr.joined)
	if rr.joined == nil {
		rr.joined = map[string][]*fleet.Agent{}
	}
	out := make([]*fleet.Agent, 0, len(agents))
	chats := map[string]bool{}
	for _, a := range agents {
		if s := byID[a.ID]; a.Rush && s != nil {
			rr.of[a.Key] = s.ID
			rr.members[s.ID] = append(rr.members[s.ID], a)
			continue
		}
		out = append(out, a)
		chats[a.Key] = true
	}
	for i := range rr.list {
		s := &rr.list[i]
		key := roomKeyPrefix + s.ID
		if _, hidden := m.store.Overlay.Hidden[key]; hidden || !s.Running && len(rr.members[s.ID]) == 0 {
			continue
		}
		a := rr.rows[s.ID]
		if a == nil {
			a = &fleet.Agent{Key: key}
			rr.rows[s.ID] = a
		}
		a.DisplayName, a.Name, a.Cwd, a.Kind = s.Topic, s.Topic, s.Cwd, "room"
		a.CreatedAt, a.UpdatedAt = s.Created, s.Updated
		a.State, a.Done = "done", false
		if s.Running {
			a.State = "working"
		}
		a.Spend = fleet.Spend{}
		for _, mb := range rr.members[s.ID] {
			a.Spend.Cost += mb.Spend.Cost
			a.Spend.Today += mb.Spend.Today
		}
		if chats[s.From] {
			rr.joined[s.From] = append(rr.joined[s.From], a) // under its chat, not a row of its own
			continue
		}
		out = append(out, a)
	}
	return out
}

// roomMembers are the rows that go under row a: a room's members, or a
// chat's rooms, each with its members.
func (m *Model) roomMembers(a *fleet.Agent) []*fleet.Agent {
	id, ok := strings.CutPrefix(a.Key, roomKeyPrefix)
	if ok {
		return m.rooms.members[id]
	}
	var out []*fleet.Agent
	for _, r := range m.rooms.joined[a.Key] {
		out = append(append(out, r), m.roomMembers(r)...)
	}
	return out
}

// roomOf is the room a row is, or a member of.
func (m *Model) roomOf(a *fleet.Agent) (*room.Summary, bool) {
	if a == nil {
		return nil, false
	}
	id, ok := strings.CutPrefix(a.Key, roomKeyPrefix)
	if !ok {
		id, ok = m.rooms.of[a.Key]
	}
	if !ok {
		return nil, false
	}
	for i := range m.rooms.list {
		if m.rooms.list[i].ID == id {
			return &m.rooms.list[i], true
		}
	}
	return nil, false
}

// openRoomFor opens the room a is, or is a member of, in the pane, as
// opening an agent does; false when a has nothing to do with a room.
func (m *Model) openRoomFor(a *fleet.Agent) (tea.Cmd, bool) {
	s, ok := m.roomOf(a)
	if !ok {
		return nil, false
	}
	m.sel = roomKeyPrefix + s.ID
	m.preview, m.paneFocus = true, true
	return nil, true
}

// roomStatus is a room in a few words: who's speaking, paused, over.
func roomStatus(s *room.Summary) (marker, text string) {
	n := fmt.Sprintf(" · %d agents", len(s.Members))
	switch {
	case s.Over != "" || !s.Running:
		if s.Verdict {
			return paint(cGreen, "✓"), "verdict in" + n
		}
		return faint("·"), "over" + n
	case s.Paused:
		return paint(cYellow, "⏸"), "paused" + n
	case len(s.Speaking) == 1:
		return "", s.Speaking[0] + " speaking" + n
	case len(s.Speaking) > 1:
		return "", "openings" + n
	}
	return "", "between turns" + n
}

// roomLine is a room's row: its icon, topic and how it stands.
func (m *Model) roomLine(a *fleet.Agent, w int, sel bool) string {
	s, _ := m.roomOf(a)
	if s == nil {
		return ""
	}
	marker, status := roomStatus(s)
	if marker == "" {
		marker = paint(cOrange, convo.Spin("", m.tick+len(s.ID)))
	}
	right := "  " + dim(status)
	name := paint(cText, "◈ "+s.Topic)
	if sel {
		name = paint(cBright+bold, "◈ "+s.Topic)
	}
	space := max(4, w-3-cellw.String(right))
	return " " + marker + " " + fit(name, space) + right
}

// roomMemberLine is a member's row under its room: its name, its agent and
// what it's doing.
func (m *Model) roomMemberLine(a *fleet.Agent, w int, sel bool) string {
	s, _ := m.roomOf(a)
	name, label := a.DisplayName, ""
	if s != nil {
		for _, mb := range s.Members {
			if strings.HasPrefix(a.DisplayName, "Room · "+mb.Name+" ·") {
				name, label = mb.Name, mb.Label()
			}
		}
	}
	marker := faint("·")
	switch {
	case a.Live():
		marker = paint(cOrange, convo.Spin(a.Kind, m.tick+len(a.ID)))
	case a.PID != 0:
		marker = dim("◦")
	}
	doing := a.Detail
	style := cSub
	if sel {
		style = cBright + bold
	}
	return "   " + marker + " " + fit(paint(style, "└ "+name)+"  "+faint(label)+"  "+dim(doing), max(4, w-6))
}

// askHideRoom is ctrl+x on a room's row: y hides the room and its members'
// sessions, stopping it first when it's still running; its log stays.
func (m *Model) askHideRoom(a *fleet.Agent) tea.Cmd {
	s, ok := m.roomOf(a)
	if !ok {
		return nil
	}
	id, running, members := s.ID, s.Running, m.roomMembers(a)
	m.confirm = &confirmation{question: "Hide room " + s.Topic + "?", yesText: "hide",
		detail: "y hides the room and its agents from the list for good, stopping it · its log kept on disk",
		onYes: func() tea.Cmd {
			next := ""
			if m.sel == a.Key {
				next = m.neighbour(a.Key)
			}
			m.store.Hide(a.Key)
			for _, mb := range members {
				m.store.Hide(mb.Key)
			}
			_ = m.store.SaveOverlay()
			m.refresh()
			if next != "" {
				m.sel = next
			}
			m.flash("hidden: room "+s.Topic, false)
			if !running {
				return nil
			}
			return cmdErr("", func() error { return room.Post(id, room.Entry{From: room.User, Kind: room.Control, Text: "stop"}) })
		}}
	return nil
}

// isRoomRow is whether a is a room's own row, not an agent.
func isRoomRow(a *fleet.Agent) bool { return isRoomKey(a.Key) }

func isRoomKey(key string) bool { return strings.HasPrefix(key, roomKeyPrefix) }

// roomConn is what a room's pane header says where an agent's says how
// it's connected: how the room stands, and the way to each member.
func (m *Model) roomConn(a *fleet.Agent) string {
	s, ok := m.roomOf(a)
	if !ok {
		return ""
	}
	marker, status := roomStatus(s)
	if s.Paused && s.Running && s.Over == "" {
		status = paint(cYellow+bold, "PAUSED") + dim(" · /resume carries on")
	} else {
		status = dim(status)
	}
	return strings.TrimSpace(marker+" "+status) + dim(fmt.Sprintf(" · alt+1…%d one's own session", len(s.Members)))
}
