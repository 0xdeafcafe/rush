package plugind

import (
	"encoding/json/jsontext"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// TestBrokerRunsTheExamplePlugin builds plugins/examples/delegate, approves
// it, and talks to it as a session host would: Claude Code's MCP messages
// in, the plugin's replies out.
func TestBrokerRunsTheExamplePlugin(t *testing.T) {
	if testing.Short() || plugin.Supported() != nil {
		t.Skip("builds and sandboxes a program")
	}
	// Short, as a unix socket's path must be.
	home, err := os.MkdirTemp("/tmp", "rush-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(home) })
	home, _ = filepath.EvalSymlinks(home)
	t.Setenv("RUSH_HOME", home)
	dir := filepath.Join(plugin.Root(), "delegate")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	src := "../../plugins/examples/delegate"
	if out, err := exec.Command("go", "build", "-o", filepath.Join(dir, "delegate"), src).CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	m, _ := os.ReadFile(filepath.Join(src, "plugin.json"))
	if err := os.WriteFile(filepath.Join(dir, "plugin.json"), m, 0o600); err != nil {
		t.Fatal(err)
	}
	p, err := plugin.Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err := plugin.Approve(p); err != nil {
		t.Fatal(err)
	}

	done := make(chan error, 1)
	go func() { done <- Run() }()
	// Wait for it here: a Broker that can't reach it would start one, and
	// in a test the executable is the test binary.
	for i := 0; ; i++ {
		if c, err := plugin.DialBroker(); err == nil {
			c.Close()
			break
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
	var b plugin.Broker
	mcp := func(msg string) map[string]any {
		t.Helper()
		var out map[string]any
		if err := jsonx.Unmarshal(b.MCP("delegate", "sess1", jsontext.Value(msg)), &out); err != nil {
			t.Fatal(err)
		}
		return out
	}

	init := mcp(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}`)
	if info := init["result"].(map[string]any)["serverInfo"].(map[string]any); info["name"] != "rush-delegate" {
		t.Fatalf("initialize = %v", init)
	}
	if n := mcp(`{"jsonrpc":"2.0","method":"notifications/initialized"}`); n["result"] == nil {
		t.Fatalf("notification reply = %v", n)
	}
	list := mcp(`{"jsonrpc":"2.0","id":2,"method":"tools/list"}`)
	tools, _ := list["result"].(map[string]any)["tools"].([]any)
	if len(tools) != 4 {
		t.Fatalf("tools/list = %v", list)
	}
	text := func(r map[string]any) string {
		res, _ := r["result"].(map[string]any)
		c, _ := res["content"].([]any)
		if len(c) == 0 {
			return ""
		}
		return c[0].(map[string]any)["text"].(string)
	}
	if got := text(mcp(`{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_agents","arguments":{}}}`)); got != "No agents." {
		t.Fatalf("list_agents = %q", got)
	}
	// Sessions it didn't start are out of its reach, and ids can't be paths.
	for _, id := range []string{"someone-else", "../../plugin-data/delegate"} {
		r := mcp(`{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"message_agent","arguments":{"id":"` + id + `","text":"rm -rf ~"}}}`)
		if got := text(r); !strings.Contains(got, "no session") && !strings.Contains(got, "bad session id") {
			t.Fatalf("message_agent(%s) = %q", id, got)
		}
	}
	// Outside its workspaces, nothing starts.
	r := mcp(`{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"start_agent","arguments":{"cwd":"/tmp","prompt":"hi"}}}`)
	if got := text(r); !strings.Contains(got, "outside the plugin's workspaces") {
		t.Fatalf("start_agent in /tmp = %q", got)
	}

	// It is started again when it dies.
	st, err := Ask()
	if err != nil || len(st) != 1 || st[0].State != "running" {
		t.Fatalf("status = %+v, %v", st, err)
	}
	_ = syscall.Kill(st[0].PID, syscall.SIGKILL)
	deadline := time.Now().Add(10 * time.Second)
	for {
		st, _ = Ask()
		if len(st) == 1 && st[0].State == "running" && st[0].PID != 0 && st[0].Restarts == 1 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("not restarted: %+v", st)
		}
		time.Sleep(100 * time.Millisecond)
	}
	if got := text(mcp(`{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"list_agents","arguments":{}}}`)); got != "No agents." {
		t.Fatalf("after restart, list_agents = %q", got)
	}

	// Revoking the last plugin stops it, and the broker with it.
	if err := plugin.Revoke("delegate"); err != nil {
		t.Fatal(err)
	}
	if err := Reload(); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("broker still running with nothing approved")
	}
	if _, err := os.Stat(plugin.BrokerSock()); !os.IsNotExist(err) {
		t.Fatal("broker left its socket behind")
	}
}
