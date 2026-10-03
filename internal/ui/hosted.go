package ui

import (
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// NewHosted is the view of one rush-mode session alone, for embedding in
// another app: rush's header, then its Session at the whole width, with no
// Agents list and no keys that lead to another agent. Efficiency, Machine
// and Settings open with ctrl+\, and Agents is this session. id is the
// session's rush id. Only ctrl+q quits: esc leaves the message box, then
// stops the turn. The session's host keeps running.
//
// Past conversations are left out until its Agents list or the command bar
// needs every agent: finding them reads every transcript, and one in a
// folder macOS guards (Desktop, say) would hold the view behind a privacy
// prompt the embedding app has to answer.
func NewHosted(store *state.Store, version, id string) *Model {
	m := newModel(store, version, true)
	m.hosted = id
	m.loader.Keep(id)
	m.onboard = false
	m.snap = m.hostedSnap(m.snap) // its reading lands as the view starts
	m.rebuild()
	m.pinHosted()
	return m
}

// hostedSnap is a snapshot of every agent as the view takes it; in hosted,
// unless its list is shown (ctrl+6), only the one session is in it, so
// nothing else is listed, counted or acted on.
func (m *Model) hostedSnap(snap *fleet.Snapshot) *fleet.Snapshot {
	if m.hosted == "" {
		return snap
	}
	m.fleetAgents = snap.Agents
	var mine *fleet.Agent
	for _, a := range snap.Agents {
		if a.Rush && a.ID == m.hosted {
			mine = a
			m.hostedKey = a.Key
			break
		}
	}
	if m.hostedList {
		return snap
	}
	s := *snap
	s.Agents = nil
	if mine != nil {
		s.Agents = append(s.Agents, mine)
	}
	return &s
}

// hostedAlone is hosted showing its one session, the list hidden.
func (m *Model) hostedAlone() bool { return m.hosted != "" && !m.hostedList }

// listToggleKey is the key that shows and hides Agents beside a Session:
// ctrl+6, which terminals send as ctrl+^ unless they speak the kitty
// keyboard protocol.
func listToggleKey(s string) bool { return s == "ctrl+6" || s == "ctrl+^" || s == "ctrl+shift+6" }

// everyAgent has the next reading take in past conversations, which
// hosted leaves out until something shows every agent.
func (m *Model) everyAgent() {
	if m.hosted != "" && m.loader != nil {
		m.loader.SkipPast(false)
		m.refresh()
	}
}

// allAgents is every agent, hosted's hidden ones included: what the
// command bar searches and goes to.
func (m *Model) allAgents() []*fleet.Agent {
	if m.hosted != "" && m.fleetAgents != nil {
		return m.fleetAgents
	}
	return m.snap.Agents
}

// anyAgent is an agent by key among allAgents.
func (m *Model) anyAgent(key string) *fleet.Agent {
	for _, a := range m.allAgents() {
		if a.Key == key {
			return a
		}
	}
	return nil
}

// hostedShow is hosted going to another agent than its own, from the
// command bar: the list comes beside it, as ctrl+6 shows it, and ctrl+6
// or esc from the list is the way back.
func (m *Model) hostedShow(key string) {
	if m.hosted == "" || m.hostedList || key == m.hostedKey {
		return
	}
	m.hostedList = true
	m.full, m.preview = false, true
	s := *m.snap
	s.Agents = m.fleetAgents
	m.snap = m.hostedSnap(&s)
	m.rebuild()
}

// toggleList shows or hides Agents beside the open Session. In hosted the
// list comes with every agent, to pick and answer, and the plugin
// group-by modes; hidden again, the view is back on hosted's own session
// with hosted's limits. Elsewhere it moves between the split and the
// Session alone, without changing the layout #view keeps.
func (m *Model) toggleList() tea.Cmd {
	if m.hosted != "" {
		m.hostedList, m.hostedAway = !m.hostedList, false
		if m.hostedList {
			m.everyAgent()
			m.full, m.preview, m.paneFocus = false, true, false
			m.flash("Agents beside the session · ctrl+6 or esc hides them", false)
		} else {
			m.bar, m.picker, m.inKind = nil, nil, inPrompt
		}
		// The full fleet is already cached (m.fleetAgents): narrowing or
		// widening the list is a filter of it, not a fresh disk read.
		s := *m.snap
		s.Agents = m.fleetAgents
		m.snap = m.hostedSnap(&s)
		m.pinHosted()
		m.rebuild()
		return m.loadPreview()
	}
	if m.selected() == nil {
		return nil
	}
	if m.listW > 0 && m.chatOpen() {
		m.full, m.paneFocus = true, true
		return m.loadPreview()
	}
	m.full, m.peekFrom = false, ""
	if !m.chatOpen() {
		m.preview = true
	}
	if l, _ := m.widths(); l == 0 {
		// No room for both, or the Session alone is your layout: the list
		// takes the screen.
		m.leaveChat()
	}
	m.paneFocus = false
	return m.loadPreview()
}

// pinHosted keeps the hosted view on its session, whatever a key or message
// did to the selection. Efficiency, Machine and Settings open as usual;
// Agents is the one session, unless its list is shown.
func (m *Model) pinHosted() {
	if m.hosted == "" {
		return
	}
	m.zen, m.peekFrom = false, ""
	if m.view != placeAgents || m.hostedList {
		return
	}
	if m.hostedKey != "" {
		m.sel, m.shown = m.hostedKey, m.hostedKey
	}
	m.preview, m.full = true, true
	if m.hostedAway && m.paneFocus {
		m.hostedAway = false // something gave the box the keys again
	}
	if !m.hostedAway {
		m.paneFocus = true
	}
}

// canInterrupt is whether c has a turn running that can be stopped, and
// wasn't just asked to stop.
func canInterrupt(c *hostConn) bool {
	return c != nil && c.client != nil && c.sess.Live() != nil && time.Since(c.stopArmed) > 2*time.Second && agent.Supports(sessionAgent(c), agent.FeatureInterrupt)
}

// stopTurn is esc on a running turn: it stops at once, and esc again
// asks whether to close, restart or switch it.
func (m *Model) stopTurn(c *hostConn) tea.Cmd {
	c.stopArmed = time.Now()
	m.escAt, m.escKey = time.Now(), c.key
	m.flash("stopping the turn · esc again to close, restart or switch it", false)
	return hostCmd(func() error { return c.client.Interrupt() })
}

// hostedAwayKey is a key in hosted while the message box doesn't have the
// keys: esc stops the turn if one is running and does nothing else, enter
// or → give the box the keys back, and any other key goes back to the box
// and on to it.
func (m *Model) hostedAwayKey(s string) (tea.Cmd, bool) {
	switch s {
	case "esc":
		if canInterrupt(m.host) {
			return m.stopTurn(m.host), true
		}
		return nil, true
	case "enter", "right":
		m.paneFocus, m.hostedAway = true, false
		return nil, true
	}
	m.paneFocus, m.hostedAway = true, false
	return nil, false
}

// hostedKeyGuard drops the keys that would leave the one session in hosted:
// zen, the command bar, { } between list and Session, ctrl+n to the next
// agent, and , . < > between places, which stay text. ctrl+\ still goes
// to the next place, and ctrl+6 shows or hides Agents beside the session.
// It says whether it took the key.
func (m *Model) hostedKeyGuard(s string) (tea.Cmd, bool) {
	if m.hosted == "" {
		return nil, false
	}
	if m.hostedList {
		switch {
		case s == "ctrl+z":
			return nil, true
		case s == "esc" && !m.paneFocus && m.mode == modeList && m.dialog == nil && m.picker == nil:
			return m.toggleList(), true // esc from the list hides it
		}
		return nil, false
	}
	switch s {
	case "ctrl+q", "ctrl+\\":
		return nil, false
	case "ctrl+z", "ctrl+n":
		return nil, true
	}
	if m.placeStep(s) != 0 {
		return nil, true
	}
	if m.view != placeAgents {
		return nil, false // the place's own keys, tab through its pages included
	}
	if m.host == nil {
		// Still opening: nothing to type into yet.
		return nil, s != "ctrl+c"
	}
	if m.hostedAway && !m.paneFocus && m.picker == nil && m.confirm == nil && m.sheet == nil && m.dialog == nil {
		return m.hostedAwayKey(s)
	}
	if (s == "{" || s == "}") && len(m.host.input) == 0 {
		return nil, true
	}
	if s == "tab" {
		if c := m.host; c != nil {
			if cmd, used := m.slashKey(c, s); used {
				return cmd, true
			}
		}
		return nil, true
	}
	return nil, false
}
