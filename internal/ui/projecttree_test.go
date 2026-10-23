package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

// Projects in the rail look: a repository's main checkout and worktrees
// hung off it, each ending in clean-up's verdict.
func TestProjectTree(t *testing.T) {
	now := time.Now()
	const root = "/src/checkout"
	a := &fleet.Agent{Key: "pay", DisplayName: "add Apple Pay to checkout", PID: 7, Root: root, Repo: root + "/.claude/worktrees/apple-pay", Cwd: root + "/.claude/worktrees/apple-pay"}
	a.State, a.UpdatedAt = "working", now
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 140, h: 40, lastState: map[string]string{}}
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{a}}
	m.folders.byRoot = map[string]fleet.Folder{root: {Root: root, Git: fleet.GitState{Branch: "main", Behind: 3}}}
	checked := now.Add(-time.Minute)
	m.clean.wts = []fleet.Worktree{
		{Path: root + "/.claude/worktrees/apple-pay", Repo: root, Branch: "rush/apple-pay", Made: fleet.MadeByRush, Own: true, InUse: "add Apple Pay", Changed: 6, Checked: checked, Size: 240 << 20},
		{Path: root + "/.claude/worktrees/promo-focus", Repo: root, Branch: "rush/promo-focus", Made: fleet.MadeByRush, Own: true, Checked: checked, Landed: true, Target: "main", Size: 1 << 30},
		{Path: root + "/.worktrees/spike", Repo: root, Branch: "spike/streams", Made: fleet.MadeByOther, Checked: checked, Unpushed: 2, OffRemote: 2},
	}
	m.rebuild()
	rows := m.projectTree(130)
	var lines []string
	for _, r := range rows {
		lines = append(lines, r.line)
	}
	if p := os.Getenv("RUSH_TREE_DUMP"); p != "" {
		_ = os.WriteFile(p, []byte(strings.Join(lines, "\n")+"\n"), 0o644)
	}
	text := ansi.Strip(strings.Join(lines, "\n"))
	for _, want := range []string{"checkout", "3 behind", "main checkout", "apple-pay", "in use", "promo-focus", "can go", ".worktrees/spike", "not made by rush", "Temporary"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	if !strings.Contains(text, "└─") {
		t.Errorf("no closing branch in\n%s", text)
	}
	// Each worktree is a row to pick; enter on it asks to remove it.
	n := 0
	for _, r := range rows {
		if r.wt != nil && r.pickable() {
			n++
		}
	}
	if n != 3 {
		t.Errorf("%d worktree rows to pick, want 3", n)
	}
}
