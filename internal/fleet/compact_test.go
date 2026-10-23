package fleet

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/proc"
)

func TestCompactions(t *testing.T) {
	t.Setenv("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "")
	cfg, cwd := t.TempDir(), t.TempDir()
	os.WriteFile(filepath.Join(cfg, "settings.json"), []byte(`{"env":{"CLAUDE_CODE_AUTO_COMPACT_WINDOW":"400000"}}`), 0o600)
	os.MkdirAll(filepath.Join(cwd, ".claude"), 0o700)
	a := &Agent{Key: "k", Kind: "claude", Past: true, Acct: agent.Profile{Kind: "claude", Dir: cfg}}
	a.Cwd = cwd
	l := &Loader{}
	now := time.Now()
	l.compactions([]*Agent{a}, nil, now)
	if a.Compaction != (agent.Compaction{Window: 400_000, Headroom: 33_000}) {
		t.Fatalf("from your settings: %+v", a.Compaction)
	}
	// The project's just-yours file wins, once the last reading is old.
	os.WriteFile(filepath.Join(cwd, ".claude", "settings.local.json"), []byte(`{"env":{"CLAUDE_CODE_AUTO_COMPACT_WINDOW":"300000"}}`), 0o600)
	b := &Agent{Key: "k", Kind: "claude", Past: true, Acct: a.Acct}
	b.Cwd = cwd
	l.compactions([]*Agent{b}, nil, now.Add(time.Second))
	if b.Compaction.Window != 400_000 {
		t.Fatalf("kept for a while: %+v", b.Compaction)
	}
	l.compactions([]*Agent{b}, nil, now.Add(compactEvery))
	if b.Compaction.Window != 300_000 {
		t.Fatalf("read again: %+v", b.Compaction)
	}
	l.compactions(nil, nil, now)
	if len(l.compact) != 0 {
		t.Fatal("a row no longer listed keeps nothing")
	}
}

// A process's environment is read once while it's the same process: a
// new one on the same pid, started later, is read afresh.
func TestEnvOfReadOncePerProcess(t *testing.T) {
	if proc.Env(os.Getpid()) == nil {
		t.Skip("no process environments here")
	}
	pid, start := os.Getpid(), time.Now().Add(-time.Minute)
	l := &Loader{prevTab: &proc.Table{Procs: map[int]*proc.Proc{pid: {PID: pid, Start: start}}}}
	if l.envOf(pid) == nil {
		t.Fatal("not read")
	}
	l.envs[pid] = procEnv{start: start, env: []string{"KEPT=1"}}
	if got := l.envOf(pid); len(got) != 1 || got[0] != "KEPT=1" {
		t.Fatalf("the same process read again: %d vars", len(got))
	}
	l.prevTab.Procs[pid].Start = start.Add(time.Second)
	if got := l.envOf(pid); len(got) == 1 && got[0] == "KEPT=1" {
		t.Fatal("another process on the pid was given the first's environment")
	}
}
