package host

import (
	"bufio"
	"os"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/events"
)

func TestEventsLogStateChangesOnce(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	s := &server{cfg: Config{ID: "ev"}}
	if err := os.MkdirAll(dir("ev"), 0o700); err != nil {
		t.Fatal(err)
	}
	s.info.State = "working"
	s.noteState()
	s.noteState() // unchanged: not logged again
	s.info.State = "idle"
	s.info.Background = []Task{{ID: "b1"}}
	s.noteState()
	s.event("check-in", map[string]any{"why": "long background tasks"})
	events.Flush()

	f, err := os.Open(EventsPath("ev"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	var kinds, states []string
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		var l struct {
			Kind string `json:"kind"`
			T    string `json:"t"`
			Info shown  `json:"info"`
		}
		if err := jsonx.Unmarshal(sc.Bytes(), &l); err != nil || l.T == "" {
			t.Fatalf("line %q: %v", sc.Text(), err)
		}
		kinds = append(kinds, l.Kind)
		states = append(states, l.Info.State)
	}
	if len(kinds) != 3 || kinds[0] != "state" || kinds[1] != "state" || kinds[2] != "check-in" || states[0] != "working" || states[1] != "idle" {
		t.Fatalf("kinds %v states %v", kinds, states)
	}
}
