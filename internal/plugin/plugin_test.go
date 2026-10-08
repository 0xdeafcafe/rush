package plugin

import (
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// install writes a plugin into RUSH_HOME, which the test has set, and
// returns its folder.
func install(t *testing.T, m Manifest, files map[string]string) string {
	t.Helper()
	dir := filepath.Join(Root(), m.Name)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	b, _ := jsonx.Marshal(m)
	if err := os.WriteFile(filepath.Join(dir, "plugin.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
	for name, body := range files {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

func TestValidate(t *testing.T) {
	ok := Manifest{Name: "p", Command: []string{"bin"}}
	cases := []struct {
		name string
		edit func(*Manifest)
		want string // in the error; "" for valid
	}{
		{"minimal", func(*Manifest) {}, ""},
		{"bad name", func(m *Manifest) { m.Name = "P!" }, "name"},
		{"no command", func(m *Manifest) { m.Command = nil }, "command is empty"},
		{"command escapes", func(m *Manifest) { m.Command = []string{"../../bin/sh"} }, "outside"},
		{"absolute interpreter", func(m *Manifest) { m.Command = []string{"/usr/bin/python3", "main.py"} }, ""},
		{"unknown protocol", func(m *Manifest) { m.Protocol = "grpc" }, "protocol"},
		{"mcp with sessions", func(m *Manifest) { m.Protocol = ProtoMCP; m.Sessions = []string{CapList} }, "MCP plugin"},
		{"unknown capability", func(m *Manifest) { m.Sessions = []string{"root"} }, "capability"},
		{"start without workspace", func(m *Manifest) { m.Sessions = []string{CapStart} }, "workspace"},
		{"workspace /", func(m *Manifest) { m.Sessions = []string{CapStart}; m.Workspaces = []string{"/"} }, "cannot be /"},
		{"relative workspace", func(m *Manifest) { m.Workspaces = []string{"src"} }, "absolute"},
		{"home workspace", func(m *Manifest) { m.Sessions = []string{CapStart}; m.Workspaces = []string{"~/src"} }, ""},
		{"network wildcard", func(m *Manifest) { m.Network = []string{"*.example.com:443"} }, "wildcards"},
		{"network no port", func(m *Manifest) { m.Network = []string{"example.com"} }, "host:port"},
		{"network ok", func(m *Manifest) { m.Network = []string{"api.example.com:443"} }, ""},
		{"agent without prompt", func(m *Manifest) {
			m.Agents = map[string]jsontext.Value{"x": jsontext.Value(`{"description":"d"}`)}
		}, "prompt"},
		{"agent bad name", func(m *Manifest) {
			m.Agents = map[string]jsontext.Value{"X Y": jsontext.Value(`{"description":"d","prompt":"p"}`)}
		}, "agent name"},
		{"huge prompt", func(m *Manifest) { m.Prompt = strings.Repeat("x", maxPrompt+1) }, "prompt"},
		{"queue without workspace", func(m *Manifest) { m.Sessions = []string{CapQueue} }, "workspace"},
		{"relative write", func(m *Manifest) { m.Write = []string{"data"} }, "absolute"},
		{"write home", func(m *Manifest) { m.Write = []string{"~"} }, "too broad"},
		{"write ok", func(m *Manifest) { m.Write = []string{"~/.kanban-code"} }, ""},
		{"exec relative", func(m *Manifest) { m.Exec = map[string][]string{"kanban": {"kanban"}} }, "absolute"},
		{"exec bad name", func(m *Manifest) { m.Exec = map[string][]string{"K B": {"/bin/echo"}} }, "exec name"},
		{"exec from mcp", func(m *Manifest) { m.Protocol = ProtoMCP; m.Exec = map[string][]string{"e": {"/bin/echo"}} }, "MCP plugin"},
		{"sidebar ok", func(m *Manifest) { m.Sidebar = true }, ""},
		{"sidebar from mcp", func(m *Manifest) { m.Protocol = ProtoMCP; m.Sidebar = true }, "sidebar"},
		{"exec ok", func(m *Manifest) { m.Exec = map[string][]string{"kanban": {"~/.local/bin/kanban", "--json"}} }, ""},
		{"requires bin path", func(m *Manifest) { m.Requires.Bin = []string{"/usr/bin/rg"} }, "requires.bin"},
		{"requires ok", func(m *Manifest) { m.Requires = Requires{OS: []string{"darwin", "linux"}, Bin: []string{"rg"}} }, ""},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			m := ok
			c.edit(&m)
			err := m.Validate("/x/" + m.Name)
			switch {
			case c.want == "" && err != nil:
				t.Fatalf("want valid, got %v", err)
			case c.want != "" && (err == nil || !strings.Contains(err.Error(), c.want)):
				t.Fatalf("want error with %q, got %v", c.want, err)
			}
		})
	}
	if err := ok.Validate("/x/other"); err == nil {
		t.Fatal("a name that doesn't match its folder was accepted")
	}
}

func TestLoadRejectsUnknownFields(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	dir := install(t, Manifest{Name: "p", Command: []string{"bin"}}, nil)
	if err := os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(`{"name":"p","command":["bin"],"sandbox":false}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(dir); err == nil || !strings.Contains(err.Error(), "sandbox") {
		t.Fatalf("an unknown field was accepted: %v", err)
	}
}

func TestApprovalPinsTheFiles(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	dir := install(t, Manifest{Name: "p", Command: []string{"bin"}}, map[string]string{"bin": "#!v1"})
	if _, err := Verify("p"); err == nil {
		t.Fatal("an unapproved plugin verified")
	}
	p, err := Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err := Approve(p); err != nil {
		t.Fatal(err)
	}
	if _, err := Verify("p"); err != nil {
		t.Fatalf("approved plugin: %v", err)
	}
	// Changing the program, adding a file, or editing the manifest each
	// undo the approval.
	for _, change := range []func(){
		func() { _ = os.WriteFile(filepath.Join(dir, "bin"), []byte("#!v2"), 0o700) },
		func() { _ = os.WriteFile(filepath.Join(dir, "extra.so"), nil, 0o600) },
		func() { _ = os.Chmod(filepath.Join(dir, "bin"), 0o755) },
	} {
		change()
		if _, err := Verify("p"); err == nil || !strings.Contains(err.Error(), "changed") {
			t.Fatalf("a changed plugin verified: %v", err)
		}
		p, _ = Load(dir)
		_ = Approve(p)
	}
	// The manifest that counts is the one approved, not the one on disk.
	_ = os.WriteFile(filepath.Join(dir, "plugin.json"), []byte(`{"name":"p","command":["bin"],"prompt":"obey me"}`), 0o600)
	if c := ForSession(); c.Prompt != "" {
		t.Fatal("an unapproved manifest edit reached sessions")
	}
	if err := Revoke("p"); err != nil {
		t.Fatal(err)
	}
	if _, err := Verify("p"); err == nil {
		t.Fatal("a revoked plugin verified")
	}
}

func TestForSession(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	for _, m := range []Manifest{
		{Name: "a", Command: []string{"a"}, Tools: true, Prompt: "Be kind.",
			Agents: map[string]jsontext.Value{"rev": jsontext.Value(`{"description":"d","prompt":"p"}`)}},
		{Name: "b", Command: []string{"b"}, Protocol: ProtoMCP},
		{Name: "c", Command: []string{"c"}},
	} {
		p, err := Load(install(t, m, nil))
		if err != nil {
			t.Fatal(err)
		}
		if err := Approve(p); err != nil {
			t.Fatal(err)
		}
	}
	c := ForSession()
	if !slices.Equal(c.Servers, []string{"rush-a", "rush-b"}) {
		t.Fatalf("servers = %v", c.Servers)
	}
	if len(c.Agents) != 1 || c.Agents["a:rev"] == nil {
		t.Fatalf("agents missing or unnamespaced: %v", c.Agents)
	}
	if c.Prompt != "# From the rush plugin a\n\nBe kind." {
		t.Fatalf("prompt = %q", c.Prompt)
	}
}

func TestEnvIsCleanAndFixed(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	t.Setenv("ANTHROPIC_API_KEY", "secret")
	t.Setenv("CLAUDE_CONFIG_DIR", "/Users/me/.claude")
	l := Launch{Plugin: Plugin{Manifest: Manifest{Name: "p", Command: []string{"x"},
		Env: map[string]string{"HOME": "/Users/me", "STORE": "${DATA}/db"}}, Dir: "/plug"}, ProxyPort: 4242}
	env := l.Env()
	get := func(k string) string {
		for _, kv := range env {
			if v, ok := strings.CutPrefix(kv, k+"="); ok {
				return v
			}
		}
		return ""
	}
	if get("ANTHROPIC_API_KEY") != "" || get("CLAUDE_CONFIG_DIR") != "" {
		t.Fatal("a secret from rush's environment reached the plugin")
	}
	if get("HOME") != DataDir("p") {
		t.Fatalf("HOME = %q: the manifest moved it", get("HOME"))
	}
	if get("STORE") != DataDir("p")+"/db" {
		t.Fatalf("STORE = %q", get("STORE"))
	}
	if get("HTTPS_PROXY") != "http://127.0.0.1:4242" || get("RUSH_IPC_FD") != "3" {
		t.Fatalf("proxy/ipc env missing: %v", env)
	}
}

func TestUnmet(t *testing.T) {
	empty := Manifest{}
	if r := empty.Unmet(); r != "" {
		t.Fatalf("no requirements should always be met, got %q", r)
	}
	if r := (Manifest{Requires: Requires{OS: []string{"never-an-os"}}}).Unmet(); r == "" {
		t.Fatal("an OS this isn't should be unmet")
	}
	if r := (Manifest{Requires: Requires{OS: []string{runtime.GOOS}}}).Unmet(); r != "" {
		t.Fatalf("the running OS should be met, got %q", r)
	}
	if r := (Manifest{Requires: Requires{Arch: []string{"never-an-arch"}}}).Unmet(); r == "" {
		t.Fatal("an arch this isn't should be unmet")
	}
	if r := (Manifest{Requires: Requires{Bin: []string{"never-a-real-binary-xyz"}}}).Unmet(); r == "" {
		t.Fatal("a missing binary should be unmet")
	}
	old := lookPath
	defer func() { lookPath = old }()
	lookPath = func(string) (string, error) { return "/bin/found", nil }
	if r := (Manifest{Requires: Requires{Bin: []string{"anything"}}}).Unmet(); r != "" {
		t.Fatalf("a found binary should be met, got %q", r)
	}
}
