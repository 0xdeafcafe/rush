package host

import (
	"os"
	"path/filepath"
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
	due := overdueTasks(tasks, nil, map[string]time.Time{"c": now.Add(-10 * time.Minute)}, now)
	if len(due) != 2 || due[0].ID != "a" || due[1].ID != "c" {
		t.Fatalf("due: %+v", due)
	}
	if n := longTaskNote(due, nil, now); !strings.Contains(n, `"wait for queue 40"`) || !strings.Contains(n, "2h30m") {
		t.Fatalf("note: %s", n)
	}
	if due := overdueTasks([]Task{tasks[2]}, nil, map[string]time.Time{"c": now.Add(-2 * time.Hour)}, now); len(due) != 0 {
		t.Fatalf("asked 2h ago, it waits: %+v", due)
	}
	if due := overdueTasks([]Task{tasks[2]}, nil, map[string]time.Time{"c": now.Add(-5 * time.Hour)}, now); len(due) != 1 {
		t.Fatalf("asked over 4h ago, it's asked again: %+v", due)
	}
}

func TestOverdueTasksSkipsSubagents(t *testing.T) {
	now := time.Now()
	tasks := []Task{
		{ID: "a", Type: "local_agent", StartedAt: now.Add(-40 * time.Minute)},
		{ID: "w", Type: "local_workflow", StartedAt: now.Add(-40 * time.Minute)},
	}
	if due := overdueTasks(tasks, nil, nil, now); len(due) != 0 {
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
	if due := overdueTasks(tasks, nil, nudged, now); len(due) != 0 {
		t.Fatalf("a second check-in within the hour: %+v", due)
	}
	// An hour on, both in one.
	later := now.Add(time.Hour)
	if due := overdueTasks(tasks, nil, nudged, later); len(due) != 2 {
		t.Fatalf("both in one check-in: %+v", due)
	}
	// Nothing new to ask about: none.
	if due := overdueTasks(tasks, nil, map[string]time.Time{"a": now, "b": now}, now.Add(time.Minute)); len(due) != 0 {
		t.Fatalf("asked about already: %+v", due)
	}
}

func TestOverdueTasksJudgedByOutput(t *testing.T) {
	now := time.Now()
	tasks := []Task{
		{ID: "busy", Type: "local_bash", StartedAt: now.Add(-50 * time.Minute)},
		{ID: "quiet", Type: "local_bash", StartedAt: now.Add(-50 * time.Minute)},
	}
	wrote := map[string]time.Time{"busy": now.Add(-2 * time.Minute), "quiet": now.Add(-40 * time.Minute)}
	due := overdueTasks(tasks, wrote, nil, now)
	if len(due) != 1 || due[0].ID != "quiet" {
		t.Fatalf("only the quiet one: %+v", due)
	}
	if n := longTaskNote(due, wrote, now); !strings.Contains(n, "last wrote 40m0s ago") {
		t.Fatalf("note: %s", n)
	}
	// Still writing past longTaskAnyway, it's asked about anyway.
	tasks[0].StartedAt = now.Add(-5 * time.Hour)
	if due := overdueTasks(tasks[:1], wrote, nil, now); len(due) != 1 {
		t.Fatalf("a chatty task running 5h: %+v", due)
	}
}

func TestLastOutput(t *testing.T) {
	dir := t.TempDir()
	at := time.Now().Add(-time.Hour).Truncate(time.Second)
	f := filepath.Join(dir, "b1.output")
	if err := os.WriteFile(f, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(f, at, at); err != nil {
		t.Fatal(err)
	}
	got := lastOutput([]string{dir, t.TempDir()}, []string{"b1", "gone"})
	if !got["b1"].Equal(at) || len(got) != 1 {
		t.Fatalf("lastOutput: %v", got)
	}
}
