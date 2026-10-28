package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func TestCareReason(t *testing.T) {
	const g = 1 << 30
	for _, c := range []struct {
		name          string
		free          uint64
		diskGB        int
		pressed       time.Duration
		ramMin        int
		wantSomething bool
	}{
		{"plenty of disk, calm memory", 100 * g, 0, 0, 0, false},
		{"under the default 20G", 12 * g, 0, 0, 0, true},
		{"under 20G but set to 10G", 12 * g, 10, 0, 0, false},
		{"disk watch off", 1 * g, -1, 0, 0, false},
		{"pressed past the default 15m", 100 * g, 0, 16 * time.Minute, 0, true},
		{"pressed, not long enough", 100 * g, 0, 10 * time.Minute, 0, false},
		{"pressed 6m with 5m set", 100 * g, 0, 6 * time.Minute, 5, true},
		{"memory watch off", 100 * g, 0, time.Hour, -1, false},
	} {
		got := careReason(c.free, 500*g, c.diskGB, c.pressed, c.ramMin)
		if (got != "") != c.wantSomething {
			t.Errorf("%s: careReason = %q", c.name, got)
		}
	}
	if got := careReason(1, 0, 0, 0, 0); got != "" {
		t.Errorf("an unread disk is no reason: %q", got)
	}
}

func TestOffPick(t *testing.T) {
	for _, n := range []int{-1, 0, 10, 50} {
		if got := offValue(offPick(n, "%dG"), "%dG"); got != n {
			t.Errorf("offValue(offPick(%d)) = %d", n, got)
		}
	}
}

func TestCarePrompt(t *testing.T) {
	p := carePrompt("the disk has 9G free", []string{"Disk: 9G free of 500G."}, nil)
	if !strings.Contains(p, "the disk has 9G free") || !strings.Contains(p, "Disk: 9G free of 500G.") {
		t.Fatalf("the reason and what rush knows are missing:\n%s", p)
	}
	if strings.Contains(p, "Haven") {
		t.Fatalf("Haven without a report from it:\n%s", p)
	}
	// Each plugin's report and rules go in, under its title.
	reports := []plugin.CleanupReport{{Title: "Haven", Report: "* wt1", Rules: "- never drops databases"}, {Title: "go-clean", Report: "- build cache · 4.0G"}}
	p = carePrompt("x", nil, reports)
	for _, want := range []string{"Haven, as its plugin says:\n- never drops databases", "What Haven reports:\n* wt1", "What go-clean reports:\n- build cache · 4.0G"} {
		if !strings.Contains(p, want) {
			t.Fatalf("missing %q:\n%s", want, p)
		}
	}
}

// With the cleanup plugin off, the sheet recommends turning it on and
// names who reports to it.
func TestCareSheetOff(t *testing.T) {
	s := &careSheet{why: "the disk has 9G free", cur: 1, off: true, from: []string{"go-clean", "haven"}}
	out := strings.Join(s.body(&Model{}, 100, 40), "\n")
	for _, want := range []string{"Turn on the cleanup plugin", "recommended", "Not now"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	if strings.Contains(out, "Start a cleanup agent") {
		t.Errorf("offers an agent with the plugin off:\n%s", out)
	}
	if got := s.reads(); got != "your worktrees, what agents left in /tmp and what go-clean and haven report" {
		t.Errorf("reads = %q", got)
	}
}

func TestCareSheetNotNowStartsNothing(t *testing.T) {
	m := &Model{}
	s := &careSheet{why: "the disk has 9G free", cur: 1}
	m.sheet = s
	if cmd := s.pick(m); cmd != nil || m.sheet != nil || len(m.care.agents) != 0 {
		t.Fatal("Not now must close the sheet and start nothing")
	}
}

func TestCareSheetBody(t *testing.T) {
	s := &careSheet{why: "the disk has 9G free, under the 20G you set", cur: 1}
	out := strings.Join(s.body(&Model{}, 100, 40), "\n")
	for _, want := range []string{"Start a cleanup agent", "Not now", "9G free"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
}

func TestAutoMode(t *testing.T) {
	if got := autoMode("claude"); got != "auto" {
		t.Errorf("Claude Code's auto mode = %q", got)
	}
	if got := autoMode("no-such-agent"); got != "" {
		t.Errorf("an unknown agent has no auto mode, got %q", got)
	}
}

// The cleanup agent's row says it's rush's, and why it runs, in place of
// the home folder it works in.
func TestStartedByLine(t *testing.T) {
	got := ansi.Strip(startedBy(&fleet.Agent{StartedBy: "cleanup", Why: "18G free on disk"}))
	if got != "rush · cleanup · 18G free on disk" {
		t.Errorf("cleanup = %q", got)
	}
	if got := ansi.Strip(startedBy(&fleet.Agent{StartedBy: "kanban"})); got != "kanban" {
		t.Errorf("a plugin you installed = %q", got)
	}
	if got := startedBy(&fleet.Agent{}); got != "" {
		t.Errorf("a session you started = %q", got)
	}
}
