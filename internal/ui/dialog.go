package ui

import (
	"os"
	"os/exec"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
)

// Boxes drawn over a screen, and a file opened in your editor: shared by
// Settings, the pickers and the sheets.

// overlayBox draws body in a panel of width bw centred over base, dimming
// everything behind it.
func (m *Model) overlayBox(base string, body []string, bw int) string {
	lines := strings.Split(base, "\n")
	box := boxLines(body, bw)
	for y := range lines {
		lines[y] = faint(unplace(ansi.Strip(fit(lines[y], m.w))))
	}
	top, left := max(1, (len(lines)-len(box))/2), (m.w-bw)/2
	m.keepOverlay(left, top, bw, len(box), left+2, top+2)
	return strings.Join(pasteAt(lines, box, top, left), "\n")
}

// boxLines is body in a rounded box bw wide, a blank line inside top and
// bottom.
func boxLines(body []string, bw int) []string { return edgedBox(body, bw, cDim) }

// edgedBox is boxLines with its edge in col.
func edgedBox(body []string, bw int, col string) []string {
	inner := bw - 4
	edge := func(s string) string { return paint(col, s) }
	box := []string{edge("╭" + strings.Repeat("─", bw-2) + "╮")}
	for _, l := range append([]string{""}, append(body, "")...) {
		box = append(box, edge("│")+panel(" "+fit(l, inner)+" ")+edge("│"))
	}
	return append(box, edge("╰"+strings.Repeat("─", bw-2)+"╯"))
}

// pasteAt lays box over lines with its top left corner at (left, top).
func pasteAt(lines, box []string, top, left int) []string {
	for i, b := range box {
		y := top + i
		if y < 0 || y >= len(lines) {
			continue
		}
		l := lines[y]
		lines[y] = ansi.Truncate(l, left, "") + reset + b + ansi.TruncateLeft(l, left+cellw.String(ansi.Strip(b)), "")
	}
	return lines
}

var panelBG string // applyColors sets this and every other ground

func panel(s string) string {
	return panelBG + strings.ReplaceAll(s, reset, reset+panelBG) + reset
}

// editFile opens path in $VISUAL or $EDITOR, vi without either.
func editFile(path string) tea.Cmd {
	return editorCmd(func() (string, error) { return path, nil }, func(err error) tea.Msg { return dialogReload{err: err} })
}

// editorCmd hands the terminal to $VISUAL or $EDITOR (vi without either)
// on the file prepare gives, and done says how it went. prepare, and
// finding the shell to run the editor in, happen in the Cmd, off the UI
// goroutine; only the editor itself has the terminal while it runs.
func editorCmd(prepare func() (string, error), done func(error) tea.Msg) tea.Cmd {
	return func() tea.Msg {
		path, err := prepare()
		if err != nil {
			return done(err)
		}
		ed := firstNonEmpty(os.Getenv("VISUAL"), os.Getenv("EDITOR"), "vi")
		c := exec.Command("sh", "-c", ed+` "$1"`, "sh", path)
		return tea.ExecProcess(c, done)()
	}
}

type dialogReload struct{ err error }
