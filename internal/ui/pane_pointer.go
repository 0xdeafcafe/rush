package ui

import "regexp"

// Pane hit targets come from the rendered header and rows, so their feedback
// and click areas follow the same wrapping and clipping as the visible UI.
type paneTab struct {
	start, end int
	view       int
}
type paneHover struct {
	row     int // pane row + 1; zero is no row
	ref     string
	tab     int // view + 1
	label   bool
	context bool
}

func (m *Model) paneTabAt(c *hostConn, x, y int) int {
	if m.zenFull() || y != m.paneTop+paneTabsRow {
		return -1
	}
	x -= m.paneX()
	for _, tab := range c.paneTabs {
		if x >= tab.start && x < tab.end {
			return tab.view
		}
	}
	return -1
}

func (m *Model) hoverPane(x, y int) bool {
	c := m.host
	if c == nil {
		return false
	}
	next := paneHover{}
	if m.mode == modeList && m.dialog == nil && m.sheet == nil && !m.embedded && x >= m.paneX() && x < m.w {
		if i := m.paneTabAt(c, x, y); i >= 0 {
			next.tab = i + 1
		}
		next.label = m.clickLabel(c, x, y)
		next.context = m.contextAt(c, x, y)
		i := y - m.paneTop
		if x < m.paneX()+c.paneW && i >= c.bodyTop && i < len(c.rowRefs) && c.rowRefs[i] != "" {
			next.row, next.ref = i+1, c.rowRefs[i]
		}
	}
	if next == c.pointerHover {
		return false
	}
	c.pointerHover = next
	// Hover decorates the visible rows; it never changes transcript layout.
	c.scrollOnly = true
	return true
}

func (m *Model) clickPaneTab(c *hostConn, x, y int) bool {
	i := m.paneTabAt(c, x, y)
	if i < 0 {
		return false
	}
	c.view, c.scroll = i, 0
	c.scrollOnly = false
	m.paneFocus = true
	return true
}

var hoverBackground = regexp.MustCompile(`\x1b\[48;[0-9;]*m`)

// Hover is applied to the displayed copy, never the transcript cache.
func (c *hostConn) paintHover(rows []string) {
	i := c.pointerHover.row - 1
	if i < c.bodyTop || i >= len(rows) || i >= len(c.rowRefs) || c.pointerHover.ref == "" || c.rowRefs[i] != c.pointerHover.ref {
		return
	}
	// The row's own grounds give way to the hover's; a lit keycap or chip
	// keeps its own, as its dark text needs it.
	row := hoverBackground.ReplaceAllStringFunc(rows[i], func(bg string) string {
		if bg == qCapOn || bg == qChip || bg == kbHitBG {
			return bg
		}
		return ""
	})
	rows[i] = hoverLine(row, c.paneW)
}
