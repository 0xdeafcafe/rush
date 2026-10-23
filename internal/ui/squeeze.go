package ui

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/squeeze"
)

// squeezedMsg says what a pass of transcript compression did.
type squeezedMsg squeeze.Result

// squeezeTranscripts stores transcripts untouched for two days compressed,
// in the background: two minutes after rush opens, then every six hours.
// They read exactly as before, to Claude Code and everything else.
func (m *Model) squeezeTranscripts() tea.Cmd {
	if m.store.Config.KeepTranscriptsPlain || m.squeezing || (m.tick != 120 && m.tick%21600 != 120) {
		return nil
	}
	p := m.store.Config.ActiveAccount().Profile()
	t, ok := agent.As[agent.Transcripts](p.Kind)
	if !ok {
		return nil
	}
	dirs := []string{t.TranscriptsDir(p)}
	m.squeezing = true
	return func() tea.Msg { return squeezedMsg(squeeze.Transcripts(dirs, 48*time.Hour)) }
}

func (m *Model) onSqueezed(msg squeezedMsg) {
	m.squeezing = false
	if msg.Files > 0 || msg.Failed > 0 {
		events.Log("transcripts-compressed", map[string]any{"files": msg.Files, "failed": msg.Failed, "bytesBefore": msg.Before, "bytesAfter": msg.After})
	}
	if msg.Files == 0 {
		return
	}
	m.flash(fmt.Sprintf("stored %d idle transcripts compressed: %s → %s on disk · they read the same as before",
		msg.Files, disk(msg.Before), disk(msg.After)), false)
}
