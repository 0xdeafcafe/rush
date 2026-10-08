package ui

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/cellw"
)

// Geometry is recorded from the visible rows, after scrolling and wrapping.
// The pointer and keyboard therefore select the same setting at every size.
type settingsGeometry struct {
	top, x, width, railWidth, controlWidth int
	rows, sections                         []int
}

func (m *Model) settingsHeight() int { return max(1, m.h-len(m.header())-4) }

func (m *Model) settingsHeading(w int) []string {
	p := m.curPage()
	descriptions := map[int]string{
		pageProviders:  "Whose models you use: sign-ins, keys, limits and models.",
		pageHarnesses:  "The programs that run them, and their session defaults.",
		pageProfiles:   "Which providers a session gets, by folder and at a limit.",
		pageGeneral:    "Sessions, notifications, background work, and how the workspace looks.",
		pageKeys:       "Find, practice and customize your shortcuts.",
		pagePlugins:    "Manage extensions and the access they need.",
		pageUpdates:    "Keep rush and your installed tools up to date.",
	}
	title := ""
	desc := descriptions[m.dialog.page]
	if m.dialog.plugin != "" {
		title = paint(cText, m.dialog.plugin) + " · "
	}
	out := []string{fit(title+paint(cSub, desc), w), ""}
	if p.head != nil {
		if rows := p.head(m, w); len(rows) > 0 {
			out[1] = fit(rows[0], w)
		}
	}
	if m.dialog.page == pageProviders {
		out[1] = fit(m.provTotals(), w)
	}
	return out
}

// settingsForm keeps navigation, controls and selected-setting help stationary.
// Only the settings window moves; the footer and preview never fall offscreen.
func (m *Model) settingsForm(secs []section, pageKeys []string, w, h int) []string {
	d := m.dialog
	all := flat(secs)
	previews := false
	for _, st := range all {
		previews = previews || st.preview != nil
	}
	d.cursor = max(0, min(d.cursor, max(0, len(all)-1)))
	cur := rowAt(secs, d.cursor)
	h = max(1, h)
	railW, helpW := 0, 0
	if w >= 100 && !previews {
		helpW = min(58, w/3)
	}
	if w >= 145 && len(secs) > 1 && !previews {
		railW = 24
	}
	mainW := w
	if helpW > 0 {
		mainW -= helpW + 3
	}
	if railW > 0 {
		mainW -= railW + 3
	}

	// Full form preserves section context and supports crossing a boundary with
	// a single arrow. The rail is a shortcut, not a second navigation mode.
	rows := m.formRows(secs, d.cursor, mainW)
	indices := make([]int, len(rows))
	for i := range indices {
		indices[i] = -1
	}
	line, index, selectedSection := 0, 0, 0
	starts := make([]int, len(secs))
	for si, sec := range secs {
		starts[si] = index
		line++
		if sec.title != "" {
			line++
		}
		for range sec.rows {
			indices[line] = index
			if index == d.cursor {
				selectedSection = si
			}
			index++
			line++
		}
	}
	cursorLine := 0
	for i, n := range indices {
		if n == d.cursor {
			cursorLine = i
			break
		}
	}
	// A bottom inspector is deliberately small on a narrow screen. Wide screens
	// use the entire right pane, including real previews for visual preferences.
	extra := []string{"?", "details"}
	if len(secs) > 1 {
		extra = append(extra, "ctrl+↑↓", "section")
	}
	footer := m.formKeys(cur, append(extra, pageKeys...), w)
	contentH := max(1, h-2)
	var help []string
	if helpW > 0 {
		help = m.settingsHelp(cur, helpW, contentH)
	}
	bottomH := 0
	if helpW == 0 && contentH >= 9 {
		bottomH = min(7, contentH/3)
		if previews && contentH >= 24 {
			bottomH = min(26, contentH*2/3)
		}
		help = m.settingsHelp(cur, w, bottomH)
	}
	listH := max(1, contentH-bottomH)
	first, last := window(len(rows), cursorLine, listH)
	// Do not use frame's cursor scrolling: the complete body fits its viewport.
	d.formGeometry = settingsGeometry{top: len(m.header()) + len(m.underHead()) + 2, x: 2, width: w, railWidth: railW, controlWidth: mainW, rows: indices[first:last]}
	if railW > 0 {
		d.formGeometry.sections = starts
	}
	out := make([]string, 0, h)
	for i := range contentH {
		left, middle, right := "", "", ""
		if railW > 0 && i < len(secs) {
			left = "  " + secs[i].title
			if i == selectedSection {
				left = paint(cOrange, "▸ ") + paint(cText+bold, secs[i].title)
			} else {
				left = paint(cSub, left)
			}
		}
		if i < listH && first+i < last {
			middle = rows[first+i]
		}
		if helpW == 0 && i >= listH && i-listH < len(help) {
			middle = help[i-listH]
		}
		if helpW > 0 && i < len(help) {
			right = help[i]
		}
		row := fit(middle, mainW)
		if railW > 0 {
			row = fit(left, railW) + faint(" │ ") + row
		}
		if helpW > 0 {
			row += faint(" │ ") + fit(right, helpW)
		}
		out = append(out, row)
	}
	progress := ""
	if len(all) > 0 {
		progress = fmt.Sprintf("%d of %d", d.cursor+1, len(all))
	}
	out = append(out, rule("", progress, w), footer)
	return out[:min(h, len(out))]
}

