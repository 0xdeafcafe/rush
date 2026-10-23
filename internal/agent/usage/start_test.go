package usage

import (
	"reflect"
	"testing"
)

func TestDiffSaysWhatChangedBetweenStarts(t *testing.T) {
	was := &Start{
		Tools:    []string{"Bash", "Read", "mcp__old__a"},
		Servers:  []string{"old"},
		Sections: []Section{{"rush's task list", 300}, {"plugin hex", 120}},
		Parts:    map[string]int{"System tools": 12000, "MCP tools": 800, "Skills": 500},
	}
	now := &Start{
		Tools:    []string{"Bash", "Read", "mcp__ts__def", "mcp__ts__refs"},
		Servers:  []string{"ts"},
		Sections: []Section{{"rush's task list", 340}, {"plugin ts", 900}},
		Parts:    map[string]int{"System tools": 12000, "MCP tools": 2400, "Skills": 0},
	}
	c := Diff(was, now)
	if c == nil {
		t.Fatal("no change found")
	}
	if !reflect.DeepEqual(c.ToolsAdded, []string{"mcp__ts__def", "mcp__ts__refs"}) || !reflect.DeepEqual(c.ToolsRemoved, []string{"mcp__old__a"}) {
		t.Fatalf("tools +%v -%v", c.ToolsAdded, c.ToolsRemoved)
	}
	if !reflect.DeepEqual(c.ServersAdded, []string{"ts"}) || !reflect.DeepEqual(c.ServersRemoved, []string{"old"}) {
		t.Fatalf("servers +%v -%v", c.ServersAdded, c.ServersRemoved)
	}
	// Biggest move first: the new plugin, the old one, then the task list.
	want := []Delta{{Name: "plugin ts", Now: 900, Added: true}, {Name: "plugin hex", Was: 120, Gone: true}, {Name: "rush's task list", Was: 300, Now: 340}}
	if !reflect.DeepEqual(c.Sections, want) {
		t.Fatalf("sections %+v", c.Sections)
	}
	// System tools held; Skills went to 0, which is a change, not a removal.
	wantParts := []Delta{{Name: "MCP tools", Was: 800, Now: 2400}, {Name: "Skills", Was: 500, Now: 0}}
	if !reflect.DeepEqual(c.Parts, wantParts) {
		t.Fatalf("parts %+v", c.Parts)
	}
}

func TestDiffComparesOnlyWhatBothStartsKnew(t *testing.T) {
	was := &Start{Tools: []string{"Bash"}, Sections: []Section{{"a", 10}}, Parts: map[string]int{"Memory": 100}}
	// Not yet told its tools, not yet counted: neither reads as removed.
	if c := Diff(was, &Start{Sections: []Section{{"a", 10}}}); c != nil {
		t.Fatalf("change %+v from a start not yet described", c)
	}
	if c := Diff(nil, &Start{Tools: []string{"Bash"}}); c != nil {
		t.Fatalf("change %+v on a first start", c)
	}
	if c := Diff(was, &Start{Tools: []string{"Bash"}, Sections: []Section{{"a", 10}}, Parts: map[string]int{"Memory": 100}}); c != nil {
		t.Fatalf("change %+v when nothing changed", c)
	}
}

func TestPartsOfLeavesOutTheConversationAndFreeSpace(t *testing.T) {
	u := &Context{Parts: []ContextPart{
		{Name: "System prompt", Tokens: 3000}, {Name: "Messages", Tokens: 40000},
		{Name: "MCP tools", Tokens: 900, Deferred: true}, {Name: "Autocompact buffer", Tokens: 33000, Kind: "buffer"},
		{Name: "Free space", Tokens: 100000, Kind: "free"},
	}}
	if got := PartsOf(u); !reflect.DeepEqual(got, map[string]int{"System prompt": 3000}) {
		t.Fatalf("parts %v", got)
	}
	if Estimate("1234567") != 2 {
		t.Fatalf("7 characters are %d tokens, want 2 at 3.5 a token", Estimate("1234567"))
	}
}
