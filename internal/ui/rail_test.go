package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

// railModel is a list with a question, a finished turn with a failing PR
// and two agents at work, in one repository with a linked worktree.
func railModel(look string) *Model {
	now := time.Now()
	agent1 := func(key, st string) *fleet.Agent {
		a := &fleet.Agent{Key: key, DisplayName: key, PID: 7, Root: "/src/checkout", Repo: "/src/checkout"}
		a.State, a.UpdatedAt = st, now.Add(-5*time.Minute)
		return a
	}
	ask := agent1("upgrade react", "blocked")
	done := agent1("fix tax rounding", "done")
	done.Repo = "/src/checkout/.claude/worktrees/tax"
	pr := agent.PR{Number: 118, State: "OPEN"}
	pr.Checks.Passed, pr.Checks.Failed = 4, 1
	done.PRs = []agent.PR{pr}
	w1, w2 := agent1("add apple pay", "working"), agent1("rate limit the api", "working")
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 120, h: 40, lastState: map[string]string{}}
	m.store.Config.ListLook = look
	m.folders.byRoot = map[string]fleet.Folder{"/src/checkout": {
		Root: "/src/checkout",
		Git:  fleet.GitState{Branch: "main", Changed: 6},
		Trees: map[string]fleet.GitState{
			"/src/checkout/.claude/worktrees/tax": {Branch: "rush/tax-rounding", Ahead: 2},
		},
	}}
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{ask, done, w1, w2}}
	m.rebuild()
	return m
}

// The rail look: each section an edge, each agent a branch off it with a
// line under it saying where it works, and no project headings.
func TestRailLook(t *testing.T) {
	m := railModel("")
	lines := m.listLines(110, 40)
	plain := make([]string, len(lines))
	for i, l := range lines {
		plain[i] = ansi.Strip(l)
	}
	text := strings.Join(plain, "\n")
	if p := os.Getenv("RUSH_RAIL_DUMP"); p != "" {
		_ = os.WriteFile(p, []byte(strings.Join(lines, "\n")+"\n"), 0o644)
	}
	for _, want := range []string{
		"┃ Needs you",
		"┃ Finished",
		"┃ Working",
		"rush/tax-rounding · ↑2 · PR #118 ✗ 1 failed",
		"main · ±6",
	} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	if !strings.Contains(text, "├─") || !strings.Contains(text, "└─") {
		t.Errorf("no branches in\n%s", text)
	}
	for _, l := range m.lines {
		if l.kind == lineProject || l.kind == lineTree {
			t.Fatalf("a project heading in the rail look: %+v", l)
		}
	}
	// Every row of an agent, its second line too, answers to its key.
	n := 0
	for _, k := range m.rowKeys {
		if k == "fix tax rounding" {
			n++
		}
	}
	if n != 2 {
		t.Errorf("the finished agent has %d rows, want 2", n)
	}
}

// The classic look keeps a heading per project and one line per agent.
func TestClassicLook(t *testing.T) {
	m := railModel("classic")
	text := ansi.Strip(strings.Join(m.listLines(110, 40), "\n"))
	if strings.Contains(text, "┃ Needs you") || strings.Contains(text, "PR #118 ✗") {
		t.Errorf("the rail look in classic:\n%s", text)
	}
}
