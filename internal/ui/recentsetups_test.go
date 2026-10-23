package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
)

// footModel is a Session beside a list short enough to leave room.
func footModel(t *testing.T, n int) *Model {
	t.Helper()
	m, _ := benchModel(140, 50)
	m.snap.Agents = m.snap.Agents[:n]
	m.rebuild()
	m.View()
	return m
}

// The side list has no "next agent" footer: the New session sheet says
// what a session starts as.
func TestNoNextAgentFooter(t *testing.T) {
	if v := ansi.Strip(footModel(t, 3).View().Content); strings.Contains(v, "next agent") {
		t.Fatalf("a next agent footer on a list with room to spare:\n%s", v)
	}
}

func TestRecentSetupsDedupedWithoutCurrent(t *testing.T) {
	m := footModel(t, 6)
	for i, a := range m.snap.Agents {
		a.Kind = "claude"
		a.Spend.Model = []string{"", "claude-sonnet-4-5", "claude-sonnet-4-5", "claude-haiku-4-5", "", "claude-sonnet-4-5"}[i]
	}
	m.host = nil // the Session's own setup isn't read from its host here
	cur := m.nextStart(m.startDir())
	got := m.recentSetups(cur)
	seen := map[string]bool{strings.Join(m.startWords(cur), " · "): true}
	for _, o := range got {
		w := strings.Join(m.startWords(o), " · ")
		if seen[w] {
			t.Fatalf("%q twice or the current setup, in %v", w, got)
		}
		seen[w] = true
	}
	if len(got) != 2 || got[0].model != "claude-sonnet-4-5" || got[1].model != "claude-haiku-4-5" {
		t.Fatalf("recent setups = %+v, want sonnet then haiku", got)
	}
}
