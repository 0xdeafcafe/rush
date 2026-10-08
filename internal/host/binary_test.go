package host

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// A session keeps the program it was started with across restarts, and
// finds it again by name once that path is gone.
func TestBinaryFor(t *testing.T) {
	prog := filepath.Join(t.TempDir(), "sh")
	if err := os.WriteFile(prog, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := binaryFor(prog, agent.Kind("claude")); got != prog {
		t.Fatalf("kept: got %s", got)
	}
	gone := filepath.Join(t.TempDir(), "old", "sh")
	got := binaryFor(gone, agent.Kind("claude"))
	if got == gone || filepath.Base(got) != "sh" {
		t.Fatalf("gone: got %s, want sh found again", got)
	}
	if got := binaryFor("my-wrapper", agent.Kind("claude")); got != "my-wrapper" {
		t.Fatalf("a bare name is left to PATH: got %s", got)
	}
	missing := filepath.Join(t.TempDir(), "no-such-program-xyz")
	if got := binaryFor(missing, agent.Kind("claude")); got != missing {
		t.Fatalf("nothing to find: got %s, want the path kept for the error", got)
	}
}
