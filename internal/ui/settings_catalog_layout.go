package ui

import (
	"fmt"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/charmbracelet/x/ansi"
)

type catalogGeometry struct {
	x, width, listWidth, listTop, detailX, detailTop int
	wide                                             bool
	listRows, detailRows, allDetails                 []int
}

// Catalogs keep the list and selected detail visible together. Page scrolling
// belongs to the detail, independently of the selected row in the list.
func (m *Model) catalogKey(key string, inside bool) (tea.Cmd, bool) {
	d := m.dialog
	step := max(1, min(8, m.h-len(m.header())-14))
	switch key {
	case "home":
		d.cursor = 0
		d.catalogScroll = 0
		return nil, true
	case "end":
		d.cursor = max(0, m.dialogLen()-1)
		if inside {
			d.catalogScroll = -1
		}
		return nil, true
	case "pgdown":
		d.catalogScroll = max(0, d.catalogScroll) + step
		return nil, true
	case "pgup":
		d.catalogScroll = max(0, d.catalogScroll-step)
		return nil, true
	case "up", "down", "j", "k":
		if inside {
			d.catalogScroll = -1
		} else {
			d.catalogScroll = 0
		}
	}
	return nil, false
}

func (m *Model) catalogBody(catalog []section, picked int, inside, accounts bool, w int, detail func(int) ([]string, int)) []string {
	d := m.dialog
	d.catalogGeometry = catalogGeometry{}
	height := max(6, m.h-len(m.header())-6)
	paneHeight := max(2, height-1)
	wide := w >= 72
	if accounts {
		wide = w >= 100
	}
	lw, dw := w, w
	if wide {
		lw = min(36, max(24, w/4))
		dw = w - lw - 3
	}
	catalog = slices.Clone(catalog)
	if len(catalog) > 0 && catalog[0].title == m.curPage().name {
		catalog[0].title = ""
	}
	list, listCursor, listIndices := catalogList(catalog, picked, inside, lw)
	rows, detailCursor := detail(dw)
	if !wide {
		// A short neighborhood leaves a useful detail preview even on a narrow
		// terminal. Selecting another item always updates it without Enter.
		listHeight := min(3, len(list))
		if inside {
			listHeight = 1
			list = []string{paint(cOrange, "▸ ") + rowAt(catalog, picked).label}
			listCursor = 0
			listIndices = []int{picked}
		}
		first, last := window(len(list), listCursor, listHeight)
		list = list[first:last]
		listIndices = listIndices[first:last]
		paneHeight = max(2, paneHeight-len(list)-1)
	}
	maxScroll := max(0, len(rows)-paneHeight)
	if d.catalogScroll < 0 {
		d.catalogScroll = max(0, detailCursor-paneHeight+2)
	}
	d.catalogScroll = min(maxScroll, max(0, d.catalogScroll))
	end := min(len(rows), d.catalogScroll+paneHeight)
	shown := rows[d.catalogScroll:end]
	g := &d.catalogGeometry
	g.x, g.width, g.listWidth, g.wide = 2, w, lw, wide
	g.listTop = len(m.header()) + len(m.underHead()) + 2
	g.detailTop, g.detailX = g.listTop, 2+lw+3
	if !wide {
		g.detailTop += len(list) + 1
		g.detailX = 2
	}
	if len(g.allDetails) == len(rows) {
		g.detailRows = g.allDetails[d.catalogScroll:end]
	}
	var out []string
	if wide {
		first, last := window(len(list), listCursor, paneHeight)
		list = list[first:last]
		listIndices = listIndices[first:last]
		for i := range paneHeight {
			left, right := "", ""
			if i < len(list) {
				left = list[i]
			}
			if i < len(shown) {
				right = shown[i]
			}
			out = append(out, fit(left, lw)+faint(" │ ")+fit(right, dw))
		}
	} else {
		out = append(out, list...)
		out = append(out, faint(strings.Repeat("─", w)))
		for _, line := range shown {
			out = append(out, fit(line, w))
		}
	}
	controls := []string{"↑↓", "select", "→", "controls"}
	if inside {
		controls = []string{"↑↓", "select", "←→", "change", "enter", "action"}
	}
	if accounts {
		if inside {
			controls = append(rowAt(m.provForm(m.provPicked()), d.cursor).keys, "←→", "change")
		} else {
			controls = m.listKeys(m.provPicked())
		}
	}
	if len(rows) > paneHeight {
		controls = append([]string{"PgUp/Dn", fmt.Sprintf("detail %d/%d", d.catalogScroll+1, len(rows))}, controls...)
	}
	controls = append(controls, "[ ]", "pages", "esc", "back")
	out = append(out, keysFit(w, controls...))
	g.listRows = listIndices
	return out
}

func catalogList(secs []section, picked int, inside bool, w int) ([]string, int, []int) {
	var rows []string
	var indices []int
	cursor, index := 0, 0
	for _, sec := range secs {
		if sec.title != "" {
			rows = append(rows, paint(cSub+bold, ansi.Truncate(sec.title, w, "…")))
			indices = append(indices, -1)
		}
		for _, row := range sec.rows {
			line := "  " + ansi.Truncate(row.label, max(1, w-2), "…")
			if index == picked {
				cursor = len(rows)
				if inside {
					line = paint(cOrange, "▸ ") + paint(cText, ansi.Truncate(row.label, max(1, w-2), "…"))
				} else {
					line = highlight(paint(cOrange, "▍ ")+paint(cText, ansi.Truncate(row.label, max(1, w-2), "…")), w)
				}
			}
			rows = append(rows, line)
			indices = append(indices, index)
			index++
		}
	}
	return rows, cursor, indices
}

