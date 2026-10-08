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
	if m.groupOf[a.Key] != "Done" {
		t.Fatalf("draft gone: in %s, want Done", m.groupOf[a.Key])
	}
}
