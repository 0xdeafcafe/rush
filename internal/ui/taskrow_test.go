package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/convo"
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
