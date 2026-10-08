package ui

import (
	tea "charm.land/bubbletea/v2"
	uv "github.com/charmbracelet/ultraviolet"

	"github.com/0xdeafcafe/photon/termimg"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// Pictures in Kitty graphics (termimg): the terminal is asked its cell size
// in pixels as the window is sized, so a picture keeps its shape, and the
// pictures convo makes are written to it raw, outside the frame.

// askCell asks the terminal for its cell size, where pictures are drawn.
func askCell() tea.Cmd {
	if !termimg.Drawn() {
		return nil
	}
	return tea.Raw(termimg.QueryCell)
}

// onCellSize takes the terminal's answer, its cell or its window in
// pixels, reporting whether msg was one.
func (m *Model) onCellSize(msg tea.Msg) bool {
	switch msg := msg.(type) {
	case uv.CellSizeEvent:
		termimg.SetCell(msg.Width, msg.Height)
	case uv.PixelSizeEvent:
		if !termimg.Measured() && m.w > 0 && m.h > 0 {
			termimg.SetCell(msg.Width/m.w, msg.Height/m.h)
		}
	default:
		return false
	}
	return true
}

// pictureMsg says a picture's made: the frame after it draws it.
type pictureMsg struct{}

var pictureWaiting bool // a Cmd waits on convo.PictureMade; UI goroutine only

// pictureCmds writes the pictures made since the last message, raw, and
// waits for the next to be made.
func pictureCmds(msg tea.Msg) tea.Cmd {
	if _, ok := msg.(pictureMsg); ok {
		pictureWaiting = false
	}
	var cmds []tea.Cmd
	if s := convo.PictureWrites(); s != "" {
		cmds = append(cmds, tea.Raw(s))
	}
	if !pictureWaiting && convo.PicturesBusy() {
		pictureWaiting = true
		cmds = append(cmds, func() tea.Msg { <-convo.PictureMade(); return pictureMsg{} })
	}
	return tea.Batch(cmds...)
}

// unplace blanks a faded screen's pictures, whose ids went with its colours.
func unplace(s string) string { return termimg.Blank(s) }
