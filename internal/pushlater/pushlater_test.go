package pushlater

import (
	"errors"
	"testing"
	"time"
)

func TestRewrite(t *testing.T) {
	for cmd, want := range map[string]string{
		"git push":                  "'rush' push-later",
		"git push -u origin feat/x": "'rush' push-later -u origin feat/x",
		"git add a && git commit -m x && git push": "git add a && git commit -m x && 'rush' push-later",
		"cd repo; git push origin HEAD":            "cd repo; 'rush' push-later origin HEAD",
		"git push --force":                         "",
		"git push --force-with-lease":              "",
		"git push origin --delete feat/x":          "",
		"git push --tags":                          "",
		"git push origin main":                     "",
		"git push origin HEAD:main":                "",
		"git push origin a b c":                    "",
		"git push && gh pr create":                 "",
		"git push | tail -2":                       "",
		"git push 2>&1":                            "",
		"echo git push":                            "",
	} {
		got, ok := Rewrite(cmd, "rush")
		if ok != (want != "") || got != want {
			t.Errorf("Rewrite(%q) = %q, %v; want %q", cmd, got, ok, want)
		}
	}
}

func TestDue(t *testing.T) {
	at := time.Date(2026, 10, 23, 12, 0, 0, 0, time.UTC)
	if d := Due(at, at.Add(30*time.Second)); d != Quiet-30*time.Second {
		t.Errorf("Due 30s in = %s", d)
	}
	if d := Due(at, at.Add(Quiet+time.Second)); d != 0 {
		t.Errorf("Due past quiet = %s", d)
	}
}

// A burst of holds sends one push, the newest, once the branch is quiet;
// a failure is told once.
func TestWaiterSendsTheNewestOnceQuiet(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	clock := time.Date(2026, 10, 23, 12, 0, 0, 0, time.UTC)
	key, err := Write(Hold{Dir: "/r", Branch: "feat/x", Args: []string{"first"}, At: clock})
	if err != nil {
		t.Fatal(err)
	}
	var pushed [][]string
	var slept []time.Duration
	var failed []error
	held := false
	w := Waiter{
		Now: func() time.Time { return clock },
		Sleep: func(d time.Duration) {
			slept = append(slept, d)
			clock = clock.Add(d)
			if !held { // a second push lands a minute into the wait
				held = true
				_, _ = Write(Hold{Dir: "/r", Branch: "feat/x", Args: []string{"second"}, At: clock.Add(-Quiet + time.Minute)})
			}
		},
		Push: func(h Hold) error { pushed = append(pushed, h.Args); return errors.New("rejected") },
		Fail: func(_ Hold, err error) { failed = append(failed, err) },
	}
	w.Run(key)
	if len(pushed) != 1 || pushed[0][0] != "second" {
		t.Fatalf("pushed %v, want only the newest", pushed)
	}
	if len(slept) != 2 || len(failed) != 1 {
		t.Fatalf("slept %v, failed %v", slept, failed)
	}
	if _, err := Read(key); err == nil {
		t.Fatal("the hold outlived its push")
	}
}
