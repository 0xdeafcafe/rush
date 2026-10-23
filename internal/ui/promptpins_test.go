package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// Scrolled into a long conversation, the prompt you're reading under is
// pinned at the top once it's scrolled off it, and clicking a pin goes to
// its turn. At the end, with the latest on screen, nothing's pinned.
func TestPromptPins(t *testing.T) {
	m, _ := benchModel(200, 50)
	c := m.host
	c.sess, c.bodyBuf, c.historyMode = benchConvo(400), nil, convo.HistoryOpen
	pinned := func() (rows []string) {
		for i, r := range m.rushPane(120, 50) {
			if i < len(c.rowRefs) && strings.HasPrefix(c.rowRefs[i], pinRefPrefix) {
				rows = append(rows, ansi.Strip(r))
			}
		}
		return rows
	}
	if got := pinned(); len(got) != 0 {
		t.Fatalf("at the end, nothing: %q", got)
	}
	c.scroll, c.scrollOnly = 2000, true
	if got := pinned(); len(got) != 1 || !strings.Contains(got[0], "current") {
		t.Fatalf("mid-way, the current one: %q", got)
	}
	m.clickRef(c, pinRefPrefix+c.sess.TurnRef(0))
	if c.sel != c.sess.TurnRef(0) || !c.selMoved {
		t.Fatalf("a pin goes to its turn: sel %q", c.sel)
	}
	for i, r := range m.rushPane(120, 50) {
		if i < len(c.rowRefs) && !strings.HasPrefix(c.rowRefs[i], pinRefPrefix) && strings.Contains(ansi.Strip(r), "turn 1:") {
			return
		}
	}
	t.Fatal("after the click, the first prompt is in view")
}

func TestPromptPinsNoneAtTop(t *testing.T) {
	m, _ := benchModel(200, 50)
	c := m.host
	c.sess, c.bodyBuf, c.historyMode = benchConvo(400), nil, convo.HistoryOpen
	c.scroll, c.scrollOnly = 1<<20, true // as far up as it goes
	for i := range m.rushPane(120, 50) {
		if i < len(c.rowRefs) && strings.HasPrefix(c.rowRefs[i], pinRefPrefix) {
			t.Fatalf("row %d pinned at the very top", i)
		}
	}
}
