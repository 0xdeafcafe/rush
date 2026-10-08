package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

const board2 = `{"links": [
  {"id": "c1", "name": "Older", "column": "requires_attention", "sessionLink": {"sessionId": "11111111-aaaa"}, "lastActivity": "2026-09-01T00:00:00Z", "updatedAt": "x", "manuallyArchived": false},
  {"id": "c2", "name": "Newer", "column": "requires_attention", "sessionLink": {"sessionId": "22222222-bbbb"}, "lastActivity": "2026-09-03T00:00:00Z", "updatedAt": "x", "manuallyArchived": false},
  {"id": "c3", "name": "Placed", "column": "requires_attention", "sortOrder": 0, "sessionLink": {"sessionId": "33333333-cccc"}, "updatedAt": "2026-08-01T00:00:00Z", "manuallyArchived": false},
  {"id": "c4", "column": "in_progress", "promptBody": "Fix the flaky test\nin CI", "sessionLink": {"sessionId": "44444444-dddd"}, "updatedAt": "x", "manuallyArchived": false},
  {"id": "c5", "name": "No conversation yet", "column": "backlog", "updatedAt": "x", "manuallyArchived": false},
  {"id": "c6", "name": "Archived", "column": "done", "sessionLink": {"sessionId": "66666666-ffff"}, "updatedAt": "x", "manuallyArchived": true},
  {"id": "c7", "name": "Discovered", "column": "all_sessions", "sessionLink": {"sessionId": "77777777-0000"}, "updatedAt": "x", "manuallyArchived": false},
  {"id": "c9", "column": "done", "sessionLink": {"sessionId": "99999999-2222"}, "updatedAt": "y", "manuallyArchived": false},
  {"id": "c8", "name": "Shipped", "column": "done", "sessionLink": {"sessionId": "88888888-1111"}, "updatedAt": "x", "manuallyArchived": false}
]}`

func TestSidebarPayload(t *testing.T) {
	home := t.TempDir()
	t.Setenv("KANBAN_CODE_HOME", home)
	_ = os.WriteFile(filepath.Join(home, "links.json"), []byte(board2), 0o600)
	cards, err := readCards()
	if err != nil {
		t.Fatal(err)
	}
	b, _ := jsonx.Marshal(sidebarPayload(cards))

	// It's what rush's broker takes.
	s, err := plugin.ParseSidebar("kanban", b)
	if err != nil {
		t.Fatalf("rush refuses %s: %v", b, err)
	}
	var sections []string
	for _, sec := range s.Sections {
		sections = append(sections, sec.Title)
	}
	if got := strings.Join(sections, ","); got != "In Progress,Waiting,Done" {
		t.Fatalf("sections = %s", got)
	}
	want := map[string]plugin.SidebarAgent{
		"44444444-dddd": {Name: "Fix the flaky test", Section: "In Progress", Order: 0},
		"33333333-cccc": {Name: "Placed", Section: "Waiting", Order: 0},
		"22222222-bbbb": {Name: "Newer", Section: "Waiting", Order: 1},
		"11111111-aaaa": {Name: "Older", Section: "Waiting", Order: 2},
		"99999999-2222": {Section: "Done", Order: 0},
		"88888888-1111": {Name: "Shipped", Section: "Done", Order: 1},
	}
	if len(s.Agents) != len(want) {
		t.Fatalf("agents = %+v", s.Agents)
	}
	for id, w := range want {
		if s.Agents[id] != w {
			t.Errorf("%s = %+v, want %+v", id, s.Agents[id], w)
		}
	}
	if s.Title != "Kanban" {
		t.Errorf("title %q", s.Title)
	}

	empty, _ := jsonx.Marshal(sidebarPayload(nil))
	if s, err := plugin.ParseSidebar("kanban", empty); err != nil || !s.Empty() {
		t.Fatalf("an empty board should clear the list's arrangement: %s %v", empty, err)
	}
}
