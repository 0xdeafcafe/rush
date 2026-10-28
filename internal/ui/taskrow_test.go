package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func TestTaskRowWindow(t *testing.T) {
	s := &convo.Session{Tasks: []convo.Task{
		{Subject: "one", Status: "completed"}, {Subject: "two", Status: "completed"},
		{Subject: "three", Status: "in_progress"}, {Subject: "four"}, {Subject: "five"}, {Subject: "six"},
	}}
	rows := taskRow(s, 80, false)
	got := ansi.Strip(strings.Join(rows, "\n"))
	for _, want := range []string{"2 of 6 done", "all tasks ›", "✓ two", "■ three", "☐ four", "☐ five", "+1 more"} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q in\n%s", want, got)
		}
	}
	if strings.Contains(got, "✓ one") {
		t.Errorf("shows tasks done before the last:\n%s", got)
	}
}

func TestTaskRowShut(t *testing.T) {
	s := &convo.Session{Tasks: []convo.Task{{Subject: "one", Status: "completed"}, {Subject: "two", Status: "in_progress"}, {Subject: "three"}}}
	rows := taskRow(s, 80, true)
	if len(rows) != 1 || !strings.Contains(ansi.Strip(rows[0]), "▸ Tasks") || !strings.Contains(ansi.Strip(rows[0]), "two") {
		t.Fatalf("shut should be one row with the task under way: %q", rows)
	}
}

// With room right of the conversation, the tasks go in the column there,
// all in sight, and leave the box; without it they stay on the box.
func TestTasksBeside(t *testing.T) {
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: convo.New(), open: map[string]bool{}}
	c.sess.Tasks = []convo.Task{{Subject: "one", Status: "completed"}, {Subject: "two", Status: "in_progress"}, {Subject: "three"}}
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, host: c, paneFocus: true, w: 240, h: 60}
	a := &fleet.Agent{Key: "k", Rush: true}

	dock := ansi.Strip(strings.Join(m.paneDock(a, c, 200, 60), "\n"))
	side := ansi.Strip(strings.Join(c.side, "\n"))
	if !c.tasksBeside || strings.Contains(dock, "Tasks") {
		t.Fatalf("200 wide, the tasks stayed on the box:\n%s", dock)
	}
	for _, want := range []string{"Tasks  1 of 3", "✓ one", "■ two", "☐ three"} {
		if !strings.Contains(side, want) {
			t.Errorf("column missing %q:\n%s", want, side)
		}
	}
	if c.panelRefs[sideRefs+1] != sideTasksRef {
		t.Errorf("a task row's click goes to %q", c.panelRefs[sideRefs+1])
	}

	dock = ansi.Strip(strings.Join(m.paneDock(a, c, 120, 60), "\n"))
	if c.tasksBeside || len(c.side) > 0 || !strings.Contains(dock, "Tasks") {
		t.Fatalf("120 wide, the tasks should sit on the box:\n%s", dock)
	}
}
