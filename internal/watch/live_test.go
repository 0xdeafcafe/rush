package watch

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestAWatchKeepsItsTaskLiveUntilItEnds(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Unix(1000, 0)
	var seen []Live
	polls := []PR{pr("a", "lint=failed", "test=pending"), pr("a", "lint=failed", "test=passed")}
	looks := 0
	code := WatchPR(context.Background(), Options{
		Repo: "o/r", Number: 7, Until: UntilDone, Timeout: time.Hour, Task: "bx1",
		Fetch: func(context.Context, string, int, time.Duration) (PR, int, error) {
			p := polls[min(looks, len(polls)-1)]
			looks++
			return p, 4000, nil
		},
		Sleep: func(_ context.Context, d time.Duration) bool {
			if l, ok := LiveFor("bx1", now); ok {
				seen = append(seen, l)
			}
			now = now.Add(d)
			return true
		},
		Logs: func(context.Context, string, int64) ([]byte, error) { return nil, nil },
		Now:  func() time.Time { return now },
	})
	if code != ExitFailed || len(seen) != 1 {
		t.Fatalf("code %d, live while waiting %d times", code, len(seen))
	}
	l := seen[0]
	if l.Number != 7 || l.Failed != 1 || l.Pending != 1 || l.FailedChecks[0] != "lint" || l.Running[0] != "test" || l.Reports() {
		t.Errorf("live as it waited: %+v", l)
	}
	if _, ok := LiveFor("bx1", now); ok {
		t.Error("the record outlived the watch")
	}
}

func TestALiveRecordIsntBelievedPastItsTimeoutOrItsProcess(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	start := time.Unix(1000, 0)
	saveLive(Live{Task: "a", PID: os.Getpid(), Start: start, Timeout: 10 * time.Minute})
	if _, ok := LiveFor("a", start.Add(14*time.Minute)); !ok {
		t.Error("not believed within its grace")
	}
	if _, ok := LiveFor("a", start.Add(16*time.Minute)); ok {
		t.Error("believed past its timeout and grace")
	}
	cmd := exec.Command("true")
	if err := cmd.Run(); err != nil {
		t.Skip(err)
	}
	saveLive(Live{Task: "b", PID: cmd.Process.Pid, Start: start})
	if _, ok := LiveFor("b", start); ok {
		t.Error("believed after its process went")
	}
	if _, ok := LiveFor("../a", start); ok {
		t.Error("a task id that isn't one reads a file")
	}
}

func TestTaskIDIsTheOutputFileClaudeCodeWritesTo(t *testing.T) {
	if os.Getenv("WATCH_TASK_CHILD") != "" {
		os.Stderr.WriteString("task=" + TaskID() + "\n")
		return
	}
	dir := filepath.Join(t.TempDir(), "s", "tasks")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	out, err := os.Create(filepath.Join(dir, "b7xk2q.output"))
	if err != nil {
		t.Fatal(err)
	}
	defer out.Close()
	var stderr strings.Builder
	cmd := exec.Command(os.Args[0], "-test.run=^TestTaskIDIsTheOutputFileClaudeCodeWritesTo$")
	cmd.Env = append(os.Environ(), "WATCH_TASK_CHILD=1")
	cmd.Stdout, cmd.Stderr = out, &stderr
	if err := cmd.Run(); err != nil {
		t.Fatal(err, stderr.String())
	}
	if !strings.Contains(stderr.String(), "task=b7xk2q\n") {
		t.Errorf("child said %q", stderr.String())
	}
}
