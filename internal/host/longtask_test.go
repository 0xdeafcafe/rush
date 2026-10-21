package host

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

func TestAskableSkipsSubagentsAndEnded(t *testing.T) {
	s := &server{}
	s.onTask(event.Background{Tasks: []event.BackgroundTask{
		{ID: "mine", Type: "local_bash"},
		{ID: "theirs", Type: "local_bash", Parent: "a1"},
		{ID: "killed", Type: "local_bash"},
	}})
	// Stopped, it can end without a notice and stay in the list.
	s.onTask(event.TaskUpdated{ID: "killed", Status: "killed"})
	s.onTask(event.TaskUpdated{ID: "mine", Status: "running"})
	got := s.askable()
	if len(got) != 1 || got[0].ID != "mine" {
		t.Fatalf("askable: %+v", got)
	}
}

func TestOverdueTasks(t *testing.T) {
	now := time.Now()
	tasks := []Task{
		{ID: "a", Label: "wait for queue 40", Type: "monitor", StartedAt: now.Add(-150 * time.Minute)},
		{ID: "b", Label: "fresh", StartedAt: now.Add(-5 * time.Minute)},
		{ID: "c", Label: "asked lately", StartedAt: now.Add(-3 * time.Hour)},
	}
	// a is due, and c, long too, comes in the same check-in.
	due := overdueTasks(tasks, map[string]time.Time{"c": now.Add(-10 * time.Minute)}, now)
	if len(due) != 2 || due[0].ID != "a" || due[1].ID != "c" {
		t.Fatalf("due: %+v", due)
	}
	if n := longTaskNote(due, now); !strings.Contains(n, `"wait for queue 40"`) || !strings.Contains(n, "2h30m") {
		t.Fatalf("note: %s", n)
	}
	if due := overdueTasks([]Task{tasks[2]}, map[string]time.Time{"c": now.Add(-2 * time.Hour)}, now); len(due) != 0 {
		t.Fatalf("asked 2h ago, it waits: %+v", due)
	}
	if due := overdueTasks([]Task{tasks[2]}, map[string]time.Time{"c": now.Add(-5 * time.Hour)}, now); len(due) != 1 {
		t.Fatalf("asked over 4h ago, it's asked again: %+v", due)
	}
}

func TestOverdueTasksSkipsSubagents(t *testing.T) {
	now := time.Now()
	tasks := []Task{
		{ID: "a", Type: "local_agent", StartedAt: now.Add(-40 * time.Minute)},
		{ID: "w", Type: "local_workflow", StartedAt: now.Add(-40 * time.Minute)},
	}
	if due := overdueTasks(tasks, nil, now); len(due) != 0 {
		t.Fatalf("subagents are never asked about: %+v", due)
	}
}

func TestOverdueTasksOneCheckInAnHour(t *testing.T) {
	now := time.Now()
	tasks := []Task{
		{ID: "a", Type: "local_bash", StartedAt: now.Add(-31 * time.Minute)},
		{ID: "b", Type: "local_bash", StartedAt: now.Add(-30 * time.Minute)},
	}
	// b crosses 30m a minute after a was asked about: it waits.
	nudged := map[string]time.Time{"a": now.Add(-time.Minute), longTaskLast: now.Add(-time.Minute)}
	if due := overdueTasks(tasks, nudged, now); len(due) != 0 {
		t.Fatalf("a second check-in within the hour: %+v", due)
	}
	// An hour on, both in one.
	later := now.Add(time.Hour)
	if due := overdueTasks(tasks, nudged, later); len(due) != 2 {
		t.Fatalf("both in one check-in: %+v", due)
	}
	// Nothing new to ask about: none.
	if due := overdueTasks(tasks, map[string]time.Time{"a": now, "b": now}, now.Add(time.Minute)); len(due) != 0 {
		t.Fatalf("asked about already: %+v", due)
	}
}
