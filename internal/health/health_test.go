package health

import (
	"os"
	"testing"
	"time"
)

// A login that stops working is failing at once; an answer since clears it.
// A busy provider never fails. Failures for a moment do after Streak in a
// row, or two spanning StreakWindow.
func TestState(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	t0 := time.Now()
	const k = "claude/a"
	Answered(k, t0)
	if st := Failed(k, "API Error: 429 rate_limit_error", t0.Add(time.Second)); st != OK {
		t.Fatalf("rate limited isn't failing: %v", st)
	}
	if st := Failed(k, "API Error: 401 invalid_grant sk-secret", t0.Add(2*time.Second)); st != Failing {
		t.Fatalf("a refused login fails at once: %v", st)
	}
	if e := Load()[k]; e.Error != "401 invalid_grant" {
		t.Fatalf("the error kept should be the summary: %q", e.Error)
	}
	Answered(k, t0.Add(3*time.Second))
	for i := range Streak - 1 {
		if st := Failed(k, "API Error: Connection error.", t0.Add(time.Duration(4+i)*time.Second)); st != OK {
			t.Fatalf("failure %d for a moment: %v", i+1, st)
		}
	}
	if st := Failed(k, "API Error: Connection error.", t0.Add(10*time.Second)); st != Failing {
		t.Fatalf("a streak should fail: %v", st)
	}
	Answered(k, t0.Add(11*time.Second))
	Failed(k, "API Error: 500", t0.Add(12*time.Second))
	if st := Failed(k, "API Error: 500", t0.Add(12*time.Second+StreakWindow)); st != Failing {
		t.Fatalf("two spanning the window should fail: %v", st)
	}
	if st := Failed(k, "Prompt is too long", t0.Add(time.Hour)); st != Unknown || Load()[k].Error != "500" {
		t.Fatal("a failure saying nothing of the provider isn't kept")
	}
}

// Writes merge by their newest times, so a late write of an old answer
// doesn't clear a newer failure; an answer with nothing to clear writes
// nothing; a login resets its harness's records.
func TestMergeAndReset(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	t0 := time.Now()
	Answered("codex/x", t0)
	Failed("codex/x", "401 Unauthorized", t0.Add(2*time.Second))
	Answered("codex/x", t0.Add(time.Second)) // an older answer, written late
	if Load()["codex/x"].State() != Failing {
		t.Fatal("an older answer cleared a newer failure")
	}
	Answered("claude/a", t0)
	fi, _ := os.Stat(Path())
	time.Sleep(10 * time.Millisecond)
	Answered("claude/a", t0.Add(time.Minute))
	if fi2, _ := os.Stat(Path()); !fi2.ModTime().Equal(fi.ModTime()) {
		t.Fatal("an answer that changes nothing wrote the file")
	}
	Reset("codex", "")
	if _, ok := Load()["codex/x"]; ok {
		t.Fatal("logging in should forget the harness's records")
	}
	if _, ok := Load()["claude/a"]; !ok {
		t.Fatal("another harness's record went too")
	}
}
