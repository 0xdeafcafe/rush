package ui

import (
	"slices"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// A finished agent with a draft in its box keeps its row while it ages out
// of its section; once the draft's gone it moves.
func TestDraftHoldsRowInPlace(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(160, 40)
	a := m.snap.Agents[1] // finished a minute ago
	m.sel, m.host.key = a.Key, a.Key
	m.rebuild()
	at := func() int { return slices.IndexFunc(m.order, func(b *fleet.Agent) bool { return b.Key == a.Key }) }
	was, wasAt := m.groupOf[a.Key], at()
	a.Done, a.UpdatedAt = true, m.snap.At.Add(-3*time.Hour)
	m.rebuild()
	if m.groupOf[a.Key] != was || at() != wasAt {
		t.Fatalf("typing: moved to %s #%d, want %s #%d", m.groupOf[a.Key], at(), was, wasAt)
	}
	m.host.input = nil
	m.rebuild()
	if m.groupOf[a.Key] != "Earlier" {
		t.Fatalf("draft gone: in %s, want Earlier", m.groupOf[a.Key])
	}
}

// A finish you never looked at waits in Finished for a day, then goes to
// Earlier like any other: Finished doesn't keep it for good.
func TestUnseenFinishAgesIntoEarlier(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(160, 40)
	a := m.snap.Agents[1]
	a.Done, a.PID, a.Past, a.Interactive, a.Seen, a.Pinned = false, 0, false, false, false, false
	a.State, a.Needs, a.Subs.Direct, a.Subs.Nested = "done", "", 0, 0
	a.UpdatedAt = m.snap.At.Add(-time.Minute)
	m.rebuild()
	if got := m.groupOf[a.Key]; got != finishedSection {
		t.Fatalf("just finished: in %q, want %s", got, finishedSection)
	}
	a.UpdatedAt = m.snap.At.Add(-25 * time.Hour)
	m.rebuild()
	if got := m.groupOf[a.Key]; got != "Earlier" {
		t.Fatalf("a day on: in %q, want Earlier", got)
	}
}
