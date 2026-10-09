package antigravity

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

func TestLaunchPreservesPermissionAndConversation(t *testing.T) {
	args, err := launchArgs(agent.StartOptions{Resume: true, SessionID: "saved", Model: "gemini-example", Mode: "plan", Effort: "high"})
	if err != nil {
		t.Fatal(err)
	}
	got := strings.Join(args, " ")
	if !strings.Contains(got, "--conversation saved") || !strings.Contains(got, "--mode plan") || strings.Contains(got, "skip-permissions") || strings.Contains(got, "experimental-acp") {
		t.Fatal(got)
	}
	for _, o := range []agent.StartOptions{{Mode: "yolo"}, {Mode: "autoEdit"}, {Fork: true}, {Resume: true}} {
		if _, err := launchArgs(o); err == nil {
			t.Fatalf("silently accepted incompatible options: %+v", o)
		}
	}
	args, err = launchArgs(agent.StartOptions{Mode: "always-proceed"})
	if err != nil || !strings.Contains(strings.Join(args, " "), "--dangerously-skip-permissions") {
		t.Fatal(args, err)
	}
}
func TestStreamTextToolsAndCumulativeUsage(t *testing.T) {
	var evs []event.Event
	p := parser{emit: func(e event.Event) bool { evs = append(evs, e); return true }}
	lines := []string{
		`{"event":"init","conversation_id":"session","init":{"cwd":"/work","model":"chosen"}}`,
		`{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"hello"}}`,
		`{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"agent_response","text_delta":" world"}}`,
		`{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"pwd"},"output":"/work"}}}`,
		`{"event":"result","result":{"status":"SUCCESS","response":"hello world","num_turns":1,"usage":{"input_tokens":100,"output_tokens":10,"cache_read_tokens":30}}}`,
	}
	for _, l := range lines {
		if _, err := p.line([]byte(l)); err != nil {
			t.Fatal(err)
		}
	}
	messages := 0
	tools := 0
	for _, ev := range evs {
		if m, ok := ev.(event.Message); ok {
			for _, part := range m.Parts {
				if part.Kind == event.Text {
					messages++
					if part.Text != "hello world" {
						t.Fatal(part.Text)
					}
				}
				if part.Call != nil {
					tools++
					if part.Call.Kind != tool.Shell || part.Call.Input.Command != "pwd" {
						t.Fatal(part.Call)
					}
				}
			}
		}
	}
	if messages != 1 || tools != 1 {
		t.Fatalf("duplicated or missing transcript rows: %+v", evs)
	}
	if p.end.Tokens.Input != 100 {
		t.Fatal(p.end)
	}
	p.line([]byte(`{"event":"result","result":{"status":"SUCCESS","response":"second","num_turns":2,"usage":{"input_tokens":120,"output_tokens":14,"cache_read_tokens":35}}}`))
	if p.end.Tokens.Input != 20 || p.end.Tokens.Output != 4 || p.end.Tokens.CacheRead != 5 {
		t.Fatalf("cumulative tokens double counted: %+v", p.end)
	}
}
func TestPersistentTurnsAndSavedHistory(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	bin := filepath.Join(t.TempDir(), "agy")
	script := `#!/bin/sh
printf '%s\n' '{"event":"init","conversation_id":"native-id","init":{"model":"test"}}'
i=0
while IFS= read -r input; do
 case "$input" in *'"event":"user"'*) ;; *) exit 9;; esac
 i=$((i+1))
 printf '{"event":"result","result":{"status":"SUCCESS","response":"reply","num_turns":%s,"usage":{"input_tokens":%s}}}\n' "$i" "$i"
done
`
	if err := os.WriteFile(bin, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	a, err := (Adapter{}).Start(ctx, agent.StartOptions{Binary: bin, SessionID: "provisional"})
	if err != nil {
		t.Fatal(err)
	}
	c := a.(*conn)
	defer c.Close()
	if err := c.Send(agent.Input{Images: []string{"x.png"}}); err == nil {
		t.Fatal("image silently dropped")
	}
	for _, text := range []string{"first", "second"} {
		if err := c.Send(agent.Input{Text: text}); err != nil {
			t.Fatal(err)
		}
		done := false
		for !done {
			select {
			case ev, ok := <-c.Events():
				if !ok {
					t.Fatal("process closed")
				}
				if end, ok := ev.(event.TurnEnd); ok {
					if end.Reason != "done" {
						t.Fatal(end)
					}
					done = true
				}
			case <-ctx.Done():
				t.Fatal("turn timed out")
			}
		}
	}
	if err := c.Close(); err != nil {
		t.Fatal(err)
	}
	evs, err := (Adapter{}).History(agent.Session{ID: "native-id"}, time.Time{})
	if err != nil {
		t.Fatal(err)
	}
	var prompts []string
	for _, ev := range evs {
		if m, ok := ev.(event.Message); ok && m.Role == "user" {
			prompts = append(prompts, m.Parts[0].Text)
		}
	}
	if !reflect.DeepEqual(prompts, []string{"first", "second"}) {
		t.Fatalf("lost prompts across init/resume: %v", prompts)
	}
	if _, cut, err := (Adapter{}).HistoryTail(agent.Session{ID: "native-id"}, 100); err != nil || !cut {
		t.Fatal(cut, err)
	}
	if historyPath("../elsewhere") != "" {
		t.Fatal("unsafe history path")
	}
}
func TestModelList(t *testing.T) {
	got := parseModels("Fetching available models...\ngemini-example  Gemini Example\nclaude-example Claude Example\n")
	if len(got) != 2 || got[0].ID != "gemini-example" {
		t.Fatal(got)
	}
}

// Headless, Antigravity denies a call it can't ask about and ends "SUCCESS"
// with nothing said: that's an error naming what it denied, not an empty answer.
func TestDeniedWithoutAskingIsAnError(t *testing.T) {
	results := 0
	p := parser{emit: func(e event.Event) bool {
		if m, ok := e.(event.Message); ok && m.Parts[0].Kind == event.ToolResult && m.Parts[0].Output.IsError {
			results++
		}
		return true
	}}
	var ended bool
	var err error
	// As agy sends it: an empty response step, then the call it denied.
	for _, l := range []string{
		`{"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"agent_response"}}`,
		`{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo ok"}}}}`,
		`{"event":"step_update","step_update":{"step_index":2,"state":"ERROR","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo ok"}}}}`,
		`{"event":"result","result":{"status":"SUCCESS","response":"","num_turns":1,"denied_actions":[{"action":"command","display_name":"RunCommand"}]}}`,
	} {
		if ended, err = p.line([]byte(l)); err != nil {
			t.Fatal(err)
		}
	}
	if !ended || p.end.Reason != "error" || !strings.Contains(p.end.Err, "RunCommand") {
		t.Fatalf("ended %v: %+v", ended, p.end)
	}
	if results != 1 {
		t.Errorf("the denied call got %d failed results, want 1", results)
	}
}
