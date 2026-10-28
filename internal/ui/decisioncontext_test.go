package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/charmbracelet/x/ansi"
)

// A decision's card is titled by the decision, then says what's at stake;
// the title isn't said again under it.
func TestDecisionCardShowsContext(t *testing.T) {
	c := &hostConn{qDecide: true, qPicks: map[int]map[int]bool{}}
	qs := []question{{Question: "Sessions that woke show no history.\n\n1. What to make easier to find", Header: "What to make easier to find",
		Options: []event.Choice{{Label: "history", Description: "look where the session started"}, {Label: "names"}}}}
	text := ansi.Strip(strings.Join(drawQuestion(c, "", qs, 100, 0, 0), "\n"))
	title, at := strings.Index(text, "? What to make easier to find"), strings.Index(text, "Sessions that woke show no history.")
	if title < 0 || at < 0 || title > at || strings.Contains(text, "1. What to make easier to find") {
		t.Fatalf("title then context, got:\n%s", text)
	}
	if !strings.Contains(text, "look where the session started") {
		t.Fatalf("no option detail in:\n%s", text)
	}
}
