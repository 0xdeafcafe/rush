package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/convo"
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

// Hovering a step keeps its thumbnail's pixels: they're drawn as grounds.
func TestHoverKeepsThumbnail(t *testing.T) {
	px := "\x1b[38;2;200;10;10m\x1b[48;2;10;200;10m▀\x1b[49m " + reset
	c := &hostConn{bodyTop: 0, paneW: 20, rowRefs: []string{"s", ""}, rowOwners: []string{"", "s"}, rowBody: []int{0, 1}}
	c.shown = []convo.Line{{Text: "step", Ref: "s"}, {Text: px, Owner: "s", Picture: true}}
	c.pointerHover = paneHover{row: 1, ref: "s"}
	rows := []string{"step", px}
	c.paintHover(rows)
	if !strings.Contains(rows[1], "\x1b[48;2;10;200;10m▀") {
		t.Fatalf("the thumbnail lost its pixels under the hover: %q", rows[1])
	}
	if strings.Contains(rows[1], "\x1b[49m") {
		t.Fatalf("a clear pixel kept the terminal's ground, not the hover's: %q", rows[1])
	}
}
