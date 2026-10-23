package ui

import (
	"regexp"
	"strings"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

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
	tag     int    // the row an @agent is pointed at on, pane row + 1
	agent   string // that agent's link
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
		if a, u := m.agentAt(c, x, y); a != nil {
			next.tag, next.agent = i+1, u
			if c.pointerHover.agent != u {
				m.flash(agentHandle(a)+" · "+oneLine(a.DisplayName)+" · click to open", false)
			}
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
	if t := c.pointerHover.tag - 1; t >= 0 && t < len(rows) {
		rows[t] = underlineLink(rows[t], c.pointerHover.agent)
	}
}

// agentAt is the agent tagged at x, y in the pane's text, and its link.
func (m *Model) agentAt(c *hostConn, x, y int) (*fleet.Agent, string) {
	i := y - m.paneTop
	if i < 0 || i >= len(c.rowBody) || c.rowBody[i] < 0 {
		return nil, ""
	}
	at, ok := m.textCell(c, x, y)
	if !ok || at.row >= len(c.shown) {
		return nil, ""
	}
	u := linkAt(c.shown[at.row].Text, at.col)
	return m.linkedAgent(u), u
}

// linkedAgent is the agent an @agent link opens, or nil.
func (m *Model) linkedAgent(u string) *fleet.Agent {
	tag, ok := strings.CutPrefix(u, "rush:agent/")
	if !ok {
		return nil
	}
	for _, a := range m.order {
		if mentionable(a) && tags(a, tag) {
			return a
		}
	}
	return nil
}

// underlineLink underlines the words of each link to u in row.
func underlineLink(row, u string) string {
	open := "\x1b]8;;" + u + "\x1b\\"
	var b strings.Builder
	for {
		i := strings.Index(row, open)
		if i < 0 {
			break
		}
		j := strings.Index(row[i+len(open):], "\x1b]8;;\x1b\\")
		if j < 0 {
			break
		}
		words := row[i+len(open) : i+len(open)+j]
		b.WriteString(row[:i] + open + "\x1b[4m" + strings.ReplaceAll(words, reset, reset+"\x1b[4m") + "\x1b[24m")
		row = row[i+len(open)+j:]
	}
	return b.String() + row
}

// setHandles tells the conversation view which agent each tag names, so
// an @agent draws in its colour and opens it when clicked. Its handle
// comes first; its nickname and title tag it where nothing else does.
func (m *Model) setHandles() {
	hs := map[string]convo.Handle{}
	add := func(tag string, h convo.Handle) {
		if k := strings.ToLower(tag); k != "" {
			if _, ok := hs[k]; !ok {
				hs[k] = h
			}
		}
	}
	for pass := range 2 {
		for _, a := range m.order {
			if !mentionable(a) {
				continue
			}
			name := mentionName(a)
			h := convo.Handle{Name: name, Tint: handleTint("@" + name), Title: oneLine(a.DisplayName)}
			if pass == 0 {
				add(name, h)
				continue
			}
			add(nickname(a.Key), h)
			add(titleTag(a.DisplayName), h)
			add(strings.Join(strings.Fields(oneLine(a.DisplayName)), "-"), h)
		}
	}
	convo.SetHandles(hs)
}
