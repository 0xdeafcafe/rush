package convo

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/testhome"
)

// The test binary stands in for rush when a host is spawned.
func TestMain(m *testing.M) {
	if len(os.Args) == 4 && os.Args[1] == "host" && os.Args[2] == "run" {
		if err := host.Run(os.Args[3]); err != nil {
			os.Exit(1)
		}
		os.Exit(0)
	}
	testhome.Main(m)
}

// TestRealSession drives a real Claude Code session through a host and
// draws it, so the event mapping is checked against what Claude Code
// actually sends. It spends a few cents of Haiku: RUSH_REAL_CLAUDE=1.
func TestRealSession(t *testing.T) {
	if os.Getenv("RUSH_REAL_CLAUDE") == "" {
		t.Skip("set RUSH_REAL_CLAUDE=1 to run against the installed claude")
	}
	home, err := os.MkdirTemp("/tmp", "rush-convo-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(home)
	t.Setenv("RUSH_HOME", home)
	work := filepath.Join(home, "work")
	_ = os.MkdirAll(work, 0o755)
	_ = os.WriteFile(filepath.Join(work, "notes.txt"), []byte("alpha\nbeta\ngamma\n"), 0o644)

	cfg, err := host.Spawn(host.Config{Cwd: work, Model: "haiku",
		Prompt: "Read notes.txt, replace the line beta with BETA using Edit, then run `wc -l notes.txt` and `ls /nonexistent-dir`, and finish with a one-sentence summary."})
	if err != nil {
		t.Fatal(err)
	}
	c, err := host.Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	defer c.Stop()

	s := New()
	timeout := time.After(150 * time.Second)
	for done := false; !done; {
		select {
		case line, ok := <-c.Lines:
			if !ok {
				t.Fatal("connection closed")
			}
			ev, err := host.Decode(line)
			if err != nil {
				continue
			}
			s.Apply(ev, time.Now())
			switch ev := ev.(type) {
			case event.Approval:
				_ = c.Allow(ev.ID, nil, false)
			case event.TurnEnd:
				done = true
			}
		case <-timeout:
			t.Fatal("timed out")
		}
	}
	lines := s.Render(Options{Width: 110, Now: time.Now(), Open: map[string]bool{}})
	t.Log("\n" + plain(lines))
	if len(s.Turns) != 1 || s.Turns[0].Live || s.Turns[0].Outcome() == "" {
		t.Fatalf("turn: %+v", s.Turns)
	}
	var edits, fails int
	for _, st := range s.byID {
		if st.Tool == "Edit" && st.Status == OK {
			edits++
		}
		if st.Tool == "Bash" && st.Status == Failed && st.Exit > 0 {
			fails++
		}
	}
	if edits == 0 || fails == 0 {
		t.Errorf("want an edit and a failed command, got %d and %d", edits, fails)
	}
}
