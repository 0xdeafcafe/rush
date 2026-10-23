package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A session's provider is the default one, wherever it starts: folders no
// longer pick one.
func TestStartOnDefault(t *testing.T) {
	m, _ := accountsModel(t)
	cfg := &m.store.Config
	if k := m.startKindIn("/elsewhere"); k != "claude" {
		t.Fatalf("new sessions run %q", k)
	}
	cfg.SetDefaultProvider("zplain")
	if k := m.startKindIn(filepath.Join("/work", "repo")); k != "zplain" {
		t.Fatalf("with zplain the default, new sessions run %q", k)
	}
}

// Moving on at a limit takes new sessions to the next provider with room
// once every account of the default is nearly out; otherwise they wait.
func TestPickFollowsRoom(t *testing.T) {
	m, _ := accountsModel(t)
	cfg := &m.store.Config
	cfg.SetDefaultProvider("zcodex")
	cfg.AtLimit = state.AtLimitNext
	if k := m.startKindIn("/x"); k != "zcodex" {
		t.Fatalf("with room, new sessions run %q", k)
	}
	m.quotas["zcodex:a2"] = m.quotas["zcodex:a1"] // both at 99%
	if k := m.startKindIn("/x"); k == "zcodex" {
		t.Fatalf("moving on with zcodex out, new sessions still run %q", k)
	}
	cfg.AtLimit = state.AtLimitAccount
	if pick, _ := m.startPick("/x"); pick.Kind != "zcodex" || !pick.Wait {
		t.Fatalf("staying with zcodex out, picked %+v", pick)
	}
}

// takerAgent is a provider that can start from another's conversation.
type takerAgent struct{ fakeAgent }

func (takerAgent) Features() map[agent.Feature]agent.Support {
	return map[agent.Feature]agent.Support{agent.FeatureHandoffIn: agent.Yes}
}

// A conversation a limit stopped under a profile that hands on is handed
// to the next provider that can take it once its own has no room, and
// only once.
func TestHandOffStopped(t *testing.T) {
	m, _ := accountsModel(t)
	agent.Register(takerAgent{fakeAgent{kind: "ztaker", title: "ZTaker", dir: "/z/.ztaker"}})
	if err := os.WriteFile(filepath.Join(os.Getenv("PATH"), "rush-fake-ztaker"), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	agent.Recheck()
	cfg := &m.store.Config
	cfg.SetDefaultProvider("zcodex")
	cfg.AtLimit = state.AtLimitNext
	a := &fleet.Agent{Key: "k1", DisplayName: "fixer", Rush: true, Kind: "zcodex", Profile: "zcodex", Cwd: "/x"}
	a.Detail = "usage limit · resets 15:00"
	m.snap.Agents = []*fleet.Agent{a}
	m.handOffStopped()
	if m.accts.handedOff["k1"] {
		t.Fatal("handed on while another zcodex account has room")
	}
	m.quotas["zcodex:a2"] = m.quotas["zcodex:a1"]
	m.handOffStopped()
	if !m.accts.handedOff["k1"] {
		t.Fatal("not handed on with every zcodex account out")
	}
	if cmd := m.handOffStopped(); cmd != nil && cmd() != nil {
		t.Fatal("handed on twice")
	}
}

// alt+w picks the default provider.
func TestProfilePicker(t *testing.T) {
	m, _ := accountsModel(t)
	cfg := &m.store.Config
	pick := func(label string) {
		t.Helper()
		m.openProfilePicker()
		for i, a := range m.picker.acts {
			if strings.Contains(ansi.Strip(a.label), label) {
				m.picker.cursor = i
				m.pickerKey(tea.KeyPressMsg{}, "enter")
				return
			}
		}
		t.Fatalf("no %q in the picker", label)
	}
	m.openProfilePicker()
	if m.picker == nil || !strings.Contains(ansi.Strip(m.picker.acts[m.picker.cursor].label), "★ ✻ Claude Code only") {
		t.Fatal("the picker didn't open on the default, Claude Code's own")
	}
	pick("ZCodex only")
	if d := cfg.Default(); d.Name != "zcodex" || !d.Builtin {
		t.Fatalf("default is %+v", d)
	}
}
