package ui

import (
	"time"

	tea "charm.land/bubbletea/v2"
)

// backgroundGap is the least time between frames that work landing from
// the background draws: readings, scans and reads finish several times a
// second, and each drew a whole frame whether or not anything it read
// had changed. What you do (keys, the mouse, a resize) draws at once.
const backgroundGap = 250 * time.Millisecond

// held reports whether msg is work landing from the background.
func held(msg tea.Msg) bool {
	switch msg.(type) {
	case snapMsg, sheetMsg, scanMsg, previewsMsg, paneMsg, foldersMsg, quotaMsg, usageMsg, tempMeasuredMsg, spawnFoundMsg, barsMsg:
		return true
	}
	return false
}

// holdBackground keeps the last frame for work from the background that
// lands within backgroundGap of it, and asks for one frame when the gap
// is up: what it changed is drawn then.
func (m *Model) holdBackground(msg tea.Msg) tea.Cmd {
	if !held(msg) {
		return nil
	}
	wait := backgroundGap - time.Since(m.drewAt)
	if wait <= 0 {
		return nil
	}
	m.gate.Keep()
	m.heldAt = time.Now()
	if m.holdPending {
		return nil
	}
	m.holdPending = true
	return tea.Tick(wait, func(time.Time) tea.Msg { return heldFrameMsg{} })
}

// heldFrameMsg draws what the background changed while held: unless a
// frame since (the second's tick, a key) already has.
type heldFrameMsg struct{}

func (m *Model) heldFrame() {
	m.holdPending = false
	if m.drewAt.After(m.heldAt) {
		m.gate.Keep()
	}
}
