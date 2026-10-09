package claude

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// A live session's subagent transcript quiet past RunStale is looked at
// every quietEvery, not every scan; the session's own, every scan.
func TestScannerSkipsQuietSubagents(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	dir := t.TempDir()
	main := filepath.Join(dir, "s.jsonl")
	subs := filepath.Join(dir, "s", "subagents")
	_ = os.MkdirAll(subs, 0o755)
	sub := filepath.Join(subs, "agent-a1.jsonl")
	_ = os.WriteFile(main, []byte("{}\n"), 0o644)
	_ = os.WriteFile(sub, []byte("{}\n"), 0o644)
	s := Adapter{}.SpendScanner().(*scanner)
	targets := []agent.SpendTarget{{Key: "k", Path: main, Live: true}}
	s.Run(targets)
	if _, ok := s.sizes[sub]; !ok {
		t.Fatal("the subagent's transcript wasn't read: this test wants it to be")
	}
	grow := func(p string) {
		f, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0)
		_, _ = f.WriteString("{}\n")
		f.Close()
	}
	s.grew[sub] = time.Now().Add(-claude.RunStale - time.Minute)
	was := s.sizes[sub]
	grow(sub)
	grow(main)
	s.Run(targets)
	if s.sizes[sub] != was {
		t.Fatal("a quiet subagent was looked at before quietEvery")
	}
	if st, _ := os.Stat(main); s.sizes[main] != st.Size() {
		t.Fatal("the session's own transcript wasn't looked at")
	}
	s.quietAt = time.Time{}
	s.Run(targets)
	if st, _ := os.Stat(sub); s.sizes[sub] != st.Size() {
		t.Fatal("a quiet subagent wasn't looked at after quietEvery")
	}
}

// A subagent's write left unread while its session was live is read once
// the session isn't, though the session's own transcript didn't change.
func TestScannerCatchesUpDeferred(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	dir := t.TempDir()
	main := filepath.Join(dir, "s.jsonl")
	subs := filepath.Join(dir, "s", "subagents")
	_ = os.MkdirAll(subs, 0o755)
	sub := filepath.Join(subs, "agent-a1.jsonl")
	_ = os.WriteFile(main, []byte("{}\n"), 0o644)
	_ = os.WriteFile(sub, []byte("{}\n"), 0o644)
	s := Adapter{}.SpendScanner().(*scanner)
	live := []agent.SpendTarget{{Key: "k", Path: main, Live: true}}
	s.Run(live)
	s.grew[sub] = time.Now().Add(-claude.RunStale - time.Minute)
	f, _ := os.OpenFile(sub, os.O_APPEND|os.O_WRONLY, 0)
	_, _ = f.WriteString("{}\n")
	f.Close()
	s.Run(live) // deferred
	s.Run([]agent.SpendTarget{{Key: "k", Path: main}})
	if st, _ := os.Stat(sub); s.sizes[sub] != st.Size() {
		t.Fatal("the deferred write was never read")
	}
}
