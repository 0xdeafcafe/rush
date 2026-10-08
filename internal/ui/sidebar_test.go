package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/state"
)

// sidebarPlugin approves a plugin that arranges the list, and sets its
// arrangement, as the broker would have.
func sidebarPlugin(t *testing.T, s plugin.Sidebar) {
	t.Helper()
	m := plugin.Manifest{Name: s.Plugin, Command: []string{s.Plugin}, Sidebar: true}
	dir := filepath.Join(plugin.Root(), m.Name)
	_ = os.MkdirAll(dir, 0o700)
	b, _ := jsonx.Marshal(m)
	_ = os.WriteFile(filepath.Join(dir, "plugin.json"), b, 0o600)
	if err := plugin.Approve(plugin.Plugin{Manifest: m, Dir: dir}); err != nil {
		t.Fatal(err)
	}
	if err := plugin.SaveSidebar(s); err != nil {
		t.Fatal(err)
	}
}

// loaded is a view with its first reading of the fleet taken in, as the
// running view has it a moment after it starts.
func loaded(m *Model) *Model {
	m.refreshNow()
	m.pinHosted()
	return m
}

func sid(id string) string { return id + "-0000-4000-8000-000000000000" }

func TestSidebarArrangesTheList(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "session a")
	writeSession(t, "bbbb2222", "session b")
	writeSession(t, "cccc3333", "session c")
	writeSession(t, "dddd4444", "session d")
	sidebarPlugin(t, plugin.Sidebar{Plugin: "kanban", Title: "Kanban",
		Sections: []plugin.SidebarSection{{Title: "In Progress"}, {Title: "Waiting"}},
		Agents: map[string]plugin.SidebarAgent{
			sid("aaaa1111"): {Name: "Fix login bug", Section: "Waiting", Order: 1},
			sid("bbbb2222"): {Name: "Dark mode", Section: "Waiting", Order: 0},
			sid("cccc3333"): {Name: "Card named C", Section: "In Progress"},
		}})

	m := loaded(New(state.Load(), "test"))
	if modes := m.groupModes(); modes[len(modes)-1] != "plugin:kanban" {
		t.Fatalf("group modes = %v", modes)
	}
	var cKey string
	for _, a := range m.snap.Agents {
		if a.SessionID == sid("cccc3333") {
			cKey = a.Key
		}
	}
	m.store.Overlay.Names[cKey] = "my own name"
	m.store.Config.GroupBy = "plugin:kanban"
	m.refreshNow()

	var got []string
	for _, l := range m.lines {
		switch l.kind {
		case lineSection:
			got = append(got, "§"+l.title)
		case lineAgent:
			if strings.HasPrefix(l.agent.DisplayName, "session") || l.agent.SessionID != "" && strings.HasSuffix(l.agent.SessionID, "-000000000000") {
				got = append(got, l.agent.DisplayName)
			}
		}
	}
	want := "§In Progress,my own name,§Waiting,Dark mode,Fix login bug,§Other"
	if strings.Join(got, ",") != want {
		t.Fatalf("list = %v\nwant %s", got, want)
	}
	if !m.folded(otherSection) {
		t.Fatal("Other should start folded")
	}
	if g := m.groupOf[m.keyOf(sid("dddd4444"))]; g != otherSection {
		t.Fatalf("an agent the plugin doesn't place is in %q", g)
	}

	out := m.Frame(160, 45)
	for _, want := range []string{"In Progress", "Waiting", "Dark mode", "Fix login bug", "my own name", "Other"} {
		if !strings.Contains(out, want) {
			t.Errorf("frame lacks %q:\n%s", want, out)
		}
	}

	// Back to rush's own grouping, the names are rush's again.
	m.cycleGroupBy()
	if m.store.Config.GroupBy != "status" {
		t.Fatalf("cycled to %q", m.store.Config.GroupBy)
	}
	for _, a := range m.snap.Agents {
		if a.SessionID == sid("aaaa1111") && a.DisplayName != "session a" {
			t.Fatalf("name kept outside the plugin's mode: %q", a.DisplayName)
		}
	}
	if m.folded(otherSection) {
		t.Fatal("Other folds only in a plugin's mode")
	}
}

func TestSidebarIgnoredWhenRevokedOrHosted(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	writeSession(t, "aaaa1111", "session a")
	sidebarPlugin(t, plugin.Sidebar{Plugin: "kanban", Title: "Kanban",
		Sections: []plugin.SidebarSection{{Title: "Waiting"}},
		Agents:   map[string]plugin.SidebarAgent{sid("aaaa1111"): {Name: "Fix login bug", Section: "Waiting"}}})
	store := state.Load()
	store.Config.GroupBy = "plugin:kanban"

	hosted := loaded(NewHosted(store, "test", "aaaa1111"))
	if hosted.activeSidebar() != nil || hosted.snap.Agents[0].DisplayName != "session a" {
		t.Fatal("hosted was arranged by the plugin")
	}

	m := loaded(New(store, "test"))
	if m.activeSidebar() == nil {
		t.Fatal("the plugin's mode is off")
	}
	// A stale file of a plugin no longer approved is not used.
	a := plugin.Approvals()
	delete(a, "kanban")
	b, _ := jsonx.Marshal(a)
	_ = os.WriteFile(filepath.Join(plugin.Root(), "approved.json"), b, 0o600)
	m.refreshNow()
	if m.activeSidebar() != nil || len(m.groupModes()) != len(groupModes) {
		t.Fatal("a revoked plugin still arranges the list")
	}
	for _, l := range m.lines {
		if l.kind == lineSection && (l.title == "Waiting" || l.title == otherSection) {
			t.Fatalf("plugin section %q shown after revoke", l.title)
		}
	}
}

func (m *Model) keyOf(sessionID string) string {
	for _, a := range m.snap.Agents {
		if a.SessionID == sessionID {
			return a.Key
		}
	}
	return ""
}
