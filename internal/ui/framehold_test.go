package ui

import (
	"testing"

	tea "charm.land/bubbletea/v2"
	"time"
)

// Background work landing just after a frame keeps it, asks for one frame
// when the gap is up, and what you do draws at once.
func TestBackgroundHeld(t *testing.T) {
	m, _ := benchModel(200, 50)
	m.View() // drewAt: now
	_, cmd := m.Update(sheetMsg{apply: func(*Model) tea.Cmd { return nil }})
	if same, _ := m.gate.Held(); !same || cmd == nil || !m.holdPending {
		t.Fatalf("background just after a frame: kept %v, asked %v", same, cmd != nil)
	}
	m.View()
	m.Update(sheetMsg{apply: func(*Model) tea.Cmd { return nil }})
	if same, _ := m.gate.Held(); !same {
		t.Fatal("a second landing drew")
	}
	m.View()
	m.Update(heldFrameMsg{})
	if same, _ := m.gate.Held(); same || m.holdPending {
		t.Fatal("the asked-for frame doesn't draw")
	}
	// Drawn since by another frame, the asked-for one keeps it.
	m.View()
	m.drewAt = time.Time{}
	m.Update(sheetMsg{apply: func(*Model) tea.Cmd { return nil }}) // not held: long since a frame
	m.heldAt, m.drewAt = time.Now().Add(-time.Second), time.Now()
	m.Update(heldFrameMsg{})
	if same, _ := m.gate.Held(); !same {
		t.Fatal("drew again what a frame since had drawn")
	}
	for _, msg := range []tea.Msg{tea.KeyPressMsg{}, tea.MouseClickMsg{}, tea.WindowSizeMsg{}, tickMsg{}} {
		if held(msg) {
			t.Errorf("%T is held", msg)
		}
	}
}
