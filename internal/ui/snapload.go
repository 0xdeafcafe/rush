package ui

import (
	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/menubar"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/statusline"
)

// Reading the fleet walks the disk: jobs, sessions, transcripts, the
// process table, git, the plugins' files. None of it happens on the UI's
// goroutine, where it would hold up every key while a disk (or a folder on
// a network drive) is slow. refresh asks for a reading; one is made at a
// time, off the UI, and applied when it arrives. Asking while one is being
// made asks for another once it lands, so everything asked for is seen.

// snapMsg is a reading of the fleet made off the UI's goroutine.
type snapMsg struct {
	snap     *fleet.Snapshot
	sidebars []plugin.Sidebar
	jump     string // a notification or the menu bar's menu was clicked: the agent to show
}

// refresh asks for a new reading of the fleet; it lands as a snapMsg.
func (m *Model) refresh() { m.snapWanted = true }

// loadSnapCmd starts the reading refresh asked for, unless one is being
// made already. Update calls it after every message.
func (m *Model) loadSnapCmd() tea.Cmd {
	if !m.snapWanted || m.snapLoading {
		return nil
	}
	m.snapWanted, m.snapLoading = false, true
	l, store, files, hosted := m.loader, m.store.Copy(), &m.sidebarFiles, m.hosted
	return func() tea.Msg {
		msg := snapMsg{snap: l.LoadFrom(store, true)}
		if hosted == "" {
			msg.sidebars = files.Load()
			msg.jump = menubar.Goto()
		}
		return msg
	}
}

// onSnap applies a reading of the fleet.
func (m *Model) onSnap(msg snapMsg) tea.Cmd {
	m.snapLoading = false
	m.applySnap(msg.snap, msg.sidebars)
	if k := msg.jump; k != "" && m.agentByKey(k) != nil {
		m.sel = k
		m.rebuild()
	}
	if k := m.selectOnLoad; k != "" && m.agentByKey(k) != nil {
		m.selectOnLoad = ""
		m.sel = k
		m.rebuild()
	}
	return tea.Batch(m.watchShells(), m.loadRooms())
}

// refreshNow reads the fleet on the calling goroutine: for --render and
// --soak and tests, which have no loop to hand a reading back to.
func (m *Model) refreshNow() {
	m.snapWanted = false
	if m.bars.Top.Lines == nil && m.bars.Agent.Lines == nil {
		m.bars = statusline.LoadBars()
	}
	var sb []plugin.Sidebar
	if m.hosted == "" {
		sb = m.sidebarFiles.Load()
	}
	m.rooms.list = m.rooms.lister.List()
	m.applySnap(m.loader.Load(true), sb)
}

// applySnap takes in a reading: what changed is noticed, finished agents
// are put away, and the list is laid out again.
func (m *Model) applySnap(snap *fleet.Snapshot, sidebars []plugin.Sidebar) {
	m.fleetRead = true
	m.snap = m.hostedSnap(snap)
	m.notify()
	if m.leads() {
		m.hibernate()
		m.reap()
	}
	m.sidebars = sidebars
	m.rebuild()
}