func (m *Model) settingsHelp(st setting, w, h int) []string {
	title, what, now := st.label, st.what, st.now()
	if st.about != nil {
		title, what, now = st.about()
	}
	out := []string{paint(cText+bold, fit(title, w)), ""}
	if what != "" {
		for _, line := range wrap(what, max(1, w)) {
			out = append(out, paint(cSub, line))
		}
	}
	if now != "" {
		out = append(out, "")
		for _, line := range wrap(now, max(1, w)) {
			out = append(out, paint(cText, line))
		}
	}
	if st.preview != nil && h-len(out) >= 5 {
		out = append(out, "", paint(cSub, "Preview"))
		out = append(out, st.preview(max(1, w))...)
	}
	if len(out) > h {
		out = out[:max(1, h)]
		// The full explanation remains available on a larger terminal. Keep the
		// current value visible even when the description needs clipping.
		if now != "" && h > 2 {
			out[h-1] = paint(cText, fit(now, w))
		}
	}
	return out
}

func (m *Model) settingsNavigation(s string) (tea.Cmd, bool) {
	d := m.dialog
	if d.page == pageKeys {
		return nil, false
	}
	if m.curPage().form == nil {
		return nil, false
	}
	n := m.dialogLen()
	step := max(1, (m.settingsHeight()-4)/2)
	switch s {
	case "?":
		m.sheet = &settingHelpSheet{setting: rowAt(m.curPage().form(m), d.cursor)}
		return nil, true
	case "ctrl+up", "ctrl+down":
		starts := []int{}
		at := 0
		for _, sec := range m.curPage().form(m) {
			if len(sec.rows) > 0 {
				starts = append(starts, at)
			}
			at += len(sec.rows)
		}
		if s == "ctrl+down" {
			for _, at := range starts {
				if at > d.cursor {
					d.cursor = at
					break
				}
			}
		} else {
			for i := len(starts) - 1; i >= 0; i-- {
				if starts[i] < d.cursor {
					d.cursor = starts[i]
					break
				}
			}
		}
	case "home":
		d.cursor = 0
	case "end":
		d.cursor = max(0, n-1)
	case "pgup":
		d.cursor = max(0, d.cursor-step)
	case "pgdown":
		d.cursor = min(max(0, n-1), d.cursor+step)
	default:
		return nil, false
	}
	return nil, true
}

