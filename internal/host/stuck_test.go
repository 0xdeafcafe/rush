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
// at work isn't, nor one sleeping, which waits on purpose.
func TestStuckShellIsStopped(t *testing.T) {
	never, w, err := os.Pipe() // input that never comes
	if err != nil {
		t.Fatal(err)
	}
	defer never.Close()
	defer w.Close()
	idle := exec.Command("sh", "-c", "read x; true")
	idle.Stdin = never
	busy := exec.Command("sh", "-c", "while :; do :; done")
	waits := exec.Command("sh", "-c", "sleep 60; true")
	for _, c := range []*exec.Cmd{idle, busy, waits} {
		if err := c.Start(); err != nil {
			t.Fatal(err)
		}
		defer c.Process.Kill()
	}
	time.Sleep(300 * time.Millisecond) // all settled into what they do
	shells := []int{idle.Process.Pid, busy.Process.Pid, waits.Process.Pid}
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

// A command rush ran as a script, or under the gate, is named as the agent
// wrote it.
func TestAgentCommand(t *testing.T) {
	sh := filepath.Join(t.TempDir(), "toolu_x.sh")
	if err := os.WriteFile(sh, []byte("cd x\npython3 bench.py\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	line := "'/opt/homebrew/bin/bash' --noprofile --norc -c '__rush_s=$1; . \"$__rush_s\"' rush-script '" + sh + "'"
	if got := agentCommand(line); got != "cd x\npython3 bench.py" {
		t.Fatalf("a script's = %q", got)
	}
	if got := agentCommand("'/r' gate run --name 'go' --dir '/x' -- bash -c 'go test ./...'"); got != "go test ./..." {
		t.Fatalf("a gated one's = %q", got)
	}
	if got := agentCommand("ls -la"); got != "ls -la" {
		t.Fatalf("a plain one's = %q", got)
	}
}
