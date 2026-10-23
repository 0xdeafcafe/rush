package ui

import (
	"strings"
	"testing"
)

// Hovering a row keeps a lit keycap's ground, so its dark number shows.
func TestHoverKeepsLitKeycap(t *testing.T) {
	c := &hostConn{bodyTop: 0, paneW: 60, rowRefs: []string{"q"}}
	c.pointerHover = paneHover{row: 1, ref: "q"}
	rows := []string{onBg(qSel, " "+paint(cOrange, "▌")+" "+keycap("3", true)+" "+paint(cText+bold, "skip it"), 60)}
	c.paintHover(rows)
	if !strings.Contains(rows[0], qCapOn+qInk) {
		t.Fatalf("the lit keycap lost its ground: %q", rows[0])
	}
	if strings.Contains(rows[0], qSel) {
		t.Fatalf("the row kept its own ground under the hover: %q", rows[0])
	}
}