func (m *Model) catalogDetail(secs []section, k agent.Kind, model string, inside bool, w int) ([]string, int) {
	// The header already names this pairing; its first form title repeats it.
	secs = slices.Clone(secs)
	if len(secs) > 0 {
		secs[0].title, secs[0].note = "", ""
	}
	head := []string{paint(cText+bold, agent.ProviderLabel(agent.ProviderOf(k))) + dim(" · ") + paint(cText, agent.HarnessLabel(k))}
	if model != "" {
		head = append(head, paint(cBright, model), modelTakes(k, model))
	}
	if !agent.Runs(k) {
		head = append(head, paint(cYellow, "Not installed · ")+dim(agent.Hint(k)))
	}
	cur := -1
	if inside {
		cur = max(0, min(m.dialog.cursor, max(0, len(flat(secs))-1)))
		if cur != m.dialog.cursor {
			m.dialog.catalogScroll = -1
		}
		m.dialog.cursor = cur
	}
	rows := append(head, m.formRows(secs, cur, w)...)
	cursor := -1
	if inside {
		for i, row := range rows {
			if strings.Contains(row, selBG) {
				cursor = i
				break
			}
		}
		selected := rowAt(secs, cur)
		if selected.what != "" {
			rows = append(rows, "", dim(ansi.Truncate(selected.what, w, "…")))
		}
	}
	rows = append(rows, "", rule("What it can do", "", w))
	rows = append(rows, featureSummary(func(f agent.Feature) agent.Support { return pairingSupport(k, model, f) }, w)...)
	indices := make([]int, len(rows))
	for i := range indices {
		indices[i] = -1
	}
	line, index := len(head), 0
	for _, sec := range secs {
		line++
		if sec.title != "" {
			line++
		}
		for range sec.rows {
			indices[line] = index
			line++
			index++
		}
	}
	m.dialog.catalogGeometry.allDetails = indices
	return rows, cursor
}

func pairingSupport(k agent.Kind, model string, f agent.Feature) agent.Support {
	support := agent.FeatureOf(k, f)
	if agent.KeyOnly(k) && slices.Contains([]agent.Feature{agent.FeatureSwitch, agent.FeatureSignIn, agent.FeatureQuota}, f) {
		return agent.No
	}
	if f == agent.FeatureImages && model != "" {
		if reader, ok := agent.As[agent.MediaReader](k); ok {
			if media, known := reader.Reads(model); known && media&agent.MediaImage == 0 {
				return agent.No
			}
		}
	}
	return support
}


// Coordinates are terminal cells, matching frame's two-cell left margin.
func (m *Model) catalogMouse(x, y int) tea.Cmd {
	d := m.dialog
	if d == nil || d.asking != "" || d.page != pageProviders && d.page != pageHarnesses {
		return nil
	}
	g := d.catalogGeometry
	if x < g.x || x >= g.x+g.width {
		return nil
	}
	if y >= g.listTop && y < g.listTop+len(g.listRows) && (!g.wide || x < g.x+g.listWidth) {
		at := g.listRows[y-g.listTop]
		if at < 0 {
			return nil
		}
		if !d.harnessInside && !d.inside && d.cursor == at {
			return m.curPage().key(m, "enter")
		}
		d.harnessInside, d.inside = false, false
		d.cursor, d.catalogScroll = at, 0
		return nil
	}
	if x < g.detailX || y < g.detailTop || y >= g.detailTop+len(g.detailRows) {
		return nil
	}
	at := g.detailRows[y-g.detailTop]
	if at < 0 {
		return nil
	}
	if d.page != pageHarnesses {
		if d.inside && d.cursor == at {
			return m.providersKey("enter")
		}
		if !d.inside {
			d.pick, d.inside = d.cursor, true
		}
		d.cursor = at
		return nil
	}
	if d.harnessInside && d.cursor == at {
		return m.formKey(m.harnessDetailSections(), "enter")
	}
	if !d.harnessInside {
		d.harnessReturn, d.harnessInside = d.cursor, true
	}
	d.cursor = at
	return nil
}

func (m *Model) catalogWheel(x, y, delta int) tea.Cmd {
	d := m.dialog
	if d == nil || d.asking != "" || d.page != pageProviders && d.page != pageHarnesses {
		return nil
	}
	g := d.catalogGeometry
	if x < g.x || x >= g.x+g.width {
		return nil
	}
	if y >= g.listTop && y < g.listTop+len(g.listRows) && (!g.wide || x < g.x+g.listWidth) {
		at, n := d.cursor, 0
		if d.page != pageHarnesses {
			if d.inside {
				at = d.pick
			}
			n = len(m.provItems())
		} else {
			if d.harnessInside {
				at = d.harnessReturn
			}
			n = len(flat(m.harnessCatalogSections()))
		}
		d.harnessInside, d.inside = false, false
		d.cursor = max(0, min(max(0, n-1), at+delta))
		d.catalogScroll = 0
		return nil
	}
	if x >= g.detailX && y >= g.detailTop {
		d.catalogScroll = max(0, d.catalogScroll+delta)
	}
	return nil
}

// Form rows and pointer indices share the same section spacing.
func formLineIndices(secs []section, head, total int) []int {
	indices := make([]int, total)
	for i := range indices {
		indices[i] = -1
	}
	line, index := head, 0
	for _, sec := range secs {
		line++
		if sec.title != "" {
			line++
		}
		for range sec.rows {
			indices[line] = index
			line++
			index++
		}
	}
	return indices
}
