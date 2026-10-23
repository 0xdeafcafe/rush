package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Delete says what goes before anything does: the path, that git removes
// it as a worktree, the branch that stays, and loudly, the files lost and
// the commits only that branch keeps; y does nothing while an agent runs
// in it.
func TestDeleteSheetSaysWhatGoes(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now, Agents: []*fleet.Agent{
		{Key: "done", DisplayName: "Fix the parser"},
		{Key: "run", DisplayName: "Still going", PID: 7},
	}}, w: 140, h: 60}
	wt := &fleet.Worktree{Path: "/r/.claude/worktrees/devclean", Repo: "/r", Branch: "worktree-devclean",
		Checked: now, Changed: 2, Unpushed: 1, Size: 5 << 20, Agents: []string{"done"}}
	s := &deleteSheet{items: []doomed{{wt: wt, looked: true,
		files:   []string{" M internal/ui/projects.go", "?? notes.txt"},
		commits: []string{"abc1234 feat(ui): a thing"},
	}}}
	m.sheet = s
	body := ansi.Strip(strings.Join(s.body(m, 110, 50), "\n"))
	for _, want := range []string{
		"/r/.claude/worktrees/devclean", "worktree · git removes it", "5M",
		"branch worktree-devclean stays",
		"2 uncommitted files LOST for good", "internal/ui/projects.go", "notes.txt",
		"1 commit not pushed or in main", "only worktree-devclean keeps them", "abc1234 feat(ui): a thing",
		"worked here: Fix the parser", "1 would lose work", "y delete, losing it",
	} {
		if !strings.Contains(body, want) {
			t.Errorf("sheet lacks %q:\n%s", want, body)
		}
	}

	wt.Agents = []string{"run"}
	body = ansi.Strip(strings.Join(s.body(m, 110, 50), "\n"))
	if !strings.Contains(body, "can't go: Still going is running in it") || strings.Contains(body, "y delete") {
		t.Errorf("a running agent should block it:\n%s", body)
	}
	if s.key(m, tea.KeyPressMsg{}, "y"); m.sheet != s {
		t.Error("y went ahead with an agent running in it")
	}
}

// Clean up's enter opens Delete on what's ticked; esc there comes back to
// the checklist.
func TestCleanSheetHandsToDelete(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now}}
	m.clean.checked = now
	m.clean.wts = []fleet.Worktree{{Path: t.TempDir() + "/gone", Repo: "/nowhere", Checked: now, Size: 10, Own: true}}
	m.openCleanSheet()
	c := m.sheet.(*cleanSheet)
	c.key(m, tea.KeyPressMsg{}, "enter")
	d, ok := m.sheet.(*deleteSheet)
	if !ok || len(d.items) != 1 || d.items[0].wt == nil {
		t.Fatalf("enter opened %T", m.sheet)
	}
	d.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != sheet(c) {
		t.Fatalf("esc left %T", m.sheet)
	}
}
