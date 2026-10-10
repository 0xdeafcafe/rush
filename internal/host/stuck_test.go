package host

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/proc"
)

// A shell whose tree uses no CPU for stuckAfter is hung, and stopped; one
// at work isn't.
func TestStuckShellIsStopped(t *testing.T) {
	idle := exec.Command("sh", "-c", "sleep 60; true")
	busy := exec.Command("sh", "-c", "while :; do :; done")
	for _, c := range []*exec.Cmd{idle, busy} {
		if err := c.Start(); err != nil {
			t.Fatal(err)
		}
		defer c.Process.Kill()
	}
	time.Sleep(300 * time.Millisecond) // both settled into what they do
	shells := []int{idle.Process.Pid, busy.Process.Pid}
	seen := map[int]treeUse{}
	now := time.Now()
	if got := idleShells(proc.Snapshot(nil), shells, seen, now); len(got) != 0 {
		t.Fatalf("hung at first look: %v", got)
	}
	time.Sleep(300 * time.Millisecond)
	tab := proc.Snapshot(nil)
	got := idleShells(tab, shells, seen, now.Add(stuckAfter))
	if len(got) != 1 || got[0] != idle.Process.Pid {
		t.Fatalf("hung shells %v, want only the idle one %d", got, idle.Process.Pid)
	}
	stopTree(tab, got[0])
	done := make(chan error, 1)
	go func() { done <- idle.Wait() }()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("the hung shell wasn't stopped")
	}
}

// What rush tells the main session reaches it at its next call, as rush's
// word rather than yours.
func TestNoticeIsRushsWord(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, MainNotice), []byte("it hung"), 0o600); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	if err := Inbox(dir, strings.NewReader(`{"hook_event_name":"PostToolUseFailure"}`), &out); err != nil {
		t.Fatal(err)
	}
	if s := out.String(); !strings.Contains(s, "Rush (not the user)") || !strings.Contains(s, "it hung") || strings.Contains(s, "user sent you") {
		t.Fatalf("notice handed over as %s", s)
	}
}
