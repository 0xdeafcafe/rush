//go:build linux

package proc

import (
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

// A grandchild that setsids away from a shell that then exits is handed
// to this process, not init, and reaped once it exits; a child of this
// process's own is still waited on by its exec.Cmd.
func TestSubreap(t *testing.T) {
	if err := Subreap(); err != nil {
		t.Skip(err)
	}
	pidFile := filepath.Join(t.TempDir(), "pid")
	sh := exec.Command("/bin/sh", "-c", `setsid /bin/sh -c 'echo $$ > "$1.tmp"; mv "$1.tmp" "$1"; exec sleep 1' sh "$1" </dev/null >/dev/null 2>&1 &`, "sh", pidFile)
	if err := sh.Run(); err != nil {
		t.Fatal(err)
	}
	var pid int
	for deadline := time.Now().Add(5 * time.Second); pid == 0; {
		if b, err := os.ReadFile(pidFile); err == nil {
			pid, _ = strconv.Atoi(strings.TrimSpace(string(b)))
		}
		if time.Now().After(deadline) {
			t.Fatal("the grandchild never started")
		}
		time.Sleep(10 * time.Millisecond)
	}
	s, ok := readStat(pid)
	if !ok {
		t.Fatal("the grandchild is gone already")
	}
	if s.ppid != os.Getpid() {
		t.Fatalf("grandchild's parent is %d, want this process %d", s.ppid, os.Getpid())
	}

	own := exec.Command("/bin/sh", "-c", "exit 3")
	if err := own.Start(); err != nil {
		t.Fatal(err)
	}

	for deadline := time.Now().Add(5 * time.Second); ; {
		if _, err := os.Stat("/proc/" + strconv.Itoa(pid)); os.IsNotExist(err) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("grandchild %d was not reaped (zombie: %v)", pid, Zombie(pid))
		}
		time.Sleep(50 * time.Millisecond)
	}

	// By now the own child has long exited and a SIGCHLD has passed.
	err := own.Wait()
	if ee, ok := err.(*exec.ExitError); !ok || ee.ExitCode() != 3 {
		t.Fatalf("own child's exit was taken: %v", err)
	}
}
