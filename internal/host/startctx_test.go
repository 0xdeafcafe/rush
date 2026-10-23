package host

import (
	"os"
	"slices"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// A restart compares what the agent was given with the start before: the
// first start has nothing to compare with, the next says what changed.
func TestRestartSaysWhatChanged(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	s := &server{cfg: Config{ID: "restart"}, clients: map[*conn]struct{}{}}
	if err := os.MkdirAll(dir(s.cfg.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	count := func(tools int) *usage.Context {
		return &usage.Context{Parts: []usage.ContextPart{{Name: "System tools", Tokens: tools}, {Name: "Messages", Tokens: 5000}}}
	}

	s.beginStart([]usage.Section{{Name: "rush's task list", Tokens: 300}})
	s.startInit(&event.Init{Tools: []string{"Bash", "Read"}})
	s.startCount(count(12000))
	if s.info.ContextChange != nil {
		t.Fatalf("a first start changed %+v", s.info.ContextChange)
	}

	s.beginStart([]usage.Section{{Name: "rush's task list", Tokens: 300}, {Name: "plugin ts", Tokens: 900}})
	s.startInit(&event.Init{Tools: []string{"Bash", "Read", "mcp__ts__def"}, MCP: []event.MCPServer{{Name: "ts"}}})
	c := s.info.ContextChange
	if c == nil || !slices.Equal(c.ServersAdded, []string{"ts"}) || !slices.Equal(c.ToolsAdded, []string{"mcp__ts__def"}) || len(c.Sections) != 1 || c.Parts != nil {
		t.Fatalf("after init %+v", c)
	}
	s.startCount(count(14000))
	s.startCount(count(99999)) // only the first count is the start's
	if c := s.info.ContextChange; len(c.Parts) != 1 || c.Parts[0] != (usage.Delta{Name: "System tools", Was: 12000, Now: 14000}) {
		t.Fatalf("after count %+v", c.Parts)
	}
}
