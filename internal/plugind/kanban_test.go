package plugind

import (
	"encoding/json/jsontext"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// TestBrokerRunsTheKanbanExample runs plugins/examples/kanban, sandboxed,
// against a kanban-code board and CLI of its own: Claude reads the board and
// its card, the plugin has kanban-code link the card to the agent it
// started for it, and a message for the card reaches that agent.
func TestBrokerRunsTheKanbanExample(t *testing.T) {
	if testing.Short() || plugin.Supported() != nil {
		t.Skip("builds and sandboxes a program")
	}
	home, err := os.MkdirTemp("/tmp", "rush-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(home) })
	home, _ = filepath.EvalSymlinks(home)
	t.Setenv("RUSH_HOME", home)

	// kanban-code: a board with one card, and a CLI that notes what it's
	// asked.
	kb := filepath.Join(home, "kb")
	_ = os.MkdirAll(kb, 0o700)
	_ = os.WriteFile(filepath.Join(kb, "links.json"), []byte(`{"links": [{"id": "card_bbb222", "column": "backlog",
		"projectPath": "`+home+`", "sessionLink": {"sessionId": "s-9"}, "issueLink": {"number": 12, "title": "Dark mode", "body": "Please."},
		"manualOverrides": {}, "manuallyArchived": false, "source": "githubIssue", "isRemote": false,
		"createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-01T00:00:00Z"}]}`), 0o600)
	cli := filepath.Join(home, "bin", "kanban")
	_ = os.MkdirAll(filepath.Dir(cli), 0o700)
	_ = os.WriteFile(cli, []byte("#!/bin/sh\necho \"$@\" >> "+kb+"/calls\necho '{}'\n"), 0o700)

	// An agent the plugin started for the card, as its host publishes it.
	writeInfo(t, host.Info{ID: "k1", SessionID: "s-9", Cwd: home, State: "idle", HostPID: os.Getpid(),
		StartedBy: "kanban", Meta: map[string]string{"card": "card_bbb222"}})

	dir := filepath.Join(plugin.Root(), "kanban")
	_ = os.MkdirAll(dir, 0o700)
	if out, err := exec.Command("go", "build", "-o", filepath.Join(dir, "kanban"), "../../plugins/examples/kanban").CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	m, _ := jsonx.Marshal(plugin.Manifest{Name: "kanban", Command: []string{"kanban"}, Tools: true,
		Sidebar: true, Sessions: []string{plugin.CapList, plugin.CapStart, plugin.CapQueue}, Workspaces: []string{home},
		Read: []string{kb}, Env: map[string]string{"KANBAN_CODE_HOME": kb}, Exec: map[string][]string{"kanban": {cli}}})
	_ = os.WriteFile(filepath.Join(dir, "plugin.json"), m, 0o600)
	p, err := plugin.Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err := plugin.Approve(p); err != nil {
		t.Fatal(err)
	}
	done := runBroker(t)

	var b plugin.Broker
	call := func(tool, args string) string {
		t.Helper()
		var out struct {
			Result struct {
				Content []struct{ Text string }
			}
		}
		msg := `{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"` + tool + `","arguments":` + args + `}}`
		if err := jsonx.Unmarshal(b.MCP("kanban", "k1", jsontext.Value(msg)), &out); err != nil || len(out.Result.Content) == 0 {
			t.Fatalf("%s: %v %+v", tool, err, out)
		}
		return out.Result.Content[0].Text
	}
	if got := call("board", "{}"); !strings.Contains(got, "## Backlog\n- card_bbb222  Dark mode") {
		t.Fatalf("board = %q", got)
	}
	// The card comes from the meta rush hands along with the call.
	if got := call("my_card", "{}"); !strings.Contains(got, "Issue #12: Dark mode") {
		t.Fatalf("my_card = %q", got)
	}
	if got := call("start_card", `{"card": "card_bbb222"}`); !strings.Contains(got, "already being worked on") {
		t.Fatalf("start_card on a card with an agent = %q", got)
	}
	// Reaches the agent: it isn't really running, so rush says so.
	if got := call("card_message", `{"card": "dark mode", "text": "hi"}`); !strings.Contains(got, "k1 is not running") {
		t.Fatalf("card_message = %q", got)
	}

	// Watching, it saw the agent's conversation, and linked the card to it.
	deadline := time.Now().Add(10 * time.Second)
	for {
		b, _ := os.ReadFile(filepath.Join(kb, "calls"))
		if strings.Contains(string(b), "relink card_bbb222 s-9 --json") {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("kanban relink never ran; calls: %q", b)
		}
		time.Sleep(100 * time.Millisecond)
	}

	// It arranged rush's list as the board.
	for {
		b, _ := os.ReadFile(plugin.SidebarPath("kanban"))
		if strings.Contains(string(b), `"s-9":{"name":"Dark mode","section":"Backlog"`) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("no sidebar; have %q", b)
		}
		time.Sleep(100 * time.Millisecond)
	}

	if err := plugin.Revoke("kanban"); err != nil {
		t.Fatal(err)
	}
	_ = Reload()
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("broker still running with nothing approved")
	}
	if _, err := os.Stat(plugin.SidebarPath("kanban")); !os.IsNotExist(err) {
		t.Fatal("revoked, its sidebar stayed")
	}
}

// runBroker runs the broker until nothing is approved, and waits for it to
// listen.
func runBroker(t *testing.T) <-chan error {
	t.Helper()
	done := make(chan error, 1)
	go func() { done <- Run() }()
	for i := 0; ; i++ {
		if c, err := plugin.DialBroker(); err == nil {
			c.Close()
			return done
		}
		select {
		case err := <-done:
			t.Fatalf("broker exited: %v", err)
		case <-time.After(20 * time.Millisecond):
		}
		if i > 250 {
			t.Fatal("broker never listened")
		}
	}
}
