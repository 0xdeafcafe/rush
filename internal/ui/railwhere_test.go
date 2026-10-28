package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A session's view says where it stands after its project and branch on
// the rail's second line, never in their place.
func TestAgentWhereKeepsProjectBesideViewSummary(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}}
	a := &fleet.Agent{Key: "k", ID: "s1", Rush: true, Root: "/src/rush", Branch: "main"}
	m.projTitles = map[string]string{"/src/rush": "rush"}
	m.agentViews = map[string]*sessionViews{"s1": {checked: time.Now(), list: []host.View{{Name: "plan", Summary: "8 milestones, 1 running"}}}}
	got := ansi.Strip(m.agentWhere(a, 80))
	if !strings.HasPrefix(got, "rush · main · 8 milestones, 1 running") {
		t.Fatalf("second line = %q, want the project, branch, then the view's summary", got)
	}
}
