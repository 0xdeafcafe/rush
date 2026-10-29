package ui

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// What an agent the session started edits shows in the session's Changes,
// marked with that agent's name.
func TestSpawnedEditsInChanges(t *testing.T) {
	kid := convo.New()
	now := time.Now()
	kid.Apply(host.Sent{Text: "add start_agent"}, now)
	call := tool.Call{ID: "e1", Name: "Edit", Kind: tool.Edit, Input: tool.Input{Path: "/w/internal/agtools/agents.go"}}
	kid.Apply(event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{{Kind: event.ToolCall, Call: &call}}}, now)
	out := tool.Output{CallID: "e1", Patches: []tool.Patch{{OldStart: 1, NewStart: 1, Lines: []string{" a", "+b"}}}}
	kid.Apply(event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult, Output: &out}}}, now)
	c := &hostConn{kind: "claude", key: "k", sess: convo.New(), spawns: map[string]*spawnRun{"t1": {path: "/x/kid.jsonl", sess: kid, sp: convo.Spawn{Name: "Kimi CLI"}}}}
	c.setHelpers()
	ch := c.sess.Changes()
	if len(ch) != 1 || ch[0].Path != "/w/internal/agtools/agents.go" || ch[0].Add != 1 || len(ch[0].By) != 1 || ch[0].By[0] != "Kimi CLI" {
		t.Fatalf("changes %+v", ch)
	}
}
