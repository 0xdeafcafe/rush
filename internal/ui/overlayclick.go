package ui

import (
	"regexp"
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"
)

// The mouse in a box over the screen, a sheet, a question or a menu: one
// way for them all, which presses the keys a click stands for, so each
// does with it just what its keys do. Outside the box is esc; on a tab,
// ] until that tab's up; on a key in a row of keys, that key; on a row,
// ↑ or ↓ until the cursor's on it, then enter. Sheets that take the mouse
// themselves (mouseSheet) still do, inside the box.

// overlayHit is the box over the screen as last drawn: its edges, and
// where its body's first cell landed.
type overlayHit struct {
	x0, y0, x1, y1 int // the box, edges in, x1 and y1 past it
	bx, by         int // the body's top left cell
	on             bool
}

// keepOverlay notes a box drawn bw wide and h tall at (left, top), its
// body starting at (bx, by).
func (m *Model) keepOverlay(left, top, bw, h, bx, by int) {
	m.over = overlayHit{x0: left, y0: top, x1: left + bw, y1: top + h, bx: bx, by: by, on: true}
}

// overlayBody is the body of the box that has the keys, as it draws now,
// and whether there's one drawn.
func (m *Model) overlayBody() ([]string, bool) {
	o := m.over
	if !o.on {
		return nil, false
	}
	bw := o.x1 - o.x0
	switch {
	case m.keyHelp != nil:
		return m.keyHelpBox(bw-4, max(10, m.h*4/5)-2), true
	case m.bar != nil:
		return m.barBox(bw, max(8, m.h*2/3)-2), true
	case m.confirm != nil:
		return m.confirmBody(bw), true
	case m.sheet != nil:
		return m.sheet.body(m, bw-4, max(8, m.h-6)), true
	case m.picker != nil:
		return m.pickerBody(bw - 4), true
	}
	return nil, false
}

// overlayClick is a left click at (x, y) while a box has the keys,
// reporting whether the box took it.
func (m *Model) overlayClick(x, y int) (tea.Cmd, bool) {
	body, ok := m.overlayBody()
	if !ok {
		return nil, false
	}
	o := m.over
	if x < o.x0 || x >= o.x1 || y < o.y0 || y >= o.y1 {
		return m.press("esc"), true
	}
	if _, own := m.sheet.(mouseSheet); own && m.bar == nil && m.confirm == nil {
		return nil, false // the sheet's own, as it lays itself out
	}
	i, col := y-o.by, x-o.bx
	if i < 0 || i >= len(body) {
		return nil, true
	}
	if cmd, ok := m.tabClick(body, i, col); ok {
		return cmd, true
	}
	if cmd, ok := m.rowClick(body, i); ok {
		return cmd, true
	}
	if k := hintKey(body[i], col); k != "" {
		return m.press(k), true
	}
	return nil, true
}

// overlayWheel scrolls a box that has the keys as ↑ and ↓ do, reporting
// whether one took it.
func (m *Model) overlayWheel(up bool) (tea.Cmd, bool) {
	if _, ok := m.overlayBody(); !ok {
		return nil, false
	}
	if _, own := m.sheet.(mouseSheet); own && m.bar == nil && m.confirm == nil {
		return nil, false
	}
	if up {
		return m.press("up"), true
	}
	return m.press("down"), true
}

// press is a key pressed as the keyboard would, through every binding.
func (m *Model) press(s string) tea.Cmd {
	k, ok := keyOf(s)
	if !ok {
		return nil
	}
	return m.key(k)
}

// cursorRow is the body line the cursor's on, drawn with ▍ (sheetRow and
// the menus); -1 when there isn't one.
func cursorRow(body []string) int {
	for i, l := range body {
		if strings.HasPrefix(strings.TrimLeft(ansi.Strip(l), " "), "▍") {
			return i
		}
	}
	return -1
}

// rowText is a row's words, without the cursor's mark or the padding.
func rowText(l string) string {
	return strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(ansi.Strip(l)), "▍"))
}

