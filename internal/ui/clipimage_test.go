package ui

import (
	"strings"
	"testing"
	"time"

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
