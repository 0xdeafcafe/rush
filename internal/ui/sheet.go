package ui

import (
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
)

// --- sheets ---

// A sheet is a panel over the screen for one of Claude Code's own screens
// done rush's way: /fork's options, /rewind, /plugins, /statusline, /skills. It
// takes every key until it closes (m.sheet = nil).
type sheet interface {
	// body draws the sheet's inside, w wide and at most h tall.
	body(m *Model, w, h int) []string
	key(m *Model, k tea.KeyPressMsg, s string) tea.Cmd
}

// sheetMsg is work a sheet started finishing: apply runs on the UI side.
type sheetMsg struct{ apply func(m *Model) tea.Cmd }

// sheetDo runs f off the UI and hands its result back to apply.
func sheetDo[T any](f func() (T, error), apply func(m *Model, v T, err error) tea.Cmd) tea.Cmd {
	return func() tea.Msg {
		v, err := f()
		return sheetMsg{apply: func(m *Model) tea.Cmd { return apply(m, v, err) }}
	}
}

// sizedSheet is a sheet that wants a width of its own (0 for the usual).
type sizedSheet interface{ width(m *Model) int }

// sheetWidth is the panel's width on this screen.
func (m *Model) sheetWidth() int {
	want := 112
	if s, ok := m.sheet.(sizedSheet); ok && s.width(m) > 0 {
		want = s.width(m)
	}
	return max(40, min(m.w-6, want))
}

// mouseSheet is a sheet that takes the mouse: x and y are within its
// body, as body drew it, and ev is what happened there.
type mouseSheet interface {
	mouse(m *Model, ev mouseEv, x, y int) tea.Cmd
}

type mouseEv int

const (
	mousePress mouseEv = iota
	mouseDrag          // moved with the left button down
	mouseRelease
	mouseWheelUp
	mouseWheelDown
)

// sheetView draws the open sheet over base, and keeps where its body
// landed for the mouse.
func (m *Model) sheetView(base string) string {
	bw := m.sheetWidth()
	body := m.sheet.body(m, bw-4, max(8, m.h-6))
	// As overlayBox places it: the edge and a blank line above the body,
	// the edge and a space left of it.
	box := len(body) + 4
	m.sheetAt = [2]int{(m.w-bw)/2 + 2, max(1, (strings.Count(base, "\n")+1-box)/2) + 2}
	return m.overlayBox(base, body, bw)
}

// sheetMouse hands the mouse to the open sheet, when it takes it.
func (m *Model) sheetMouse(ev mouseEv, x, y int) tea.Cmd {
	if s, ok := m.sheet.(mouseSheet); ok {
		return s.mouse(m, ev, x-m.sheetAt[0], y-m.sheetAt[1])
	}
	return nil
}

// sheetTitle is a sheet's first line: its name, and what it's for.
func sheetTitle(name, about string, w int) string {
	return paint(cText+bold, name) + "  " + dim(ansi.Truncate(about, max(0, w-ansi.StringWidth(name)-2), "…"))
}

// sheetTabs draws a row of tabs, the current one bright.
func sheetTabs(names []string, cur int) string {
	var parts []string
	for i, n := range names {
		if i == cur {
			parts = append(parts, paint(cOrange+bold, n))
		} else {
			parts = append(parts, dim(n))
		}
	}
	return strings.Join(parts, faint("  ·  ")) + faint("   [ ]")
}

// sheetRow is one choosable row: highlighted with ▍ when it's the cursor's.
func sheetRow(line string, on bool, w int) string {
	if on {
		return highlight(paint(cOrange, "▍")+" "+line, w)
	}
	return "  " + line
}

// window picks which of n rows to show in h lines, keeping cur in view.
func window(n, cur, h int) (from, to int) {
	if n <= h {
		return 0, n
	}
	from = max(0, min(cur-h/2, n-h))
	return from, from + h
}

// textField draws an editable line with a cursor at pos when focused.
func textField(buf []rune, pos int, focused bool, placeholder string, w int) string {
	if len(buf) == 0 && !focused {
		return faint(placeholder)
	}
	if !focused {
		return paint(cText, ansi.Truncate(string(buf), w, "…"))
	}
	pos = max(0, min(pos, len(buf)))
	before, after := string(buf[:pos]), string(buf[pos:])
	if len(buf) == 0 {
		return paint(cOrange, "▏") + faint(placeholder)
	}
	if over := ansi.StringWidth(before) - (w - 2); over > 0 {
		before = ansi.TruncateLeft(before, over, "…")
	}
	return paint(cText, before) + paint(cOrange, "▏") + paint(cText, ansi.Truncate(after, max(0, w-ansi.StringWidth(before)-1), "…"))
}

// center puts s in the middle of w cells.
func center(s string, w int) string {
	pad := max(0, w-cellw.String(s))
	return strings.Repeat(" ", pad/2) + s + strings.Repeat(" ", pad-pad/2)
}
