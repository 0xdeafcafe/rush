package plugin

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

const uuid = "8c76706f-1c00-4aed-9c6d-7509f3033943"

func TestParseSidebar(t *testing.T) {
	s, err := ParseSidebar("kanban", jsontext.Value(`{"title": "Kan\u001b[31mban",
		"sections": [{"title": "In Progress"}, {"title": "Wait‮ing\n"}],
		"agents": {"`+uuid+`": {"name": "Fix \u001b]0;pwned\u0007login\tbug", "section": "In Progress", "order": 2}}}`))
	if err != nil {
		t.Fatal(err)
	}
	if s.Plugin != "kanban" || s.Title != "Kanban" || s.Sections[1].Title != "Waiting" {
		t.Fatalf("not cleaned: %+v", s)
	}
	if a := s.Agents[uuid]; a.Name != "Fix login bug" || a.Order != 2 {
		t.Fatalf("agent = %+v", a)
	}

	for _, clear := range []string{``, `null`, `{}`, `{"sections": []}`} {
		s, err := ParseSidebar("kanban", jsontext.Value(clear))
		if err != nil || !s.Empty() {
			t.Errorf("%q: %+v %v", clear, s, err)
		}
	}

	many := make([]SidebarSection, MaxSidebarSections+1)
	for i := range many {
		many[i].Title = strings.Repeat("s", i+1)
	}
	manyJSON, _ := jsonx.Marshal(map[string]any{"sections": many})
	agents := map[string]SidebarAgent{}
	for i := range MaxSidebarAgents + 1 {
		agents[fmt.Sprintf("a%d", i)] = SidebarAgent{Section: "A"}
	}
	agentsJSON, _ := jsonx.Marshal(map[string]any{"sections": []SidebarSection{{"A"}}, "agents": agents})
	bad := map[string]string{
		"too many sections": string(manyJSON),
		"too many agents":   string(agentsJSON),
		"long title":        `{"title": "` + strings.Repeat("t", 65) + `", "sections": [{"title": "A"}]}`,
		"long section":      `{"sections": [{"title": "` + strings.Repeat("t", 65) + `"}]}`,
		"long name":         `{"sections": [{"title": "A"}], "agents": {"s1": {"name": "` + strings.Repeat("n", 201) + `", "section": "A"}}}`,
		"id as a path":      `{"sections": [{"title": "A"}], "agents": {"../x": {"section": "A"}}}`,
		"unknown section":   `{"sections": [{"title": "A"}], "agents": {"s1": {"section": "B"}}}`,
		"twice":             `{"sections": [{"title": "A"}, {"title": "A"}]}`,
		"untitled":          `{"sections": [{"title": "\u001b[0m"}]}`,
		"unknown field":     `{"sections": [{"title": "A"}], "color": "red"}`,
	}
	for name, params := range bad {
		if _, err := ParseSidebar("kanban", jsontext.Value(params)); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestSidebarsFollowApprovals(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	with := Manifest{Name: "kanban", Command: []string{"kanban"}, Sidebar: true}
	if err := Approve(Plugin{Manifest: with, Dir: install(t, with, nil)}); err != nil {
		t.Fatal(err)
	}
	s, _ := ParseSidebar("kanban", jsontext.Value(`{"title": "Kanban", "sections": [{"title": "A"}], "agents": {"s1": {"name": "one", "section": "A"}}}`))
	if err := SaveSidebar(s); err != nil {
		t.Fatal(err)
	}
	fi, err := os.Stat(SidebarPath("kanban"))
	if err != nil || fi.Mode().Perm() != 0o600 {
		t.Fatalf("sidebar file: %v %v", fi, err)
	}
	if d, _ := os.Stat(SidebarRoot()); d.Mode().Perm() != 0o700 {
		t.Fatalf("sidebar folder mode %o", d.Mode().Perm())
	}

	var w Sidebars
	got := w.Load()
	if len(got) != 1 || got[0].Label() != "Kanban" || got[0].Agents["s1"].Name != "one" || got[0].UpdatedAt.IsZero() {
		t.Fatalf("load = %+v", got)
	}

	// Changed on disk, it's read again; broken, it's left out.
	s.Agents["s1"] = SidebarAgent{Name: "two", Section: "A"}
	_ = SaveSidebar(s)
	later := time.Now().Add(time.Second)
	_ = os.Chtimes(SidebarPath("kanban"), later, later)
	if got := w.Load(); len(got) != 1 || got[0].Agents["s1"].Name != "two" {
		t.Fatalf("after a change = %+v", got)
	}
	_ = os.WriteFile(SidebarPath("kanban"), []byte(`{"plugin": "kanban", "sections": [{"title": "A"}], "agents": {"s1": {"section": "Z"}}}`), 0o600)
	if got := w.Load(); len(got) != 0 {
		t.Fatalf("a malformed file was used: %+v", got)
	}

	// A file under another plugin's name, not approved, is ignored, and
	// pruned.
	_ = SaveSidebar(Sidebar{Plugin: "other", Sections: []SidebarSection{{"X"}}})
	if got := w.Load(); len(got) != 0 {
		t.Fatalf("an unapproved plugin's sidebar was used: %+v", got)
	}
	PruneSidebars(Approvals())
	if _, err := os.Stat(SidebarPath("other")); !os.IsNotExist(err) {
		t.Fatal("prune left an unapproved plugin's sidebar")
	}

	// Clearing removes the file; so does revoking.
	_ = SaveSidebar(s)
	_ = SaveSidebar(Sidebar{Plugin: "kanban"})
	if _, err := os.Stat(SidebarPath("kanban")); !os.IsNotExist(err) {
		t.Fatal("an empty sidebar left its file")
	}
	_ = SaveSidebar(s)
	if err := Revoke("kanban"); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(SidebarPath("kanban")); !os.IsNotExist(err) {
		t.Fatal("revoking left the sidebar")
	}
	if got := w.Load(); len(got) != 0 {
		t.Fatalf("after revoke = %+v", got)
	}
}

func TestSidebarNeedsTheCapability(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m := Manifest{Name: "kanban", Command: []string{"kanban"}}
	if err := Approve(Plugin{Manifest: m, Dir: install(t, m, nil)}); err != nil {
		t.Fatal(err)
	}
	_ = SaveSidebar(Sidebar{Plugin: "kanban", Sections: []SidebarSection{{"A"}}})
	var w Sidebars
	if got := w.Load(); len(got) != 0 {
		t.Fatalf("a plugin approved without the sidebar arranged the list: %+v", got)
	}
}
