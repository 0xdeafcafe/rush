package ui

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
)

type accountFrameProbe struct {
	rows   []acctRow
	used   []acctRow
	note   string
	reused bool
}

func (s *accountFrameProbe) body(m *Model, _, _ int) []string {
	s.rows, s.used, s.note = m.accountRows(), m.inUseRows(), m.lowNote("zcodex")
	again := m.accountRows()
	usedAgain := m.inUseRows()
	s.reused = m.drawing && len(s.rows) > 0 && len(s.used) > 0 && &s.rows[0] == &again[0] && &s.used[0] == &usedAgain[0]
	return []string{"account frame probe"}
}
func (*accountFrameProbe) key(*Model, tea.KeyPressMsg, string) tea.Cmd { return nil }

func TestAccountFrameCacheExpiresBeforeNextUpdate(t *testing.T) {
	m, _ := accountsModel(t)
	probe := &accountFrameProbe{}
	m.sheet = probe
	m.View()
	if !probe.reused || !strings.Contains(probe.note, "99%") {
		t.Fatalf("first frame not shared: reused=%v note=%q", probe.reused, probe.note)
	}
	if m.accountFrame.rowsOK || m.accountFrame.inUseOK || len(m.accountFrame.notes) != 0 {
		t.Fatal("cached account data survived View")
	}
	// A config/account update between frames must be visible everywhere in
	// the very next frame, without callers remembering to invalidate a cache.
	m.accts.now["zcodex"] = "a2"
	m.store.Config.SignIns[1].Name = "renamed account"
	m.View()
	if !probe.reused || probe.note != "" {
		t.Fatalf("stale quota after switch: %q", probe.note)
	}
	found := false
	for _, r := range probe.used {
		if r.kind == "zcodex" {
			found = r.name() == "renamed account" && r.q.Used("") == 20
		}
	}
	if !found {
		t.Fatal("next frame kept old account or configuration")
	}
	// Outside rendering, command/update handlers always receive fresh rows.
	m.store.Config.SignIns[1].Name = "changed outside View"
	found = false
	for _, r := range m.accountRows() {
		if r.current && r.name() == "changed outside View" {
			found = true
		}
	}
	if !found {
		t.Fatal("account rows cached outside a frame")
	}
}
