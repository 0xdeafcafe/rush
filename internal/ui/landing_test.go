package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

const landTree = "/src/checkout/.claude/worktrees/tax"

// landModel is one stopped agent in a worktree, git having said st of it.
func landModel(st fleet.GitState, pr *fleet.PR) (*Model, *fleet.Agent) {
	now := time.Now()
	a := &fleet.Agent{Key: "tax", DisplayName: "fix tax rounding", Root: "/src/checkout", Repo: landTree, PR: pr}
	a.State, a.UpdatedAt, a.Seen = "done", now.Add(-20*time.Minute), true // looked at: Finished no longer has it
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 120, h: 40, lastState: map[string]string{}}
	m.store.Config.ListLook = "classic"
	m.folders.byRoot = map[string]fleet.Folder{"/src/checkout": {
		Root:  "/src/checkout",
		Git:   fleet.GitState{Branch: "main"},
		Trees: map[string]fleet.GitState{landTree: st},
	}}
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{a}}
	return m, a
}

// Each row in a worktree says what's left before its work lands, worst
// first, and only what's yours to merge goes under To land.
func TestLanding(t *testing.T) {
	base := fleet.GitState{Branch: "rush/tax", Base: "main"}
	with := func(f func(*fleet.GitState)) fleet.GitState { s := base; f(&s); return s }
	open := &fleet.PR{Number: 41, State: "open", Checks: "pass"}
	for _, c := range []struct {
		name    string
		st      fleet.GitState
		pr      *fleet.PR
		kind    landKind
		text    string
		section string
	}{
		{"uncommitted", with(func(s *fleet.GitState) { s.Changed, s.BaseAhead = 3, 2 }), open, landUncommitted, "±3 not committed", ""},
		{"commits, no pull request", with(func(s *fleet.GitState) { s.BaseAhead = 2 }), nil, landUnmerged, "2 commits not on main", toLandSection},
		{"one commit", with(func(s *fleet.GitState) { s.BaseAhead = 1 }), nil, landUnmerged, "1 commit not on main", toLandSection},
		{"ready", with(func(s *fleet.GitState) { s.BaseAhead = 2 }), open, landPRReady, "PR #41 ready to merge", toLandSection},
		{"failing", base, &fleet.PR{Number: 41, State: "open", Checks: "fail"}, landPRFailing, "PR #41 ✗ checks", ""},
		{"draft", base, &fleet.PR{Number: 41, State: "draft"}, landPRWaiting, "PR #41 draft", ""},
		{"merged", with(func(s *fleet.GitState) { s.BaseAhead = 2 }), &fleet.PR{Number: 41, State: "merged"}, landLanded, "landed · Earlier soon", ""},
		{"nothing done", base, nil, landNothing, "nothing to land", ""},
	} {
		t.Run(c.name, func(t *testing.T) {
			m, a := landModel(c.st, c.pr)
			l := m.landOf(a)
			if l.kind != c.kind || l.text != c.text {
				t.Fatalf("landOf = %v %q, want %v %q", l.kind, l.text, c.kind, c.text)
			}
			m.rebuild()
			if got := m.groupOf[a.Key]; (got == toLandSection) != (c.section == toLandSection) {
				t.Errorf("in %q, want To land: %v", got, c.section == toLandSection)
			}
			row := ansi.Strip(m.agentLine(a, 110, 110, false, 30, false, ""))
			if !strings.Contains(row, c.text) {
				t.Errorf("no %q at the row's end: %q", c.text, row)
			}
			m.store.Config.ListLook = ""
			if where := ansi.Strip(m.agentWhere(a, 110)); !strings.Contains(where, c.text) {
				t.Errorf("no %q under the name in the rail look: %q", c.text, where)
			}
			if c.kind != landLanded && c.kind != landNothing && landText(l) == "" {
				t.Error("nothing to tell it")
			}
		})
	}
}

// The main checkout, an agent at work, or git not having said yet: no tag.
func TestLandingQuiet(t *testing.T) {
	m, a := landModel(fleet.GitState{Branch: "rush/tax", BaseAhead: 2}, nil)
	a.Repo = a.Root
	if l := m.landOf(a); l.kind != landNone {
		t.Errorf("main checkout: %v", l.kind)
	}
	a.Repo = landTree
	a.State = "working"
	a.PID = 7
	if l := m.landOf(a); l.kind != landNone {
		t.Errorf("at work: %v", l.kind)
	}
	a.State, a.PID = "done", 0
	m.folders.byRoot = nil
	if l := m.landOf(a); l.kind != landNone {
		t.Errorf("git not asked yet: %v", l.kind)
	}
}
