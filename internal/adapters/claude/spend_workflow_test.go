package claude

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// The agents a session's workflows ran are its spend too: their
// transcripts, one folder per run under subagents/workflows, are priced
// with the session's, and a run that starts later is noticed.
func TestScannerPricesWorkflowAgents(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	t.Setenv("RUSH_CACHE", t.TempDir())
	line := func(id string) string {
		return `{"type":"assistant","timestamp":"2026-10-29T10:00:00Z","message":{"model":"claude-opus-4-1","id":"` + id + `","role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1000,"output_tokens":1000}}}` + "\n"
	}
	dir := t.TempDir()
	main := filepath.Join(dir, "s.jsonl")
	_ = os.WriteFile(main, []byte(line("m1")), 0o644)
	run := filepath.Join(dir, "s", "subagents", "workflows", "wf_1")
	_ = os.MkdirAll(run, 0o755)
	_ = os.WriteFile(filepath.Join(run, "agent-a.jsonl"), []byte(line("w1")), 0o644)

	targets := []agent.SpendTarget{{Key: "k", Path: main}}
	alone := Adapter{}.SpendScanner().(*scanner).Run(targets)["k"].Cost
	s := Adapter{}.SpendScanner().(*scanner)
	_ = os.Remove(filepath.Join(run, "agent-a.jsonl"))
	own := s.Run(targets)["k"].Cost
	if own <= 0 || alone < 1.99*own {
		t.Fatalf("with a workflow agent the session cost %v, alone %v: want it twice", alone, own)
	}

	// A second run starts after the first scan: its folder is new.
	run2 := filepath.Join(dir, "s", "subagents", "workflows", "wf_2")
	_ = os.MkdirAll(run2, 0o755)
	_ = os.WriteFile(filepath.Join(run2, "agent-b.jsonl"), []byte(line("w2")), 0o644)
	if got := s.Run(targets)["k"].Cost; got < 1.99*own {
		t.Fatalf("a workflow run started later wasn't priced: %v, want about %v", got, 2*own)
	}
}
