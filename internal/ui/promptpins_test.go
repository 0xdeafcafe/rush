package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// Scrolled into a long conversation, the first prompt is pinned at the top;
// clicking it goes to it. The latest is pinned only once scrolled off the
// top: one below is reached going down, and at the very top nothing's
// pinned over the first prompt.
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
	if got := pinned(); len(got) != 1 || !strings.Contains(got[0], "first") || !strings.Contains(got[0], "turn 1:") {
		t.Fatalf("at the end, only the first: %q", got)
	}
	c.scroll, c.scrollOnly = 2000, true
	if got := pinned(); len(got) != 1 || !strings.Contains(got[0], "first") {
		t.Fatalf("mid-way, the first only, the latest still below: %q", got)
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
