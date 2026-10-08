package ui

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Done work goes once it's been done and untouched for the wait; never while
// it runs, never when it isn't done, never when the tidy-up is off.
func TestCleanupDue(t *testing.T) {
	now := time.Now()
	st := &state.Store{}
	st.Overlay.Done = map[string]time.Time{}
	m := &Model{store: st, snap: &fleet.Snapshot{At: now}}
	add := func(key string, done time.Duration, active time.Duration, pid int) {
		a := &fleet.Agent{Key: key, PID: pid}
		a.UpdatedAt = now.Add(-active)
		if done >= 0 {
			a.Done = true
			st.Overlay.Done[key] = now.Add(-done)
		}
		m.snap.Agents = append(m.snap.Agents, a)
	}
	add("old", 5*time.Hour, 6*time.Hour, 0)
	add("recent", time.Hour, 2*time.Hour, 0)
	add("running", 5*time.Hour, 6*time.Hour, 42)
	add("notdone", -1, 9*time.Hour, 0)
	add("touched", 5*time.Hour, 30*time.Minute, 0) // done long ago, but worked on since

	if d := m.dueIn([]string{"old"}, now); d != 0 {
		t.Errorf("old: %v", d)
	}
	if d := m.dueIn([]string{"recent"}, now); d < 110*time.Minute || d > 2*time.Hour {
		t.Errorf("recent: %v, want about 2h", d)
	}
	if d := m.dueIn([]string{"touched"}, now); d < 2*time.Hour {
		t.Errorf("touched: %v, want 2h30m", d)
	}
	for _, k := range []string{"running", "notdone"} {
		if d := m.dueIn([]string{k}, now); d >= 0 {
			t.Errorf("%s: %v, should never be due", k, d)
		}
	}
	if d := m.dueIn([]string{"old", "recent"}, now); d <= 0 {
		t.Errorf("a worktree shared with a recent one waits for it: %v", d)
	}
	st.Config.CleanupHours = -1
	if d := m.dueIn([]string{"old"}, now); d >= 0 {
		t.Errorf("off: %v", d)
	}
}

// Clean up ticks only what's safe; a ticks all that's safe, then none; one
// with an agent running in it can't be ticked.
func TestCleanSheet(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now, Agents: []*fleet.Agent{{Key: "run", PID: 7}}}}
	m.clean.checked = now
	m.clean.wts = []fleet.Worktree{
		{Path: "/r/.claude/worktrees/safe", Repo: "/r", Checked: now, Size: 10},
		{Path: "/r/.claude/worktrees/dirty", Repo: "/r", Checked: now, Changed: 2},
		{Path: "/r/.claude/worktrees/busy", Repo: "/r", Checked: now, Agents: []string{"run"}},
	}
	m.clean.tmp = fleet.Scratch{StaleItems: 3, Stale: 5}
	m.openCleanSheet()
	s, _ := m.sheet.(*cleanSheet)
	if s == nil {
		t.Fatal("no sheet")
	}
	on := func() (out []string) {
		for _, it := range s.items {
			if it.on {
				name := "/tmp"
				if it.wt != nil {
					name = it.wt.Path[len("/r/.claude/worktrees/"):]
				}
				out = append(out, name)
			}
		}
		return out
	}
	if got := on(); len(got) != 2 || got[0] != "safe" || got[1] != "/tmp" {
		t.Fatalf("ticked %v", got)
	}
	if n, losing, size := s.ticked(m); n != 2 || losing != 0 || size != 15 {
		t.Fatalf("ticked %d, losing %d, %d", n, losing, size)
	}
	s.key(m, tea.KeyPressMsg{}, "a")
	if got := on(); len(got) != 0 {
		t.Fatalf("a with all safe ticked should untick: %v", got)
	}
	for i, it := range s.items {
		s.cur = i
		s.key(m, tea.KeyPressMsg{}, "space")
		if it.busy != "" && s.items[i].on {
			t.Fatal("ticked one an agent runs in")
		}
	}
	if _, losing, _ := s.ticked(m); losing != 1 {
		t.Fatalf("dirty ticked by hand should count as losing work: %d", losing)
	}
}

// An orphan is ended once it has been one for orphanGrace, not before, and
// never with KeepOrphans.
func TestDueOrphans(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now}}
	m.snap.Machine.Rows = []fleet.ProcRow{{PID: 7, Role: fleet.RoleOrphan, Start: now.Add(-time.Hour)}, {PID: 8, Role: fleet.RoleWorker}}
	if due := m.dueOrphans(now); len(due) != 0 {
		t.Fatalf("ended as soon as seen: %v", due)
	}
	if due := m.dueOrphans(now.Add(orphanGrace)); len(due) != 1 || due[0].pid != 7 {
		t.Fatalf("after the grace: %v", due)
	}
	m.store.Config.KeepOrphans = true
	m.clean.orphans = nil
	m.dueOrphans(now)
	if due := m.dueOrphans(now.Add(time.Hour)); len(due) != 0 {
		t.Fatalf("kept orphans were ended: %v", due)
	}
}

// What agents left outside their projects comes ticked only when it's
// untouched scratch in a temp folder of a finished agent; the rest waits for
// a hand, and what's held back can't be ticked.
func TestCleanSheetLeftOutside(t *testing.T) {
	now := time.Now()
	done, run := &fleet.Agent{Key: "done"}, &fleet.Agent{Key: "run", PID: 7}
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now, Agents: []*fleet.Agent{done, run}}}
	m.clean.checked = now
	m.clean.outside = []fleet.LeftItem{
		{Path: "/tmp/clone", Agent: done, Temp: true, Size: 4},
		{Path: "/opt/made", Agent: done},
		{Path: "/tmp/old", Agent: done, Temp: true, Before: true},
		{Path: "/tmp/busy", Agent: run, Temp: true},
		{Path: "/tmp/gone", Agent: done, Temp: true, Gone: true},
	}
	m.openCleanSheet()
	s, ok := m.sheet.(*cleanSheet)
	if !ok || len(s.items) != 4 {
		t.Fatalf("items: %+v", m.sheet)
	}
	for i, want := range []struct {
		on   bool
		busy bool
	}{{true, false}, {false, false}, {false, true}, {false, true}} {
		if it := s.items[i]; it.on != want.on || (it.busy != "") != want.busy {
			t.Errorf("%s: on %v busy %q", it.left.Path, it.on, it.busy)
		}
	}
}
