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
