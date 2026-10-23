package ui

import (
	"os"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/instances"
)

// restoreEnv carries what was picked across #reload: whether the
// Session had the keys, then the agent selected.
const restoreEnv = "RUSH_RESTORE"

// reload is #reload: the view quits and main runs the installed rush in
// its place. Sessions run in hosts of their own, so they run on untouched.
func (m *Model) reload() tea.Cmd {
	m.reloading = true
	return m.quit()
}

// ReloadMsg is #reload asked for from outside (SIGUSR1, sent by `rush
// reload`): the same path, on the UI goroutine.
func ReloadMsg() tea.Msg {
	return applyMsg(func(m *Model) tea.Cmd { return m.reload() })
}

// reloadAll is #reload all: the other views are told, off the UI goroutine,
// then this one reloads like #reload.
func (m *Model) reloadAll() tea.Cmd {
	tell := func() tea.Msg { instances.Reload(os.Getpid()); return nil }
	return tea.Sequence(tell, m.reload())
}

// Reload is what the next rush starts with, when #reload asked for one.
func (m *Model) Reload() (env string, ok bool) {
	if !m.reloading {
		return "", false
	}
	pane := "0"
	if m.paneFocus {
		pane = "1"
	}
	return restoreEnv + "=" + pane + m.sel, true
}

// takeRestore reads what the last rush had picked, for applyRestore, and
// keeps it from anything this one runs.
func (m *Model) takeRestore() {
	v := os.Getenv(restoreEnv)
	_ = os.Unsetenv(restoreEnv)
	if len(v) > 1 {
		m.restore, m.restorePane = v[1:], v[0] == '1'
	}
}

// applyRestore selects the agent the last rush had, once it's listed.
func (m *Model) applyRestore() {
	if m.restore == "" {
		return
	}
	for _, l := range m.lines {
		if l.kind == lineAgent && l.agent.Key == m.restore {
			m.sel, m.paneFocus, m.restore = m.restore, m.restorePane, ""
			return
		}
	}
}

// watchBinary looks at the installed rush every 30 seconds, off the UI
// goroutine: see watchHosts. With RestartOnInstall the leading view
// restarts idle sessions onto it too.
func (m *Model) watchBinary() tea.Cmd {
	if m.tick%30 != 3 {
		return nil
	}
	return m.watchHosts(m.store.Config.RestartOnInstall && m.leads(), false)
}

// reloadFields are the Model's, kept here with what uses them.
type reloadFields struct {
	reloading, restorePane, rebuiltSaid bool
	rebuilt                             bool // a newer rush than this one is installed
	restore                             string
}
