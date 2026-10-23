package ui

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A session that moved into a worktree keeps its transcript where Claude
// Code started it. When its host wakes, the history before the wake must
// be read from there, not from the folder it moved to, which has none.
func TestWokenHostFindsTranscriptWhereItStarted(t *testing.T) {
	profile := t.TempDir()
	started, moved := t.TempDir(), t.TempDir()
	acct := claude.Account{ConfigDir: profile}
	want := acct.TranscriptPath(started, "sid-1")
	if err := os.MkdirAll(filepath.Dir(want), 0o700); err != nil {
		t.Fatal(err)
	}
	line := `{"type":"user","timestamp":"2026-10-01T00:00:01Z","uuid":"u1","message":{"role":"user","content":"earlier question"}}` + "\n"
	if err := os.WriteFile(want, []byte(line), 0o600); err != nil {
		t.Fatal(err)
	}
	info := host.Info{Kind: "claude", SessionID: "sid-1", Cwd: moved, StartedAt: time.Date(2026, 10, 1, 0, 0, 5, 0, time.UTC)}
	pre, path := preFor(info, nil, host.Config{Resume: true}, nil, "", agent.Profile{Kind: "claude", Dir: profile})
	if path != want || pre.path != want {
		t.Fatalf("transcript read from %q, want %q", path, want)
	}
	if s := pre.tail(); len(s.Turns) != 1 || s.Turns[0].Prompt != "earlier question" {
		t.Fatalf("history before the wake missing: %d turns", len(s.Turns))
	}
}
