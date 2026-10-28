package convo

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// A run that writes keeps its place: rows are ordered by when each began.
func TestSubagentsOrderByBirth(t *testing.T) {
	tr := filepath.Join(t.TempDir(), "s.jsonl")
	dir := filepath.Join(filepath.Dir(tr), "s", "subagents")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	base := time.Now().Add(-time.Hour)
	for i, id := range []string{"b", "a"} { // b began first
		meta := filepath.Join(dir, "agent-"+id+".meta.json")
		os.WriteFile(meta, []byte(`{"agentType":"x"}`), 0o644)
		os.WriteFile(filepath.Join(dir, "agent-"+id+".jsonl"), []byte("{}\n"), 0o644)
		os.Chtimes(meta, base.Add(time.Duration(i)*time.Minute), base.Add(time.Duration(i)*time.Minute))
	}
	var l Subagents
	ids := func() string {
		s := ""
		for _, sa := range l.List(tr) {
			s += sa.ID
		}
		return s
	}
	if got := ids(); got != "ba" {
		t.Fatalf("order %q, want ba", got)
	}
	// b writes last now: it stays first.
	os.Chtimes(filepath.Join(dir, "agent-b.jsonl"), time.Now(), time.Now())
	if got := ids(); got != "ba" {
		t.Fatalf("after a write, order %q, want ba", got)
	}
}

// A workflow's agents are runs too, a folder down: each knows its
// workflow and phase, and one the workflow starts later is found.
func TestSubagentsOfWorkflows(t *testing.T) {
	tr := filepath.Join(t.TempDir(), "s.jsonl")
	run := filepath.Join(filepath.Dir(tr), "s", "subagents", "workflows", "wf_1")
	os.MkdirAll(run, 0o755)
	add := func(id string) {
		os.WriteFile(filepath.Join(run, "agent-"+id+".meta.json"), []byte(`{"agentType":"workflow-subagent","description":"port:`+id+`","workflowPhase":"Port"}`), 0o644)
	}
	add("a")
	var l Subagents
	got := l.List(tr)
	if len(got) != 1 || got[0].Workflow != "wf_1" || got[0].Phase != "Port" || got[0].Description != "port:a" || got[0].Path != filepath.Join(run, "agent-a.jsonl") {
		t.Fatalf("runs = %+v", got)
	}
	// Another, added a moment on: the folder's time says so.
	add("b")
	future := time.Now().Add(time.Minute)
	os.Chtimes(run, future, future)
	if got = l.List(tr); len(got) != 2 {
		t.Fatalf("the agent added later isn't found: %+v", got)
	}
}
