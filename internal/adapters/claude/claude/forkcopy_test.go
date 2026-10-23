package claude

import (
	"os"
	"path/filepath"
	"testing"
)

// A rewound copy gets the file checkpoints of the conversation it was cut
// from, under its own id; the originals stay where they were.
func TestCopyCheckpoints(t *testing.T) {
	a := Account{ConfigDir: t.TempDir()}
	src := filepath.Join(a.ConfigDir, "file-history", "old")
	if err := os.MkdirAll(src, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(src, "abc@v1"), []byte("hello\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := a.CopyCheckpoints("old", "new"); err != nil {
		t.Fatal(err)
	}
	if b, err := os.ReadFile(filepath.Join(a.ConfigDir, "file-history", "new", "abc@v1")); err != nil || string(b) != "hello\n" {
		t.Fatalf("copied %q %v", b, err)
	}
	if _, err := os.Stat(filepath.Join(src, "abc@v1")); err != nil {
		t.Fatal("the original's checkpoint is kept")
	}
	// Twice is fine, and a conversation with none has nothing to copy.
	if err := a.CopyCheckpoints("old", "new"); err != nil {
		t.Fatal(err)
	}
	if err := a.CopyCheckpoints("none", "other"); err != nil {
		t.Fatal(err)
	}
}

// Entering a worktree moves the transcript to that folder's project; it's
// found there by its session id.
func TestFindTranscriptAfterAMove(t *testing.T) {
	a := Account{ConfigDir: t.TempDir()}
	if got, want := a.FindTranscript("/repo", "sid"), a.TranscriptPath("/repo", "sid"); got != want {
		t.Fatalf("nowhere yet: %s, want %s", got, want)
	}
	moved := a.TranscriptPath("/repo/.claude/worktrees/x", "sid")
	if err := os.MkdirAll(filepath.Dir(moved), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(moved, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if got := a.FindTranscript("/repo", "sid"); got != moved {
		t.Fatalf("moved: %s, want %s", got, moved)
	}
}

// The search keeps each project's listing until the folder changes: a
// transcript written after a search, in a folder it listed or a new one,
// is found by the next, and one removed isn't.
func TestFindTranscriptSeesChanges(t *testing.T) {
	a := Account{ConfigDir: t.TempDir()}
	write := func(p string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, nil, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write(a.TranscriptPath("/other", "x"))
	home := a.TranscriptPath("/repo", "sid")
	if got := a.FindTranscript("/repo", "sid"); got != home {
		t.Fatalf("nowhere yet: %s", got)
	}
	listed := a.TranscriptPath("/other", "sid")
	write(listed)
	if got := a.FindTranscript("/repo", "sid"); got != listed {
		t.Fatalf("in a listed folder: %s, want %s", got, listed)
	}
	if err := os.Remove(listed); err != nil {
		t.Fatal(err)
	}
	if got := a.FindTranscript("/repo", "sid"); got != home {
		t.Fatalf("removed: %s", got)
	}
	fresh := a.TranscriptPath("/new", "sid")
	write(fresh)
	if got := a.FindTranscript("/repo", "sid"); got != fresh {
		t.Fatalf("in a new folder: %s, want %s", got, fresh)
	}
}
