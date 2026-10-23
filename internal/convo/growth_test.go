package convo

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

func TestContextGrowthIsEachTurnsLastRequest(t *testing.T) {
	t0 := time.Date(2026, 10, 23, 9, 0, 0, 0, time.UTC)
	at := func(m int) time.Time { return t0.Add(time.Duration(m) * time.Minute) }
	sent := func(m int, in, read, write int64, run string) Request {
		return Request{At: at(m), Run: run, Usage: usage.TokenUsage{Input: in, CacheRead: read, CacheWrite1h: write}}
	}
	s := New()
	s.Turns = []*Turn{
		{N: 1, Start: at(0)},
		{N: 2, Start: at(10)},
		{N: 3, Start: at(20), Items: []*Item{{Kind: KCompact, Compact: &event.Compacted{Before: 150_000, After: 20_000}}}},
		{N: 4, Start: at(30)}, // a slash command: nothing sent
	}
	s.Requests = []Request{
		sent(1, 10, 0, 20_000, ""),
		sent(2, 5, 20_000, 3_000, ""),
		sent(11, 5, 23_000, 50_000, ""),
		sent(12, 5, 0, 900_000, "toolu_sub"), // a subagent's own context
		sent(21, 5, 0, 22_000, ""),
	}
	got := s.ContextGrowth()
	want := []ContextStep{{Turn: 1, Tokens: 23_005}, {Turn: 2, Tokens: 73_005}, {Turn: 3, Tokens: 22_005, Compacted: 150_000}}
	if len(got) != len(want) {
		t.Fatalf("steps %+v", got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("step %d is %+v, want %+v", i, got[i], want[i])
		}
	}

	lines := ContextGrowthLines(got, "", 40)
	if len(lines) != 3 {
		t.Fatalf("lines %q", lines)
	}
	if bars := []rune(stripANSI(lines[0])); len(bars) != 3 || bars[2] != '█' {
		t.Fatalf("bars %q: the compacted turn peaks at what it compacted from", stripANSI(lines[0]))
	}
	if c := stripANSI(lines[1]); c != "#1 23k → #3 22k · peak 150k" {
		t.Fatalf("caption %q", c)
	}
	if c := stripANSI(lines[2]); !strings.Contains(c, "compacted #3 from 150k") {
		t.Fatalf("compactions %q", c)
	}
}

func TestContextChangeLinesGroupAServersTools(t *testing.T) {
	c := &usage.Change{
		ToolsAdded:   []string{"WebFetch", "mcp__ts__def", "mcp__ts__refs"},
		ServersAdded: []string{"ts"},
		Sections:     []usage.Delta{{Name: "plugin ts", Now: 900, Added: true}},
		Parts:        []usage.Delta{{Name: "MCP tools", Was: 800, Now: 2400}},
	}
	got := stripANSI(strings.Join(ContextChangeLines(c, "Claude Code", "", 100, time.Now()), "\n"))
	for _, w := range []string{"+ MCP server ts · 2 tools", "+ 1 tool  WebFetch", "rush's prompt: plugin ts", "estimated", "as counted by Claude Code:", "800 → 2.4k +1.6k"} {
		if !strings.Contains(got, w) {
			t.Fatalf("no %q in\n%s", w, got)
		}
	}
	if strings.Contains(got, "mcp__ts__def") {
		t.Fatalf("a new server's tools are listed one by one:\n%s", got)
	}
	if ContextChangeLines(nil, "Claude Code", "", 100, time.Now()) != nil {
		t.Fatal("lines for no change")
	}
}
