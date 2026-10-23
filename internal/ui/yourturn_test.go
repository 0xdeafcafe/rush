package ui

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A halt needs you; a turn that ended without asking is Finished, not Needs
// you; the idle and what stopped a moment ago are Idle; a stopped agent moves to Today once it's been still a while.
func TestFinishedTurnsWaitForYou(t *testing.T) {
	now := time.Now()
	agent := func(key, st string) *fleet.Agent {
		a := &fleet.Agent{Key: key, DisplayName: key, PID: 7}
		a.State, a.UpdatedAt = st, now.Add(-5*time.Minute)
		return a
	}
	quiet, broke, idle := agent("quiet", "done"), agent("broke", "done"), agent("idle", "done")
	broke.Spend.Halt = &claude.Halt{Kind: "server_error", Text: "API Error: Response stalled mid-stream."}
	idle.Seen = true
	rested, old := agent("rested", "done"), agent("old", "done")
	rested.PID, rested.Seen, rested.UpdatedAt = 0, true, now.Add(-3*time.Minute)
	old.PID, old.Seen, old.UpdatedAt = 0, true, now.Add(-40*time.Minute)
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 120, h: 40, lastState: map[string]string{}}
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{quiet, broke, idle, rested, old}}
	m.rebuild()
	want := map[string]string{"quiet": finishedSection, "broke": needsSection, "idle": idleSection, "rested": idleSection, "old": justLeftSection}
	for k, g := range want {
		if m.groupOf[k] != g {
			t.Errorf("%s in %q, want %q", k, m.groupOf[k], g)
		}
	}
	var titles []string
	for _, l := range m.lines {
		if l.kind == lineSection {
			titles = append(titles, l.title)
		}
	}
	if len(titles) != 4 || titles[0] != needsSection || titles[1] != finishedSection || titles[2] != idleSection || titles[3] != justLeftSection {
		t.Fatalf("sections %v", titles)
	}
	stuck := agent("stuck", "working")
	stuck.UpdatedAt = now.Add(-fleet.StuckAfter - time.Minute)
	m.snap.Agents = append(m.snap.Agents, stuck)
	if m.rebuild(); m.groupOf["stuck"] != stuckSection {
		t.Errorf("silent at work is in %q, want %q", m.groupOf["stuck"], stuckSection)
	}
	m.store.Config.ActiveMinutes = -1
	if m.rebuild(); m.groupOf["rested"] != justLeftSection {
		t.Errorf("with it off, a stopped agent is in %q", m.groupOf["rested"])
	}
	m.store.Config.JustLeftCount = 1 // the most recent stays; the rest go on to Today
	if m.rebuild(); m.groupOf["rested"] != justLeftSection || m.groupOf["old"] != "Today" {
		t.Errorf("count 1: rested in %q, old in %q", m.groupOf["rested"], m.groupOf["old"])
	}
	m.store.Config.JustLeftMinutes = -1
	if m.rebuild(); m.groupOf["rested"] != "Today" {
		t.Errorf("Just left off: rested in %q", m.groupOf["rested"])
	}
	if s, _, _ := m.rowSummary(broke); s != "stopped · Response stalled mid-stream." {
		t.Fatalf("summary %q", s)
	}
}
