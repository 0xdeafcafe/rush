package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Earlier goes by the time its rows show, newest first: one put away just
// now, but last at work days ago, sits below one at work yesterday.
func TestEarlierGoesByTheTimeShown(t *testing.T) {
	now := time.Now()
	old := &fleet.Agent{Key: "old", DisplayName: "old", Done: true, State: "done", UpdatedAt: now.Add(-72 * time.Hour)}
	newer := &fleet.Agent{Key: "newer", DisplayName: "newer", Done: true, State: "done", UpdatedAt: now.Add(-26 * time.Hour)}
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 120, h: 40, lastState: map[string]string{}}
	m.store.Overlay.Done = map[string]time.Time{"old": now, "newer": now.Add(-25 * time.Hour)}
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{old, newer}}
	m.rebuild()
	var got []string
	for _, a := range m.order {
		got = append(got, a.Key)
	}
	if strings.Join(got, ",") != "newer,old" {
		t.Errorf("Earlier = %v, want newer,old", got)
	}
}