// rowClick moves the cursor to the row clicked, then presses enter,
// reporting whether line i was a row. Only an indented line can be one;
// one the cursor never reaches (a note) leaves the cursor where it was.
func (m *Model) rowClick(body []string, i int) (tea.Cmd, bool) {
	plain := ansi.Strip(body[i])
	want := rowText(body[i])
	cur := cursorRow(body)
	if cur == i {
		return m.press("enter"), true
	}
	if cur < 0 || want == "" || !strings.HasPrefix(plain, " ") {
		return nil, false
	}
	dir, back := "down", "up"
	if i < cur {
		dir, back = back, dir
	}
	from, at := rowText(body[cur]), rowText(body[cur])
	var cmds []tea.Cmd
	moved := 0
	for range 256 {
		cmds = append(cmds, m.press(dir))
		b, ok := m.overlayBody()
		if !ok {
			return tea.Batch(cmds...), true
		}
		c := cursorRow(b)
		if c < 0 {
			break
		}
		now := rowText(b[c])
		if now == want {
			return tea.Batch(append(cmds, m.press("enter"))...), true
		}
		if now == at || now == from {
			break // the end of the list, or round to where it began
		}
		at = now
		moved++
	}
	for range moved {
		if b, ok := m.overlayBody(); !ok || cursorRow(b) >= 0 && rowText(b[cursorRow(b)]) == from {
			break
		}
		m.press(back)
	}
	return nil, false
}

// tabRow is a row of sheetTabs: its names, then [ ].
func tabRow(l string) bool { return strings.HasSuffix(strings.TrimSpace(ansi.Strip(l)), "[ ]") }

// tabClick switches to the tab clicked, pressing ] until it's the one up,
// reporting whether line i is a row of tabs.
func (m *Model) tabClick(body []string, i, col int) (tea.Cmd, bool) {
	if !tabRow(body[i]) {
		return nil, false
	}
	plain := ansi.Strip(body[i])
	lead := ansi.StringWidth(plain) - ansi.StringWidth(strings.TrimLeft(plain, " "))
	names := strings.Split(strings.TrimSuffix(strings.TrimSpace(plain), "[ ]"), "  ·  ")
	want, x := "", lead
	for _, n := range names {
		n = strings.TrimSpace(n)
		if w := ansi.StringWidth(n); col >= x && col < x+w {
			want = n
		}
		x += ansi.StringWidth(n) + 5
	}
	if want == "" {
		return nil, true
	}
	up := paint(cOrange+bold, want)
	var cmds []tea.Cmd
	for range names {
		b, ok := m.overlayBody()
		if !ok {
			break
		}
		at := slices.IndexFunc(b, tabRow)
		if at < 0 || strings.Contains(b[at], up) {
			break
		}
		cmds = append(cmds, m.press("]"))
	}
	return tea.Batch(cmds...), true
}

// hintGap parts the pairs in a row of keys: keys' "  ·  ", or a
// question's three spaces.
var hintGap = regexp.MustCompile(`\s+·\s+|\s{3,}`)

// hintKey is the key named at col in a row of keys (keys, keysFit, a
// question's), "" when col isn't on one or the line isn't such a row: the
// key has to be drawn as a key, bright before its dim label.
func hintKey(l string, col int) string {
	plain := ansi.Strip(l)
	x := 0
	rest := plain
	for rest != "" {
		seg, gap := rest, ""
		if loc := hintGap.FindStringIndex(rest); loc != nil {
			seg, gap, rest = rest[:loc[0]], rest[loc[0]:loc[1]], rest[loc[1]:]
		} else {
			rest = ""
		}
		w := ansi.StringWidth(seg)
		if col >= x && col < x+w {
			k, _, _ := strings.Cut(strings.TrimSpace(seg), " ")
			if k == "" || strings.ContainsAny(k, "↑↓←→") {
				return ""
			}
			if !strings.Contains(l, paint(cText+bold, k)+" ") && !strings.Contains(l, paint(cOrange, k)) {
				return ""
			}
			k = strings.ReplaceAll(k, "⌥", "alt+")
			if _, ok := keyOf(k); !ok {
				return ""
			}
			return k
		}
		x += w + ansi.StringWidth(gap)
	}
	return ""
}
