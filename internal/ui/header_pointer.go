package ui

import (
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/charmbracelet/x/ansi"
	"strings"
)

type headerHover struct {
	index       int
	page, valid bool
}

// Use the last rendered text for both click and hover, including clipped tabs.
// Pointer movement must not render the header or rescan accounts.
func (m *Model) headerTabAt(x, y int) headerHover {
	if m.store == nil || m.snap == nil || m.zen || m.sheet != nil || x < 0 || x >= m.w || y < 0 || y >= m.topH() {
		return headerHover{}
	}
	at := func(line string, names []string, page bool) headerHover {
		line = ansi.Strip(line)
		for i, n := range names {
			if j := strings.Index(line, n); j >= 0 {
				left := cellw.String(line[:j])
				right := left + cellw.String(n)
				if right <= m.w && x >= left-1 && x <= right {
					return headerHover{index: i, page: page, valid: true}
				}
			}
		}
		return headerHover{}
	}
	if y == m.headH() {
		names, _, _ := m.pageTabs()
		return at(m.headerPointerLines[1], names, true)
	}
	if y == 2 { // the tabs
		h := at(m.headerPointerLines[0], viewNames, false)
		if m.hosted != "" && h.index == placeProjects {
			return headerHover{}
		}
		return h
	}
	return headerHover{}
}

func (m *Model) hoverHeader(x, y int) bool {
	next := m.headerTabAt(x, y)
	if next == m.topHover {
		return false
	}
	m.topHover = next
	return true
}
