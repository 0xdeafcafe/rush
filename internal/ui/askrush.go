package ui

import (
	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// askRush starts an agent that answers questions about rush and changes
// its settings: it works in rush's own folder, with the guide in its
// system prompt. rush picks up what it writes to config.json as it's
// written (reloadConfig).
func (m *Model) askRush(question string) tea.Cmd {
	dir := state.Dir()
	return m.startHosted(question, dir, func(c *host.Config) {
		if question == "" {
			c.Name = "asking rush"
		}
		c.OwnWorktree = false // rush's folder: no project's work goes on there
		c.SystemPrompt = "The user is asking about rush itself: the terminal app you run inside, which runs and watches coding agents. " +
			"Answer from rush's guide below, and say so when it doesn't cover something rather than guessing.\n\n" +
			"You work in rush's own folder, " + dir + ". config.json there holds rush's settings; the other files are its records, so leave them alone. " +
			"When the user wants a setting changed, first say whether one of rush's # commands does it (they type it in rush's box, e.g. #mackeys on), or a page of Settings does. " +
			"Otherwise edit config.json: change only what was asked, keep it valid JSON, and tell them what you changed. " +
			"rush reads it again within a few seconds of the change.\n\n" +
			"<rush-guide>\n" + rush.Guide + "\n</rush-guide>"
	})
}

type configMsg []byte

// reloadConfig looks at config.json off the UI's goroutine: something
// other than this rush (an #ask agent, another rush, an editor) may have
// changed it.
func reloadConfig() tea.Msg {
	if b, ok := state.ConfigOnDisk(); ok {
		return configMsg(b)
	}
	return nil
}

func (m *Model) onConfig(b configMsg) {
	err := m.store.Reload(b)
	events.Log("config-reload", map[string]any{"why": "config.json changed outside this rush", "err": errText(err)})
	if err != nil {
		m.news("rush can't read config.json: it changed outside rush, "+err.Error(), "")
		return
	}
	m.refresh()
	m.news("settings reloaded: config.json changed outside rush, and rush is using them", "")
}
