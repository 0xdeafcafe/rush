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

// A session's provider comes from its profile: its folder's rule, or the
// one picked with #preset, which is for one session.
func TestStartUnderProfile(t *testing.T) {
	m, _ := accountsModel(t)
	cfg := &m.store.Config
	cfg.SetProfile("", state.Profile{Name: "Default", Providers: []string{"claude", "zcodex"}})
	cfg.SetProfile("", state.Profile{Name: "plain", Providers: []string{"zplain"}})
	cfg.SetRule("/work", "plain")
	if k := m.startKindIn("/elsewhere"); k != "claude" {
		t.Fatalf("outside the rule, new sessions run %q", k)
	}
	if k := m.startKindIn(filepath.Join("/work", "repo")); k != "zplain" {
		t.Fatalf("under the rule, new sessions run %q", k)
	}
	m.usePickedProfile("PLAIN")
	if k := m.startKindIn("/elsewhere"); k != "zplain" || m.accts.profile != "plain" {
		t.Fatalf("after #preset plain, new sessions run %q", k)
	}
	m.usePickedProfile("nope")
	if m.accts.profile != "plain" {
		t.Fatal("an unknown profile was taken")
	}
}

// A profile that mixes moves new sessions on once every account of its
// first provider is nearly out; one that stays waits on it.
func TestPickFollowsRoom(t *testing.T) {
	m, _ := accountsModel(t)
	cfg := &m.store.Config
	cfg.SetProfile("", state.Profile{Name: "Default", Providers: []string{"zcodex", "zplain"}, Mix: state.MixMix})
	cfg.SetDefaultProfile("Default")
	if k := m.startKindIn("/x"); k != "zcodex" {
		t.Fatalf("with room, new sessions run %q", k)
	}
	m.quotas["zcodex:a2"] = m.quotas["zcodex:a1"] // both at 99%
	if k := m.startKindIn("/x"); k != "zplain" {
		t.Fatalf("mixing with zcodex out, new sessions run %q", k)
	}
	p := cfg.Default()
	p.Mix = state.MixStay
	cfg.SetProfile(p.Name, p)
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
	cfg.SetProfile("", state.Profile{Name: "Default", Providers: []string{"zcodex", "zplain", "ztaker"}, OnLimit: state.LimitHandoff})
	a := &fleet.Agent{Key: "k1", DisplayName: "fixer", Rush: true, Kind: "zcodex", Profile: "Default", Cwd: "/x"}
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

// alt+w picks the default profile: a provider's own, or one of yours.
func TestProfilePicker(t *testing.T) {
	m, _ := accountsModel(t)
	cfg := &m.store.Config
	cfg.SetProfile("", state.Profile{Name: "work", Providers: []string{"zplain", "zcodex"}})
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
	pick("work")
	if cfg.DefaultProfile != "work" {
		t.Fatalf("default is %q", cfg.DefaultProfile)
	}
	pick("ZCodex only")
	if d := cfg.Default(); d.Name != "zcodex" || !d.Builtin {
		t.Fatalf("default is %+v", d)
	}
}
