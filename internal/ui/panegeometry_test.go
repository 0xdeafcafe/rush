package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
)

func TestPaneHeaderFrameEdges(t *testing.T) {
	for _, full := range []bool{false, true} {
		m, _ := benchModel(160, 44)
		m.full = full
		frame := m.listView()
		rows := strings.Split(frame, "\n")
		x := m.paneX()
		if full && x != 2 {
			t.Fatalf("full pane text moved to %d", x)
		}
		for row := 0; row < 4; row++ {
			line := rows[m.paneTop+row]
			if got := ansi.StringWidth(line); got != m.w {
				t.Fatalf("full=%v row=%d width=%d want=%d", full, row, got, m.w)
			}
			for _, col := range []int{x - 2, x - 1, m.w - 1} {
				want := bgChrome
				if col == m.w-1 {
					_, pw, _ := m.layout()
					cs := headerColours(pw-3, row, sessionAgent(m.host))
					want = cs[len(cs)-1]
				}
				if !strings.Contains(ansi.Cut(line, col, col+1), want) {
					t.Fatalf("full=%v row=%d col=%d missing chrome: %q", full, row, col, ansi.Cut(line, col, col+1))
				}
			}
		}
		border := ansi.Strip(rows[m.paneTop+4])
		if !strings.HasSuffix(border, "─") || !strings.HasPrefix(ansi.Strip(ansi.Cut(rows[m.paneTop+4], x-2, m.w)), "──") {
			t.Fatalf("border does not reach pane edges: %q", border)
		}
		if m.host.bodyTop != 5 {
			t.Fatalf("content starts at row %d, want immediately below the border at 5", m.host.bodyTop)
		}
		header := m.paneHeader(m.focused(), m.host, 160)
		if strings.TrimSpace(ansi.Strip(header[0])) != "" {
			t.Fatal("padding must be above the title")
		}
		if !strings.Contains(ansi.Strip(header[paneTabsRow]), "Conversation") {
			t.Fatal("tabs must sit immediately above the border")
		}
	}
}

func TestPaneDockStartsWithQueue(t *testing.T) {
	m, a, c := barAgentFixture(t)
	rows := m.paneDock(a, c, 100, 30)
	if len(rows) == 0 || !strings.Contains(ansi.Strip(rows[0]), "queue ·1") {
		t.Fatalf("blank strip before queue: %q", rows)
	}
	if c.qTop != 0 {
		t.Fatalf("queue starts at %d", c.qTop)
	}
	for row := range c.qAt {
		if !strings.Contains(ansi.Strip(rows[c.qTop+row]), "later") {
			t.Fatalf("queue hit row does not match rendered message: %d", row)
		}
	}
	if c.boxIdx >= len(rows) {
		t.Fatal("composer coordinate beyond dock")
	}
}

func TestPaneFrameOrdinaryRowsKeepGuttersPlain(t *testing.T) {
	m := &Model{}
	for _, header := range []bool{false, true} {
		var b strings.Builder
		m.paneFrameRow(&b, "body", 20, "", header, 6)
		if strings.Contains(b.String(), bgChrome) {
			t.Fatal("header background leaked into transcript")
		}
		if got := ansi.StringWidth(b.String()); got != 23 {
			t.Fatalf("width=%d", got)
		}
	}
}
