package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

func TestWorktreeRowSaysWhatItHas(t *testing.T) {
	m := &Model{}
	st := fleet.GitState{Branch: "worktree-devclean", Base: "main", BaseAhead: 3, BaseBehind: 89}
	got := ansi.Strip(m.worktreeLine("/r/.claude/worktrees/devclean", st, fleet.Worktree{Size: 3 << 20}, 1, 120))
	for _, want := range []string{"devclean", "3 commits", "main · 89 behind", "● 1 running", "3M"} {
		if !strings.Contains(got, want) {
			t.Errorf("row %q lacks %q", got, want)
		}
	}
	if strings.Contains(got, "worktree-devclean") {
		t.Errorf("row %q repeats the name as its branch", got)
	}
}

// A worktree folder named for its branch says the name once in the list.
func TestTreeLineNamesOnce(t *testing.T) {
	m := &Model{}
	root := "/r/.worktrees/feat-slack-migration"
	m.folders.byRoot = map[string]fleet.Folder{"/r": {Trees: map[string]fleet.GitState{root: {Branch: "feat/slack-migration", Changed: 1, Base: "origin/main"}}}}
	got := ansi.Strip(m.treeLine(listLine{title: "/r", root: root}, 120))
	if strings.Contains(got, "feat/slack-migration") || !strings.Contains(got, "feat-slack-migration  from origin/main  ±1") {
		t.Fatalf("row %q", got)
	}
	m.folders.byRoot["/r"].Trees[root] = fleet.GitState{Branch: "other", Changed: 1}
	if got := ansi.Strip(m.treeLine(listLine{title: "/r", root: root}, 120)); !strings.Contains(got, "feat-slack-migration  other  ±1") {
		t.Fatalf("another branch is still named: %q", got)
	}
}

// A worktree with one agent heads no row of its own: its name goes beside
// the agent's, which sits where the heading would have. Two keep it.
func TestSoloWorktreeRow(t *testing.T) {
	m, _ := benchModel(90, 40)
	m.store.Config.StackAt = 100 // two lines a row
	m.store.Config.ListLook = "classic" // the rail look has no worktree headings
	a, b, c := m.snap.Agents[0], m.snap.Agents[1], m.snap.Agents[2]
	m.lines = []listLine{
		{kind: lineProject, title: "rush", root: "/work/rush"},
		{kind: lineTree, title: "/work/rush", root: "/work/wt-solo"},
		{kind: lineAgent, agent: a, inset: 2 * rowInset},
		{kind: lineTree, title: "/work/rush", root: "/work/wt-pair"},
		{kind: lineAgent, agent: b, inset: 2 * rowInset},
		{kind: lineAgent, agent: c, inset: 2 * rowInset},
	}
	out := ansi.Strip(strings.Join(m.listLines(90, 40), "\n"))
	row := func(name string) string {
		for _, l := range strings.Split(out, "\n") {
			if strings.Contains(l, name) {
				return l
			}
		}
		return ""
	}
	if strings.Contains(out, "↳ wt-solo ┄") || !strings.Contains(row(a.DisplayName), "↳ wt-solo") {
		t.Errorf("the solo worktree:\n%s", out)
	}
	if !strings.Contains(out, "↳ wt-pair ┄") || strings.Contains(row(b.DisplayName), "↳") {
		t.Errorf("the worktree of two:\n%s", out)
	}
}
