package plugind

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"log"
	"os"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func TestSidebarSet(t *testing.T) {
	r, p := testPlugin(t, plugin.Manifest{})
	r.p = p
	ctx := context.Background()
	set := func(params string) error {
		_, err := r.fromPlugin(ctx, "sidebar.set", jsontext.Value(params))
		return err
	}
	board := `{"title": "Kanban", "sections": [{"title": "In Progress"}],
		"agents": {"8c76706f-1c00-4aed-9c6d-7509f3033943": {"name": "Fix \u001b[1mlogin", "section": "In Progress"}}}`

	if err := set(board); !denied(err) {
		t.Fatalf("set without the capability: %v", err)
	}
	if _, err := os.Stat(plugin.SidebarPath("kanban")); !os.IsNotExist(err) {
		t.Fatal("a refused sidebar was written")
	}

	r.p.Sidebar = true
	var e *plugin.Error
	if err := set(`{"sections": [{"title": "A"}], "agents": {"s1": {"section": "B"}}}`); err == nil || !errors.As(err, &e) || e.Code != plugin.CodeInvalidParams {
		t.Fatalf("an agent in no section: %v", err)
	}
	if err := set(board); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(plugin.SidebarPath("kanban"))
	if err != nil {
		t.Fatal(err)
	}
	var got plugin.Sidebar
	_ = jsonx.Unmarshal(b, &got)
	if got.Plugin != "kanban" || got.UpdatedAt.IsZero() || got.Agents["8c76706f-1c00-4aed-9c6d-7509f3033943"].Name != "Fix login" {
		t.Fatalf("written = %s", b)
	}

	if err := set(`{}`); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(plugin.SidebarPath("kanban")); !os.IsNotExist(err) {
		t.Fatal("clearing left the file")
	}

	// Revoked, the broker's reload removes what it had set.
	if err := set(board); err != nil {
		t.Fatal(err)
	}
	b2 := &broker{log: log.New(os.Stderr, "", 0), plugins: map[string]*runner{}, quit: make(chan struct{})}
	b2.reload()
	if _, err := os.Stat(plugin.SidebarPath("kanban")); !os.IsNotExist(err) {
		t.Fatal("a plugin no longer approved kept its sidebar")
	}
}
