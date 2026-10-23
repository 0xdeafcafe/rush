package ui

import (
	"errors"
	"os"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/host"
)

// #reload hands the next rush the agent selected and where the keys were,
// and that rush picks them up once the agent is listed.
func TestReloadKeepsThePlace(t *testing.T) {
	m, _ := benchModel(120, 40)
	m.paneFocus = true
	sel := m.sel
	if _, ok := m.Reload(); ok {
		t.Fatal("no reload asked for")
	}
	m.reloadNow()
	env, ok := m.Reload()
	if !ok || sel == "" {
		t.Fatalf("reload %v, sel %q", ok, sel)
	}
	k, v, _ := strings.Cut(env, "=")
	t.Setenv(k, v)
	n, _ := benchModel(120, 40)
	n.sel, n.paneFocus = "", false
	n.takeRestore()
	if os.Getenv(restoreEnv) != "" {
		t.Error("the next rush's own children shouldn't inherit it")
	}
	n.rebuild()
	if n.sel != sel || !n.paneFocus {
		t.Fatalf("restored %q pane %v, want %q", n.sel, n.paneFocus, sel)
	}
}

// `rush reload` signals the view, which sends ReloadMsg: it lands as #reload.
func TestReloadMsgIsReload(t *testing.T) {
	m, _ := benchModel(120, 40)
	m.paneFocus = true
	stubInstalled(t, nil)
	cmd := ReloadMsg().(applyMsg)(m)
	cmd().(applyMsg)(m)
	if _, ok := m.Reload(); !ok {
		t.Fatal("ReloadMsg didn't reload")
	}
}

// An installed rush that doesn't start leaves the view open, saying so.
func TestReloadStaysWhenTheInstalledRushFails(t *testing.T) {
	m, _ := benchModel(120, 40)
	stubInstalled(t, errors.New("exit status 2: dyld: missing symbol"))
	m.reload()().(applyMsg)(m)
	if _, ok := m.Reload(); ok {
		t.Fatal("reloaded onto a rush that doesn't start")
	}
	if !strings.Contains(m.status, "doesn't start") || !strings.Contains(m.status, "missing symbol") {
		t.Fatalf("said %q", m.status)
	}
}

func stubInstalled(t *testing.T, err error) {
	was := installedRuns
	installedRuns = func() error { return err }
	t.Cleanup(func() { installedRuns = was })
}

// #reload asks the sessions to follow onto the installed rush.
func TestReloadAsksTheAgentsToFollow(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(120, 40)
	stubInstalled(t, nil)
	m.reload()().(applyMsg)(m)
	if !host.ReloadWanted(host.ExeStamp()) {
		t.Fatal("#reload didn't ask the sessions to follow")
	}
}
