package gate

import (
	"context"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// Under critical memory pressure the newest run going is paused, its
// processes stopped, and goes on once pressure is back to normal; it
// counts as waiting meanwhile, and its agent is told why.
func TestWatchPausesTheNewest(t *testing.T) {
	if testing.Short() {
		t.Skip("takes a few of the watchdog's ticks")
	}
	t.Setenv("RUSH_HOME", t.TempDir())
	stubMemory(t, 64*gib)
	var level atomic.Int32
	level.Store(PressureCritical)
	old := PressureLevel
	PressureLevel = func() int { return int(level.Load()) }
	t.Cleanup(func() { PressureLevel = old })

	first, err := Acquire(ctx(t), Ask{Name: "tsc", Rule: Rule{Memory: gib, Limit: time.Hour}, Session: "a"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer first.Release()
	time.Sleep(10 * time.Millisecond)
	newest, err := Acquire(ctx(t), Ask{Name: "go", Rule: Rule{Memory: gib, Limit: time.Hour}, Session: "b"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer newest.Release()
	sleep := exec.Command("sleep", "30")
	if err := sleep.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { _ = sleep.Process.Kill(); _ = sleep.Wait() }()
	var mu sync.Mutex
	var told []string
	c, cancel := context.WithCancel(ctx(t))
	defer cancel()
	go newest.Watch(c, Watch{Root: sleep.Process.Pid, Tell: func(s string) { mu.Lock(); told = append(told, s); mu.Unlock() }})

	until := func(what string, ok func() bool) {
		t.Helper()
		for end := time.Now().Add(3 * Tick); !ok(); time.Sleep(100 * time.Millisecond) {
			if time.Now().After(end) {
				t.Fatalf("never %s", what)
			}
		}
	}
	paused := func() bool {
		for _, tk := range Tickets() {
			if tk.Name == "go" {
				return tk.Paused()
			}
		}
		return false
	}
	until("paused", paused)
	if st := procState(sleep.Process.Pid); !strings.HasPrefix(st, "T") {
		t.Fatalf("paused run's process is %q, want stopped", st)
	}
	if !Waiting(os.Getpid()) {
		t.Fatal("a paused run's holder doesn't count as waiting")
	}
	for _, tk := range Tickets() {
		if tk.Name == "tsc" && tk.Paused() {
			t.Fatal("the older run was paused too")
		}
	}
	mu.Lock()
	if len(told) != 1 || !strings.Contains(told[0], "paused") {
		t.Fatalf("told %q", told)
	}
	mu.Unlock()
	level.Store(PressureNormal)
	until("resumed", func() bool { return !paused() })
	if st := procState(sleep.Process.Pid); strings.HasPrefix(st, "T") {
		t.Fatal("resumed run's process is still stopped")
	}
}

// procState is ps's state for pid: T for stopped.
func procState(pid int) string {
	out, _ := exec.Command("ps", "-o", "state=", "-p", strconv.Itoa(pid)).Output()
	return strings.TrimSpace(string(out))
}
