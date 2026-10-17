package event

import (
	"reflect"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

func TestRoundTrip(t *testing.T) {
	exit := 2
	at := time.Date(2026, 9, 27, 12, 0, 0, 0, time.UTC)
	for _, ev := range []Event{
		Exchange{ID: "x", Direction: "sent", Phase: "message", Text: "hello", Sender: Peer{SessionID: "a", Kind: "codex"}, Receiver: Peer{SessionID: "b", Kind: "kimi"}},
		Init{SessionID: "s", Model: "m", Commands: []string{"compact"}, MCP: []MCPServer{{Name: "a", Status: "connected"}}},
		Delta{Index: 1, Kind: Thinking, Text: "hm"},
		Message{Role: "assistant", Parts: []Part{
			{Kind: Text, Text: "hi"},
			{Kind: ToolCall, Call: &tool.Call{ID: "c", Name: "Bash", Kind: tool.Shell, Input: tool.Input{Command: "ls"}}},
			{Kind: ToolResult, Output: &tool.Output{CallID: "c", Exit: &exit, Patches: []tool.Patch{{Lines: []string{"+a"}}}}},
		}, Tokens: &usage.TokenUsage{Input: 3}},
		Approval{ID: "a", Options: []Option{{ID: "allow", Kind: AllowOnce}}},
		Permission{ID: "p", CallID: "c", Tool: "Bash", Summary: "cd a && rm -rf ../../b/*", Decision: "allowed", At: at},
		Question{ID: "q", Asks: []Ask{{Text: "?", Options: []Choice{{Label: "A"}}}}},
		TurnEnd{Reason: "done", Duration: time.Second},
		Quota{usage.Quota{Windows: []usage.Window{{ID: "five_hour", Percent: 12, ResetsAt: at}}}},
		Limited{Window: "five_hour", ResetsAt: at},
		Billing{Billing: usage.Overage},
		StartNotice{Event: "SessionStart", Text: "loaded project guidance"},
		Plan{Todos: []tool.TodoItem{{Label: "x", Status: "pending"}}},
		Other{Adapter: "codex", Type: "x", Raw: []byte(`{"a":1}`)},
	} {
		b, err := Marshal(ev)
		if err != nil {
			t.Fatal(err)
		}
		got, err := Unmarshal(b)
		if err != nil {
			t.Fatalf("%s: %v", b, err)
		}
		if !reflect.DeepEqual(got, ev) {
			t.Errorf("round trip of %T:\n got %#v\nwant %#v", ev, got, ev)
		}
	}
}

func TestEveryEventHasAName(t *testing.T) {
	if len(names) != 28 {
		t.Errorf("%d events have names; one added to event.go needs one in codec.go", len(names))
	}
}