func (m *Model) settingsMouse(x, y int) tea.Cmd {
	d := m.dialog
	if d.asking != "" {
		return nil
	}
	if d.page == pageKeys {
		row := y - m.headH() - 4 - d.keyTop + d.keyFrom
		if x >= 2 && x < m.w-2 && row >= d.keyFrom && row < d.keyTo {
			if d.cursor == row {
				return m.keysKey("enter")
			}
			d.cursor = row
		}
		return nil
	}
	if m.curPage().form == nil {
		return m.catalogMouse(x, y)
	}
	g := d.formGeometry
	at := y - g.top
	if x < g.x || x >= g.x+g.width || at < 0 {
		return nil
	}
	if g.railWidth > 0 && x < g.x+g.railWidth {
		if at < len(g.sections) {
			d.cursor = g.sections[at]
		}
		return nil
	}
	controlX := g.x
	if g.railWidth > 0 {
		controlX += g.railWidth + 3
	}
	controlW := g.controlWidth
	if x < controlX || x >= controlX+controlW || at >= len(g.rows) || g.rows[at] < 0 {
		return nil
	}
	row := g.rows[at]
	if d.cursor == row {
		return m.formKey(m.curPage().form(m), "enter")
	}
	d.cursor = row
	return nil
}

func (m *Model) settingsWheel(x, y, delta int) tea.Cmd {
	d := m.dialog
	if d.asking != "" {
		return nil
	}
	if m.curPage().form != nil || d.page == pageKeys {
		if y >= m.headH()+4 && x >= 2 && x < m.w-2 {
			d.cursor = max(0, min(max(0, m.dialogLen()-1), d.cursor+delta))
		}
		return nil
	}
	return m.catalogWheel(x, y, delta)
}

// The inspector can be expanded on a small terminal without changing a value.
type settingHelpSheet struct {
	setting               setting
	scroll, height, count int
}

func (*settingHelpSheet) width(*Model) int { return 92 }
func (s *settingHelpSheet) body(m *Model, w, h int) []string {
	rows := m.settingsHelp(s.setting, w, 10000)
	s.height, s.count = max(1, h-2), len(rows)
	s.scroll = max(0, min(s.scroll, max(0, len(rows)-s.height)))
	out := append([]string{}, rows[s.scroll:min(len(rows), s.scroll+s.height)]...)
	return append(out, "", keysFit(w, "↑↓", "scroll", "esc", "back"))
}
func (s *settingHelpSheet) key(m *Model, _ tea.KeyPressMsg, key string) tea.Cmd {
	switch key {
	case "esc", "q", "?":
		m.sheet = nil
	case "up", "k":
		s.scroll--
	case "down", "j":
		s.scroll++
	case "pgup":
		s.scroll -= s.height
	case "pgdown", "space":
		s.scroll += s.height
	case "home":
		s.scroll = 0
	case "end":
		s.scroll = s.count
	}
	s.scroll = max(0, min(s.scroll, max(0, s.count-s.height)))
	return nil
}
func (s *settingHelpSheet) mouse(m *Model, ev mouseEv, _, _ int) tea.Cmd {
	if ev == mouseWheelDown {
		return s.key(m, tea.KeyPressMsg{}, "down")
	}
	if ev == mouseWheelUp {
		return s.key(m, tea.KeyPressMsg{}, "up")
	}
	return nil
}

// Keep the current page and its neighbours visible when the full navigation
// does not fit. Hit testing reads these same rendered names.
func (m *Model) settingsTabs(w int) string {
	lo, hi := placePages(m.view)
	pages := m.settingsPages()[lo:hi]
	cur := m.dialog.page - lo
	render := func(from, to int) string {
		var tabs []string
		if from > 0 {
			tabs = append(tabs, dim("‹"))
		}
		for i := from; i < to; i++ {
			text := paint(cSub, pages[i].name)
			if i == cur {
				text = paint(cOrange+bold+"\x1b[4m", pages[i].name)
			} else if m.topHover.valid && m.topHover.page && m.topHover.index == i {
				text = hoverLine(paint(cText, pages[i].name), cellw.String(pages[i].name))
			}
			tabs = append(tabs, text)
		}
		if to < len(pages) {
			tabs = append(tabs, dim("›"))
		}
		return strings.Join(tabs, "   ")
	}
	from, to := 0, len(pages)
	for cellw.String(render(from, to)) > w && to-from > 1 {
		if cur-from > to-cur-1 {
			from++
		} else {
			to--
		}
	}
	tabs := render(from, to)
	if cellw.String(tabs)+6 <= w {
		tabs += dim("   [ ]")
	}
	return fit(tabs, w)
}
