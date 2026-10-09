package acp

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// Kimi's background AskUserQuestion returns "running" before it's
// answered: its question stands on its own, so the call's result doesn't
// close it, and it's no background task to nag about.
func TestKimiBackgroundQuestionOutlivesItsCall(t *testing.T) {
	s, f := start(t)
	nextOf[event.Init](t, s)
	if err := s.Send(agent.Input{Text: "go"}); err != nil {
		t.Fatal(err)
	}
	f.expect("session/prompt")
	in := `{"background":true,"questions":[{"question":"Tea?","options":[{"label":"Yes"},{"label":"No"}]}]}`
	f.update(map[string]any{"sessionUpdate": "tool_call", "toolCallId": "1:q", "title": "AskUserQuestion", "kind": "other", "status": "in_progress",
		"content": []any{map[string]any{"type": "content", "content": map[string]any{"type": "text", "text": in}}}})
	nextOf[event.Message](t, s) // the call
	f.request(7, "elicitation/create", map[string]any{"sessionId": "s1", "toolCallId": "1:q", "mode": "form", "message": "Tea?",
		"requestedSchema": map[string]any{"type": "object", "properties": map[string]any{
			"q0": map[string]any{"type": "string", "title": "Tea?", "oneOf": []any{map[string]any{"const": "Yes", "title": "Yes"}}}}}})
	if q := nextOf[event.Question](t, s); q.CallID != "" {
		t.Fatalf("background question tied to its call %q", q.CallID)
	}
	s.mu.Lock()
	kind := s.calls["1:q"].c.Kind
	s.mu.Unlock()
	if kind != tool.Question {
		t.Fatalf("kind %v", kind)
	}
	f.update(map[string]any{"sessionUpdate": "tool_call_update", "toolCallId": "1:q", "status": "completed",
		"content": []any{map[string]any{"type": "content", "content": map[string]any{"type": "text", "text": "task_id: question-x\nstatus: running"}}}})
	var task string
	for range 100 {
		s.mu.Lock()
		done := s.calls["1:q"].done
		task = s.calls["1:q"].task
		s.mu.Unlock()
		if done {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if task != "" {
		t.Fatalf("question started task %q", task)
	}
}
