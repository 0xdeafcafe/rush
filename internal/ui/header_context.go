package ui

import (
	"fmt"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

func headerContext(c *hostConn, a *fleet.Agent) string {
	s := c.sess
	if s.Context <= 0 || s.ContextWindow() <= 0 {
		return ""
	}
	pct := ctxFill(a, int64(s.Context), int64(s.ContextWindow())).Pct()
	colour := cText
	if pct >= 90 {
		colour = cRed
	} else if pct >= 80 {
		colour = cYellow
	}
	value := paint(cSub, "context ") + paint(colour, fmt.Sprintf("%.0f%%", pct))
	if c.pointerHover.context {
		return hoverLine(value, cellWidthContext(pct))
	}
	return value
}
func cellWidthContext(pct float64) int { return len(fmt.Sprintf("context %.0f%%", pct)) }
func (m *Model) contextAt(c *hostConn, x, y int) bool {
	left := m.paneX() + c.contextLabel[0]
	return !m.zenFull() && y == m.paneTop+paneTitleRow && c.contextLabel[1] > 0 && x >= left && x < left+c.contextLabel[1]
}

// modeAt is whether x, y is on the permission mode in session c's header.
func (m *Model) modeAt(c *hostConn, x, y int) bool {
	left := m.paneX() + c.modeLabel[1]
	return !m.zenFull() && c.modeLabel[2] > 0 && y == m.paneTop+c.modeLabel[0] && x >= left && x < left+c.modeLabel[2]
}

// segAt is the column in row where word stands on its own in part, part
// being row's end (or its start, last false): where a header segment is,
// for a click on it.
func segAt(row, part, word string, last bool) (int, bool) {
	if word == "" || part == "" {
		return 0, false
	}
	off := strings.Index(row, part)
	if last {
		off = strings.LastIndex(row, part)
	}
	if off < 0 {
		return 0, false
	}
	for i := 0; i+len(word) <= len(part); {
		j := strings.Index(part[i:], word)
		if j < 0 {
			break
		}
		j += i
		end := j + len(word)
		if (j == 0 || part[j-1] == ' ') && (end == len(part) || part[end] == ' ') {
			return cellw.String(row[:off+j]), true
		}
		i = j + 1
	}
	return 0, false
}
