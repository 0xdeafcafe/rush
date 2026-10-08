package codex

import (
	"context"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// A spawned thread is a task from its spawn until its turn ends, and again
// while a message it was given runs; the agent tools around it draw as
// Claude Code's of the same use. Shapes as Codex 0.155's app-server sends.
func TestSpawnedThreadIsATask(t *testing.T) {
	c := newConn(context.Background())
	c.thread = "root"
	note := func(method string, params map[string]any) []event.Event {
		b, _ := jsonx.Marshal(params)
		return c.notification(message{Method: method, Params: b})
	}
	item := func(kind string) map[string]any {
		return map[string]any{"threadId": "root", "item": map[string]any{"type": "subAgentActivity", "id": "call_" + kind,
			"kind": kind, "agentThreadId": "kid", "agentPath": "/root/echoer"}}
	}
	turn := func(id, status string) map[string]any {
		return map[string]any{"threadId": "kid", "turn": map[string]any{"id": id, "status": status}}
	}
	var started, done []string
	var calls []tool.Call
	take := func(evs []event.Event) {
		for _, ev := range evs {
			switch e := ev.(type) {
			case event.TaskStarted:
				started = append(started, e.ID+"/"+e.CallID)
			case event.TaskDone:
				done = append(done, e.ID+"/"+e.Status)
			case event.Message:
				for _, p := range e.Parts {
					if p.Call != nil {
						calls = append(calls, *p.Call)
					}
				}
			}
		}
	}
	take(note("item/started", item("started")))
	take(note("turn/started", turn("t1", "inProgress")))
	take(note("turn/completed", turn("t1", "completed")))
	take(note("item/started", item("interacted")))
	take(note("turn/started", turn("t2", "inProgress")))
	if _, turnID, ok := c.child("call_started"); !ok || turnID != "t2" {
		t.Fatalf("child by its call: turn %q %v", turnID, ok)
	}
	take(note("item/started", item("interrupted")))
	take(note("turn/completed", turn("t2", "interrupted")))
	take(note("item/started", item("completed")))

	if want := []string{"kid/call_started", "kid/call_started"}; len(started) != 2 || started[0] != want[0] || started[1] != want[1] {
		t.Errorf("started %v", started)
	}
	if len(done) != 2 || done[0] != "kid/completed" || done[1] != "kid/stopped" {
		t.Errorf("done %v", done)
	}
	var names []string
	for _, c := range calls {
		names = append(names, c.Name)
	}
	if len(calls) != 3 || calls[0].Kind != tool.Subagent || calls[0].Input.Child != "kid" || calls[0].Input.Description != "echoer" ||
		calls[1].Name != "SendMessage" || string(calls[1].Raw) != `{"to":"echoer"}` || calls[2].Name != "TaskStop" {
		t.Errorf("calls %v", names)
	}
	if bg := c.background(); len(bg.Tasks) != 0 {
		t.Errorf("still running %v", bg.Tasks)
	}
}

func TestSpawnedThreadReportsProgress(t *testing.T) {
	c := newConn(context.Background())
	c.thread = "root"
	c.adopt("kid", "spawn", "worker")
	note := func(method string, params map[string]any) []event.Event {
		b, _ := jsonx.Marshal(params)
		return c.notification(message{Method: method, Params: b})
	}
	tokenEvents := note("thread/tokenUsage/updated", map[string]any{"threadId": "kid", "tokenUsage": map[string]any{
		"last": map[string]any{"inputTokens": 1200, "cachedInputTokens": 1000},
	}})
	toolEvents := note("item/started", map[string]any{"threadId": "kid", "item": map[string]any{
		"type": "commandExecution", "id": "tool-1", "command": "rg TODO",
	}})
	var progress []event.TaskProgress
	for _, evs := range [][]event.Event{tokenEvents, toolEvents} {
		for _, ev := range evs {
			if p, ok := ev.(event.TaskProgress); ok {
				progress = append(progress, p)
			}
		}
	}
	if len(progress) != 2 || progress[0].Tokens != 1200 || progress[1].ToolUses != 1 || progress[1].LastTool == "" {
		t.Fatalf("progress = %+v", progress)
	}
}

// From a rollout, the agent tools read as Claude Code's, an encrypted
// message left out.
func TestAgentToolsFromRollout(t *testing.T) {
	for name, want := range map[string]string{
		"send_message":    `SendMessage {"message":"","to":"echoer"}`,
		"followup_task":   `SendMessage {"message":"","summary":"a follow-up task","to":"echoer"}`,
		"interrupt_agent": `TaskStop {"task_id":"echoer"}`,
		"wait_agent":      `TaskOutput {"task_id":"its subagents"}`,
		"list_agents":     `ListAgents {}`,
	} {
		c := responseCall(responseItem{Type: "function_call", Name: name, Namespace: "collaboration", CallID: "c",
			Arguments: `{"target":"echoer","message":"gAAAAABqvggt"}`})
		if got := c.Name + " " + string(c.Raw); got != want {
			t.Errorf("%s: %s, want %s", name, got, want)
		}
	}
}
