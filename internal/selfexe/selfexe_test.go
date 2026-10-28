package selfexe

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// A rush removed by go install and written again a moment later is waited
// for, not failed on.
func TestWaitOutlastsAnInstall(t *testing.T) {
	exe := filepath.Join(t.TempDir(), "rush")
	go func() {
		time.Sleep(200 * time.Millisecond)
		_ = os.WriteFile(exe, []byte("new"), 0o755)
	}()
	if err := Wait(exe); err != nil {
		t.Fatalf("Wait: %v", err)
	}
	if _, err := os.Stat(exe); err != nil {
		t.Fatalf("returned before the file was back: %v", err)
	}
}

func TestWaitGivesUpOnAMissingRush(t *testing.T) {
	old := Settle
	Settle = 100 * time.Millisecond
	t.Cleanup(func() { Settle = old })
	if err := Wait(filepath.Join(t.TempDir(), "rush")); err == nil {
		t.Fatal("Wait on a rush that never comes: want an error")
	}
}

func TestWaitIsInstantOnASettledRush(t *testing.T) {
	exe := filepath.Join(t.TempDir(), "rush")
	if err := os.WriteFile(exe, []byte("old"), 0o755); err != nil {
		t.Fatal(err)
	}
	old := time.Now().Add(-time.Hour)
	if err := os.Chtimes(exe, old, old); err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	if err := Wait(exe); err != nil {
		t.Fatal(err)
	}
	if d := time.Since(start); d > 20*time.Millisecond {
		t.Fatalf("a settled rush took %v", d)
	}
}
