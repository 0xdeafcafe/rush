package ui

import (
	"testing"

	tea "charm.land/bubbletea/v2"
)

// A picked row scrolled out of sight lets go of ↓ and typing: they go to
// the box. One on screen keeps them.
func TestPickOutOfSightLetsGo(t *testing.T) {
	m, c := imageBox(t)
	c.sel, c.rowRefs = "t1", []string{"t7", "t8"}
	m.paneKey(tea.KeyPressMsg{Code: tea.KeyDown}, "down")
	if c.sel != "" {
		t.Fatalf("↓ kept a pick off screen: %q", c.sel)
	}
	c.sel = "t1"
	m.paneKey(key("v"), "v")
	if c.sel != "" || string(c.input) != "v" {
		t.Fatalf("typing with a pick off screen: sel %q, box %q", c.sel, string(c.input))
	}
	c.input, c.sel, c.rowRefs = c.input[:0], "t8", []string{"t7", "t8"}
	if !c.selShown() {
		t.Fatal("a pick on screen isn't seen as shown")
	}
}

// Back from another window, the pointer crossing the list on its way in
// doesn't take the keys from the Session; crossing it after does.
func TestPointerBackKeepsTheKeys(t *testing.T) {
	m, c := imageBox(t)
	_ = c
	m.listW, m.ptrX, m.ptrSeen = 40, 80, true
	m.update(tea.BlurMsg{})
	m.update(tea.FocusMsg{})
	m.update(tea.MouseMotionMsg{X: 10, Y: 5})
	if !m.paneFocus {
		t.Fatal("the first motion back over the list took the keys")
	}
	m.update(tea.MouseMotionMsg{X: 80, Y: 5})
	m.update(tea.MouseMotionMsg{X: 10, Y: 5})
	if m.paneFocus {
		t.Fatal("crossing onto the list after didn't take the keys")
	}
}
