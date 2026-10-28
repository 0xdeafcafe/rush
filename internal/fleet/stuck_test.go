package fleet

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestStuck(t *testing.T) {
	now := time.Now()
	a := &Agent{}
	a.State, a.UpdatedAt = "working", now.Add(-31*time.Minute)
	if !a.Stuck(now) {
		t.Fatal("quiet 31m at work should be stuck")
	}
	a.Subagents = []SubagentTile{{ID: "s", Mod: now.Add(-6 * time.Minute)}}
	if a.Stuck(now) {
		t.Fatal("a subagent writing 6m ago means it is not stuck")
	}
	if got := a.StuckSubs(now); len(got) != 1 {
		t.Fatalf("subagent quiet 6m should be stuck, got %d", len(got))
	}
	a.State = "done"
	a.Subagents = nil
	if a.Stuck(now) {
		t.Fatal("a finished agent is never stuck")
	}
}

func TestStuckSubsRunningACommand(t *testing.T) {
	now := time.Now()
	path := filepath.Join(t.TempDir(), "agent-s.jsonl")
	write := func(lines ...string) {
		if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	call := func(id string) string {
		return `{"type":"assistant","message":{"content":[{"type":"tool_use","id":"` + id + `","name":"Bash","input":{}}]}}`
	}
	result := func(id string) string {
		return `{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"` + id + `","content":"ok"}]}}`
	}
	stuck := func(quiet time.Duration) bool {
		a := &Agent{Subagents: []SubagentTile{{ID: "s", Path: path, Mod: now.Add(-quiet)}}}
		return len(a.StuckSubs(now)) == 1
	}

	write(call("a"))
	if stuck(6 * time.Minute) {
		t.Fatal("a subagent 6m into a command is not stuck")
	}
	if !stuck(31 * time.Minute) {
		t.Fatal("a subagent 31m into a command is stuck")
	}
	write(call("a"), call("b"), result("a"))
	if stuck(7 * time.Minute) {
		t.Fatal("one of two commands still running is not stuck")
	}
	write(call("a"), result("a"))
	if !stuck(8 * time.Minute) {
		t.Fatal("a subagent waiting on the model 8m is stuck")
	}
}

// One its host marked stalled is stuck at once, not after StuckAfter.
func TestStalledIsStuck(t *testing.T) {
	now := time.Now()
	a := &Agent{}
	a.State, a.UpdatedAt = "working", now.Add(-6*time.Minute)
	if a.Stuck(now) {
		t.Fatal("stuck after 6m without a stall")
	}
	a.Stalled = now.Add(-6 * time.Minute)
	if !a.Stuck(now) {
		t.Fatal("a stalled agent at work isn't stuck")
	}
	a.State = "done"
	if a.Stuck(now) {
		t.Fatal("a finished agent is stuck")
	}
}
