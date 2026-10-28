package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// An agent's running subagents get rows of their own under it, in both
// looks; past four, the last counts the rest, and ← tucks them away.
func TestSubagentRowsUnderTheirAgent(t *testing.T) {
	for _, look := range []string{"rail", "classic"} {
		m, _ := benchModel(120, 60)
		m.store.Config.ListLook = look
		a := m.snap.Agents[0]
		a.Subagents = []fleet.SubagentTile{
			{ID: "s1", Type: "Explore", Description: "find the render loop", Born: m.snap.At.Add(-3 * time.Minute)},
			{ID: "s2", Type: "general-purpose", Description: "bench the frame rate"},
		}
		out := func() string { return ansi.Strip(strings.Join(m.listLines(120, 60), "\n")) }
		got := out()
		for _, want := range []string{"├ ", "find the render loop · Explore", "3m", "└ ", "bench the frame rate · general-purpose"} {
			if !strings.Contains(got, want) {
				t.Errorf("%s: lacks %q:\n%s", look, want, got)
			}
		}
		if strings.Index(got, a.DisplayName) > strings.Index(got, "find the render loop") {
			t.Errorf("%s: the subagent sits above its agent:\n%s", look, got)
		}
		for i := range 4 {
			a.Subagents = append(a.Subagents, fleet.SubagentTile{ID: "x" + string(rune('a'+i)), Description: "more"})
		}
		if got := out(); !strings.Contains(got, "3 more subagents") {
			t.Errorf("%s: six don't count the rest:\n%s", look, got)
		}
		m.toggleSubagents(a)
		if got := out(); strings.Contains(got, "render loop") || strings.Contains(got, "more subagents") {
			t.Errorf("%s: tucked away, they still show:\n%s", look, got)
		}
		m.toggleSubagents(a)
		if !strings.Contains(out(), "render loop") {
			t.Errorf("%s: they don't come back", look)
		}
	}
}
