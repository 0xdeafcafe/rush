package ui

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	photonframe "github.com/0xdeafcafe/photon/frame"
)

// TestWheelPastEndDrawsNothing: a trackpad's momentum turning the wheel at
// either end of the Session keeps the last frame rather than drawing anew.
func TestWheelPastEndDrawsNothing(t *testing.T) {
	m, _ := benchModel(250, 70)
	m.host.sess, m.host.bodyBuf = benchConvo(30), nil
	m.View()
	wheel := func(b tea.MouseButton) bool {
		m.gate.DrawnAt(time.Time{}) // each turn its own burst
		m.Update(tea.MouseWheelMsg{X: 245, Y: 35, Button: b})
		same, _ := m.gate.Held()
		m.View()
		return same
	}
	if !wheel(tea.MouseWheelDown) {
		t.Fatal("wheel down at the latest drew a frame")
	}
	if wheel(tea.MouseWheelUp) {
		t.Fatal("wheel up from the latest kept the old frame")
	}
	for i := 0; i < 10000 && m.host.scroll < m.host.scrollCap; i++ {
		wheel(tea.MouseWheelUp)
	}
	if m.host.scrollCap == 0 {
		t.Fatal("the session fits the pane: nothing to scroll")
	}
	if !wheel(tea.MouseWheelUp) {
		t.Fatal("wheel up at the top drew a frame")
	}
}

// TestWheelBurstDrawsOnce: turns that come faster than a frame keep the
// last frame, and one frame after the burst draws where they got to.
func TestWheelBurstDrawsOnce(t *testing.T) {
	m, _ := benchModel(250, 70)
	m.host.sess, m.host.bodyBuf = benchConvo(30), nil
	m.View()
	m.gate.DrawnAt(time.Now())
	_, cmd := m.Update(tea.MouseWheelMsg{X: 245, Y: 35, Button: tea.MouseWheelUp})
	if same, pending := m.gate.Held(); !same || !pending || cmd == nil {
		t.Fatal("a turn just after a frame should wait for the burst's frame")
	}
	m.View()
	m.Update(tea.MouseWheelMsg{X: 245, Y: 35, Button: tea.MouseWheelUp})
	if same, _ := m.gate.Held(); !same {
		t.Fatal("a second turn in the burst drew a frame")
	}
	m.View()
	m.Update(photonframe.TickMsg{})
	if same, pending := m.gate.Held(); same || pending {
		t.Fatal("the burst's frame should draw")
	}
	if m.host.scroll != 6 {
		t.Fatalf("both turns should have scrolled: %d", m.host.scroll)
	}
}
