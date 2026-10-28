package ui

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/instances"
)

// restoreEnv carries what was picked across #reload: whether the
// Session had the keys, then the agent selected.
const restoreEnv = "RUSH_RESTORE"

// reload is #reload: the view quits and main runs the installed rush in
// its place. Sessions run in hosts of their own, so they run on; each is
// then moved onto the installed rush once it is between turns, by the
// leading view (watchHosts). The installed rush is run once first, off the
// UI goroutine: one that doesn't start leaves this view open, saying so.
func (m *Model) reload() tea.Cmd { return m.reloadAfter(wantAgentsReloaded) }

// wantAgentsReloaded asks for the sessions to follow onto the installed rush.
func wantAgentsReloaded() {
	if err := host.WantReload(host.ExeStamp()); err != nil {
		events.Log("reload-agents-failed", map[string]any{"err": err.Error()})
	}
}

// reloadAfter is reload, running then (off the UI goroutine) once the
// installed rush has started.
func (m *Model) reloadAfter(then func()) tea.Cmd {
	return func() tea.Msg {
		err := installedRuns()
		if err == nil && then != nil {
			then()
		}
		return applyMsg(func(m *Model) tea.Cmd {
			if err != nil {
				events.Log("reload-failed", map[string]any{"err": err.Error(), "stayed": true})
				m.fail("#reload: the installed rush doesn't start, so this one stays · " + err.Error())
				return nil
			}
			return m.reloadNow()
		})
	}
}

// reloadNow quits for main to run the installed rush.
func (m *Model) reloadNow() tea.Cmd {
	m.reloading = true
	return m.quit()
}

// installedRuns is whether the installed rush starts: --version, in 3s.
var installedRuns = func() error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, exe, "--version").CombinedOutput()
	if ctx.Err() != nil {
		return errors.New("--version didn't answer in 3s")
	}
	if err != nil {
		return fmt.Errorf("%v: %s", err, strings.TrimSpace(string(out)))
	}
	return nil
}

// ReloadMsg is #reload asked for from outside (SIGUSR1, sent by `rush
// reload`): the same path, on the UI goroutine.
func ReloadMsg() tea.Msg {
	return applyMsg(func(m *Model) tea.Cmd { return m.reload() })
}

// reloadAll is #reload all: once the installed rush starts, the other
// views are told, off the UI goroutine, then this one reloads like #reload.
func (m *Model) reloadAll() tea.Cmd {
	return m.reloadAfter(func() {
		wantAgentsReloaded()
		instances.Reload(os.Getpid())
	})
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
