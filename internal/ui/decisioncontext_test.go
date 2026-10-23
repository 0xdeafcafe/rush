package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/charmbracelet/x/ansi"
)

// A decision's card says what's at stake before the decision itself.
func TestDecisionCardShowsContext(t *testing.T) {
	c := &hostConn{qDecide: true, qPicks: map[int]map[int]bool{}}
	qs := []question{{Question: "Sessions that woke show no history.\n\n1. What to make easier to find", Header: "What to make easier to find",
		Options: []event.Choice{{Label: "history", Description: "look where the session started"}, {Label: "names"}}}}
	text := ansi.Strip(strings.Join(drawQuestion(c, "", qs, 100, 0, 0), "\n"))
	at, ask := strings.Index(text, "Sessions that woke show no history."), strings.Index(text, "1. What to make easier to find")
	if at < 0 || ask < 0 || at > ask {
		t.Fatalf("context then decision, got:\n%s", text)
	}
	if !strings.Contains(text, "look where the session started") {
		t.Fatalf("no option detail in:\n%s", text)
	}
}
