package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// While ctrl+v reads the clipboard the bottom line spins, after a flash
// rather than over it, and goes back once the image is in.
func TestPasteSpinner(t *testing.T) {
	m := &Model{w: 100, h: 20, snap: &fleet.Snapshot{At: time.Now()}}
	m.pastingImg = true
	if got := ansi.Strip(m.statusOr("hint")); !strings.Contains(got, "reading the clipboard") {
		t.Errorf("no spinner: %q", got)
	}
	m.flash("moved", false)
	if got := ansi.Strip(m.statusOr("hint")); !strings.Contains(got, "moved") || !strings.Contains(got, "reading the clipboard") {
		t.Errorf("the flash and the spinner should both show: %q", got)
	}
	m.update(clipImageMsg{})
	if got := ansi.Strip(m.statusOr("hint")); strings.Contains(got, "reading the clipboard") {
		t.Errorf("spinner outlived the read: %q", got)
	}
}

// cmd+v reads an image off the clipboard as ctrl+v does, in the Prompt and
// in a Session's box: a terminal that pastes only text lets it through
// when the clipboard holds just an image.
func TestCmdVPastesImage(t *testing.T) {
	for _, pane := range []bool{false, true} {
		for _, k := range []tea.KeyPressMsg{{Code: 'v', Mod: tea.ModCtrl}, {Code: 'v', Mod: tea.ModSuper}} {
			m, _ := benchModel(160, 40)
			m.paneFocus = pane
			if pane && m.host == nil {
				t.Fatal("bench model has no Session")
			}
			m.key(k)
			if !m.pastingImg {
				t.Errorf("%s with the Session focused=%v didn't read the clipboard", k.String(), pane)
			}
		}
	}
}
