package acp

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// fake is the agent's end of the pipes, driven by the test.
type fake struct {
	t   *testing.T
	in  *bufio.Reader
	out io.Writer
}

func (f *fake) recv() wire {
	f.t.Helper()
	line, err := f.in.ReadBytes('\n')
	if err != nil {
		f.t.Fatalf("agent read: %v", err)
	}
	var m wire
	if err := jsonx.Unmarshal(line, &m); err != nil {
		f.t.Fatalf("agent read %q: %v", line, err)
	}
	return m
}

// expect reads the next message and checks it's a call of method.
func (f *fake) expect(method string) wire {
	f.t.Helper()
	m := f.recv()
	if m.Method != method {
		f.t.Fatalf("agent got %q, want %q: %+v", m.Method, method, m)
	}
	return m
}

func (f *fake) send(v any) {
	f.t.Helper()
	b, _ := jsonx.Marshal(v)
	if _, err := f.out.Write(append(b, '\n')); err != nil {
		f.t.Fatalf("agent write: %v", err)
	}
}

func (f *fake) result(id jsontext.Value, result any) {
	f.send(map[string]any{"jsonrpc": "2.0", "id": id, "result": result})
}

func (f *fake) update(u map[string]any) {
	f.send(map[string]any{"jsonrpc": "2.0", "method": "session/update", "params": map[string]any{"sessionId": "s1", "update": u}})
}

func (f *fake) request(id int, method string, params any) {
	f.send(map[string]any{"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

func text(kind, s string) map[string]any {
	return map[string]any{"sessionUpdate": kind, "content": map[string]any{"type": "text", "text": s}}
}

// start connects a Session to a fake agent through initialize and
// session/new.
func start(t *testing.T) (*Session, *fake) { return open(t, "", nil) }

// open connects a Session to a fake agent through initialize and
// session/new, or session/load when resuming, which sends replay's
// updates before it answers.
func open(t *testing.T, resume string, replay func(f *fake)) (*Session, *fake) {
	t.Helper()
	// OS pipes, as an agent process has: buffered, so neither end waits
	// on the other to read.
	cr, aw, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	ar, cw, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	f := &fake{t: t, in: bufio.NewReader(ar), out: aw}
	type dialed struct {
		s   *Session
		err error
	}
	done := make(chan dialed, 1)
	go func() {
		s, err := Dial(context.Background(), cr, cw, Options{Dir: t.TempDir(), Adapter: "test", Resume: resume})
		done <- dialed{s, err}
	}()

	m := f.expect("initialize")
	var ip struct {
		ProtocolVersion    int `json:"protocolVersion"`
		ClientCapabilities struct {
			FS       map[string]bool `json:"fs"`
			Terminal bool            `json:"terminal"`
		} `json:"clientCapabilities"`
	}
	_ = jsonx.Unmarshal(m.Params, &ip)
	if ip.ProtocolVersion != 1 || !ip.ClientCapabilities.FS["readTextFile"] || !ip.ClientCapabilities.FS["writeTextFile"] || ip.ClientCapabilities.Terminal {
		t.Fatalf("initialize params: %s", m.Params)
	}
	f.result(m.ID, map[string]any{
		"protocolVersion":   1,
		"agentCapabilities": map[string]any{"loadSession": true, "promptCapabilities": map[string]any{"image": true}},
		"agentInfo":         map[string]any{"name": "fake", "version": "1.2.3"},
	})
	method := "session/new"
	if resume != "" {
		method = "session/load"
	}
	m = f.expect(method)
	var np struct {
		Cwd        string           `json:"cwd"`
		MCPServers []jsontext.Value `json:"mcpServers"`
		SessionID  string           `json:"sessionId"`
	}
	_ = jsonx.Unmarshal(m.Params, &np)
	if !filepath.IsAbs(np.Cwd) || np.MCPServers == nil || np.SessionID != resume {
		t.Fatalf("%s params: %s", method, m.Params)
	}
	if replay != nil {
		replay(f)
	}
	f.result(m.ID, map[string]any{
		"sessionId": "s1",
		"modes": map[string]any{"currentModeId": "default", "availableModes": []any{
			map[string]any{"id": "default", "name": "Default"}, map[string]any{"id": "plan", "name": "Plan"}}},
		"configOptions": []any{map[string]any{"type": "select", "id": "model", "name": "Model", "category": "model",
			"currentValue": "m1", "options": []any{map[string]any{"value": "m1", "name": "M1"}}}},
	})
	d := <-done
	if d.err != nil {
		t.Fatal(d.err)
	}
	t.Cleanup(func() { _ = d.s.Close(); _ = aw.Close(); _ = ar.Close() })
	return d.s, f
}

func next(t *testing.T, s *Session) event.Event {
	t.Helper()
	select {
	case ev, ok := <-s.Events():
		if !ok {
			t.Fatal("events closed")
		}
		return ev
	case <-time.After(3 * time.Second):
		t.Fatal("no event")
	}
	return nil
}

func nextOf[T event.Event](t *testing.T, s *Session) T {
	t.Helper()
	ev := next(t, s)
	v, ok := ev.(T)
	if !ok {
		t.Fatalf("got %T %+v, want %T", ev, ev, *new(T))
	}
	return v
}

func TestTurn(t *testing.T) {
	s, f := start(t)
	if got := nextOf[event.Init](t, s); got.SessionID != "s1" || got.Model != "m1" || got.Mode != "default" || got.Version != "1.2.3" {
		t.Fatalf("init: %+v", got)
	}
	f.update(map[string]any{"sessionUpdate": "available_commands_update", "availableCommands": []any{map[string]any{"name": "compact", "description": "x"}}})
	if got := nextOf[event.Init](t, s); !reflect.DeepEqual(got.Commands, []string{"compact"}) {
		t.Fatalf("commands: %+v", got)
	}

	if err := s.Send(agent.Input{Text: "hi"}); err != nil {
		t.Fatal(err)
	}
	prompt := f.expect("session/prompt")
	var pp struct {
		SessionID string         `json:"sessionId"`
		Prompt    []contentBlock `json:"prompt"`
	}
	_ = jsonx.Unmarshal(prompt.Params, &pp)
	if pp.SessionID != "s1" || len(pp.Prompt) != 1 || pp.Prompt[0].Text != "hi" {
		t.Fatalf("prompt: %s", prompt.Params)
	}

	f.update(text("agent_thought_chunk", "hmm"))
	f.update(text("agent_message_chunk", "Hel"))
	f.update(text("agent_message_chunk", "lo"))
	nextOf[event.MessageStart](t, s)
	if got := nextOf[event.PartStart](t, s); got != (event.PartStart{Index: 0, Kind: event.Thinking}) {
		t.Fatalf("part: %+v", got)
	}
	if got := nextOf[event.Delta](t, s); got != (event.Delta{Index: 0, Kind: event.Thinking, Text: "hmm"}) {
		t.Fatalf("delta: %+v", got)
	}
	nextOf[event.PartStart](t, s)
	nextOf[event.Delta](t, s)
	if got := nextOf[event.Delta](t, s); got != (event.Delta{Index: 1, Kind: event.Text, Text: "lo"}) {
		t.Fatalf("delta: %+v", got)
	}

	f.update(map[string]any{"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Edit a.go", "kind": "edit", "status": "pending",
		"locations": []any{map[string]any{"path": "/w/a.go"}},
		"content":   []any{map[string]any{"type": "diff", "path": "/w/a.go", "oldText": "a\nb\nc\n", "newText": "a\nB\nc\n"}}})
	whole := nextOf[event.Message](t, s)
	if whole.Role != "assistant" || len(whole.Parts) != 2 || whole.Parts[0].Text != "hmm" || whole.Parts[1].Text != "Hello" {
		t.Fatalf("whole message: %+v", whole)
	}
	callMsg := nextOf[event.Message](t, s)
	c := callMsg.Parts[0].Call
	if callMsg.Parts[0].Kind != event.ToolCall || c.ID != "c1" || c.Kind != tool.Edit || c.Title != "Edit a.go" || c.Input.Path != "/w/a.go" ||
		!reflect.DeepEqual(c.Input.Edits, []tool.Replace{{Old: "a\nb\nc\n", New: "a\nB\nc\n"}}) {
		t.Fatalf("call: %+v", c)
	}

	f.request(7, "session/request_permission", map[string]any{"sessionId": "s1", "toolCall": map[string]any{"toolCallId": "c1"},
		"options": []any{map[string]any{"optionId": "yes", "name": "Allow", "kind": "allow_once"},
			map[string]any{"optionId": "always", "name": "Always", "kind": "allow_always"},
			map[string]any{"optionId": "no", "name": "Reject", "kind": "reject_once"}}})
	ap := nextOf[event.Approval](t, s)
	if ap.ID != "7" || ap.Call.ID != "c1" || len(ap.Options) != 3 || ap.Options[1] != (event.Option{ID: "always", Label: "Always", Kind: event.AllowAlways}) {
		t.Fatalf("approval: %+v", ap)
	}
	if err := s.Answer(ap.ID, "yes"); err != nil {
		t.Fatal(err)
	}
	reply := f.recv()
	if string(reply.ID) != "7" || string(reply.Result) != `{"outcome":{"optionId":"yes","outcome":"selected"}}` {
		t.Fatalf("answer: %s %s", reply.ID, reply.Result)
	}

	f.update(map[string]any{"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress"})
	f.update(map[string]any{"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
		"content": []any{map[string]any{"type": "content", "content": map[string]any{"type": "text", "text": "edited"}},
			map[string]any{"type": "diff", "path": "/w/a.go", "oldText": "a\nb\nc\n", "newText": "a\nB\nc\n"}}})
	if u := nextOf[event.CallUpdated](t, s); u.Call.ID != "c1" || u.Call.Input.Path != "/w/a.go" {
		t.Fatalf("call updated: %+v", u)
	}
	res := nextOf[event.Message](t, s)
	out := res.Parts[0].Output
	want := tool.Patch{Path: "/w/a.go", OldStart: 2, OldLines: 1, NewStart: 2, NewLines: 1, Lines: []string{"-b", "+B"}}
	if res.Role != "user" || res.Parts[0].Kind != event.ToolResult || out.CallID != "c1" || out.Text != "edited" || out.IsError ||
		len(out.Patches) != 1 || !reflect.DeepEqual(out.Patches[0], want) {
		t.Fatalf("result: %+v", out)
	}

	f.update(map[string]any{"sessionUpdate": "plan", "entries": []any{
		map[string]any{"content": "one", "priority": "high", "status": "completed"},
		map[string]any{"content": "two", "priority": "low", "status": "in_progress"}}})
	if got := nextOf[event.Plan](t, s); !reflect.DeepEqual(got.Todos, []tool.TodoItem{{Label: "one", Status: "completed"}, {Label: "two", Status: "in_progress"}}) {
		t.Fatalf("plan: %+v", got)
	}
	f.update(map[string]any{"sessionUpdate": "usage_update", "used": 1200, "size": 200000, "cost": map[string]any{"amount": 0.5, "currency": "USD"}})
	if got := nextOf[event.Context](t, s); got != (event.Context{Tokens: 1200, Window: 200000}) {
		t.Fatalf("context: %+v", got)
	}

	f.update(text("agent_message_chunk", "done"))
	nextOf[event.MessageStart](t, s)
	nextOf[event.PartStart](t, s)
	nextOf[event.Delta](t, s)
	f.result(prompt.ID, map[string]any{"stopReason": "end_turn", "usage": map[string]any{"totalTokens": 30, "inputTokens": 20, "outputTokens": 10}})
	if got := nextOf[event.Message](t, s); got.Parts[0].Text != "done" {
		t.Fatalf("last message: %+v", got)
	}
	end := nextOf[event.TurnEnd](t, s)
	if end.Reason != "done" || end.Tokens.Input != 20 || end.Tokens.Output != 10 || end.Cost != 0.5 {
		t.Fatalf("turn end: %+v", end)
	}
}

func TestInterrupt(t *testing.T) {
	s, f := start(t)
	nextOf[event.Init](t, s)
	if err := s.Send(agent.Input{Text: "go"}); err != nil {
		t.Fatal(err)
	}
	prompt := f.expect("session/prompt")
	f.update(map[string]any{"sessionUpdate": "tool_call", "toolCallId": "c2", "title": "ls", "kind": "execute", "rawInput": map[string]any{"command": []any{"ls", "-la"}}})
	if got := nextOf[event.Message](t, s).Parts[0].Call; got.Kind != tool.Shell || got.Input.Command != "ls -la" {
		t.Fatalf("call: %+v", got)
	}
	f.request(8, "session/request_permission", map[string]any{"sessionId": "s1", "toolCall": map[string]any{"toolCallId": "c2"},
		"options": []any{map[string]any{"optionId": "no", "name": "No", "kind": "reject_always"}}})
	ap := nextOf[event.Approval](t, s)
	if err := s.Interrupt(); err != nil {
		t.Fatal(err)
	}
	f.expect("session/cancel")
	if reply := f.recv(); string(reply.ID) != "8" || string(reply.Result) != `{"outcome":{"outcome":"cancelled"}}` {
		t.Fatalf("cancelled answer: %s %s", reply.ID, reply.Result)
	}
	if got := nextOf[event.ApprovalCancelled](t, s); got.ID != ap.ID {
		t.Fatalf("cancelled: %+v", got)
	}
	f.update(map[string]any{"sessionUpdate": "tool_call_update", "toolCallId": "c2", "status": "failed", "rawOutput": "stopped"})
	if out := nextOf[event.Message](t, s).Parts[0].Output; !out.IsError || out.Text != "stopped" {
		t.Fatalf("failed result: %+v", out)
	}
	f.result(prompt.ID, map[string]any{"stopReason": "cancelled"})
	if got := nextOf[event.TurnEnd](t, s); got.Reason != "interrupted" {
		t.Fatalf("turn end: %+v", got)
	}
	if err := s.Answer(ap.ID, "no"); err == nil {
		t.Fatal("answered a withdrawn approval")
	}
}

func TestSettingsAndFiles(t *testing.T) {
	s, f := start(t)
	nextOf[event.Init](t, s)

	errc := make(chan error, 1)
	go func() { errc <- s.SetMode("plan") }()
	m := f.expect("session/set_mode")
	if string(m.Params) != `{"modeId":"plan","sessionId":"s1"}` {
		t.Fatalf("set_mode: %s", m.Params)
	}
	f.result(m.ID, map[string]any{})
	if err := <-errc; err != nil {
		t.Fatal(err)
	}
	if got := nextOf[event.Init](t, s); got.Mode != "plan" {
		t.Fatalf("mode: %+v", got)
	}
	if err := s.SetMode("nope"); err == nil {
		t.Fatal("set an unknown mode")
	}

	go func() { errc <- s.SetModel("m2") }()
	m = f.expect("session/set_config_option")
	if string(m.Params) != `{"configId":"model","sessionId":"s1","value":"m2"}` {
		t.Fatalf("set_config_option: %s", m.Params)
	}
	f.result(m.ID, map[string]any{"configOptions": []any{map[string]any{"type": "select", "id": "model", "name": "Model", "category": "model", "currentValue": "m2"}}})
	if err := <-errc; err != nil {
		t.Fatal(err)
	}
	if got := nextOf[event.Init](t, s); got.Model != "m2" {
		t.Fatalf("model: %+v", got)
	}

	p := filepath.Join(t.TempDir(), "f.txt")
	if err := os.WriteFile(p, []byte("one\ntwo\nthree\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	f.request(20, "fs/read_text_file", map[string]any{"sessionId": "s1", "path": p, "line": 2, "limit": 1})
	if r := f.recv(); string(r.Result) != `{"content":"two\n"}` {
		t.Fatalf("read: %s %+v", r.Result, r.Error)
	}
	f.request(21, "fs/write_text_file", map[string]any{"sessionId": "s1", "path": p, "content": "new"})
	if r := f.recv(); r.Error != nil {
		t.Fatalf("write: %+v", r.Error)
	}
	if b, _ := os.ReadFile(p); string(b) != "new" {
		t.Fatalf("wrote %q", b)
	}
	f.request(22, "terminal/create", map[string]any{"sessionId": "s1", "command": "ls"})
	if r := f.recv(); r.Error == nil || r.Error.Code != CodeMethodNotFound {
		t.Fatalf("terminal: %+v", r)
	}

	f.request(23, "elicitation/create", map[string]any{"sessionId": "s1", "mode": "form", "message": "Pick",
		"requestedSchema": map[string]any{"type": "object", "properties": map[string]any{
			"color": map[string]any{"type": "string", "title": "Colour", "oneOf": []any{map[string]any{"const": "r", "title": "Red"}, map[string]any{"const": "g", "title": "Green"}}},
			"ok":    map[string]any{"type": "boolean", "title": "Sure?"}}}})
	q := nextOf[event.Question](t, s)
	if q.ID != "23" || q.Title != "Pick" || len(q.Asks) != 2 || q.Asks[0].ID != "color" || len(q.Asks[0].Options) != 2 || q.Asks[0].Options[1].Label != "Green" {
		t.Fatalf("question: %+v", q)
	}
	if err := s.AnswerQuestion(q.ID, map[string][]string{"color": {"Green"}, "ok": {"Yes"}}); err != nil {
		t.Fatal(err)
	}
	if r := f.recv(); string(r.Result) != `{"action":"accept","content":{"color":"g","ok":true}}` {
		t.Fatalf("elicitation answer: %s", r.Result)
	}
}

func TestLoad(t *testing.T) {
	s, _ := open(t, "old", func(f *fake) {
		f.update(text("user_message_chunk", "fix it"))
		f.update(text("agent_message_chunk", "fixed"))
	})
	if got := nextOf[event.Message](t, s); got.Role != "user" || got.Parts[0].Text != "fix it" {
		t.Fatalf("user message: %+v", got)
	}
	nextOf[event.MessageStart](t, s)
	nextOf[event.PartStart](t, s)
	nextOf[event.Delta](t, s)
	if got := nextOf[event.Message](t, s); got.Role != "assistant" || got.Parts[0].Text != "fixed" {
		t.Fatalf("assistant message: %+v", got)
	}
	if got := nextOf[event.Init](t, s); got.SessionID != "old" {
		t.Fatalf("init: %+v", got)
	}
}

func TestStopReason(t *testing.T) {
	for in, want := range map[string]string{"end_turn": "done", "cancelled": "interrupted", "max_tokens": "max_tokens",
		"max_turn_requests": "max_turns", "refusal": "refusal"} {
		if got := stopReason(in); got != want {
			t.Errorf("stopReason(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestKindOf(t *testing.T) {
	for in, want := range map[string]tool.Kind{"read": tool.Read, "edit": tool.Edit, "delete": tool.Delete, "move": tool.Move,
		"search": tool.Search, "execute": tool.Shell, "think": tool.Think, "fetch": tool.Fetch, "other": tool.Other, "": tool.Other} {
		if got := kindOf(in); got != want {
			t.Errorf("kindOf(%q) = %v, want %v", in, got, want)
		}
	}
}

func TestReadInput(t *testing.T) {
	old := "x"
	tests := []struct {
		name    string
		kind    tool.Kind
		raw     string
		locs    []location
		content []toolContent
		want    tool.Input
	}{
		{"shell string", tool.Shell, `{"command":"go test","description":"Run tests"}`, nil, nil, tool.Input{Command: "go test", Description: "Run tests"}},
		{"read by location", tool.Read, `{}`, []location{{Path: "/a"}}, nil, tool.Input{Path: "/a"}},
		{"search query", tool.Search, `{"query":"foo","path":"/src"}`, nil, nil, tool.Input{Pattern: "foo", Path: "/src"}},
		{"fetch", tool.Fetch, `{"url":"https://x.dev"}`, nil, nil, tool.Input{URL: "https://x.dev"}},
		{"move", tool.Move, `{"source":"/a","destination":"/b"}`, nil, nil, tool.Input{Path: "/a", To: "/b"}},
		{"edit strings", tool.Edit, `{"file_path":"/a","old_string":"x","new_string":"y"}`, nil, nil, tool.Input{Path: "/a", Edits: []tool.Replace{{Old: "x", New: "y"}}}},
		{"diffs win", tool.Edit, `{"old_string":"q","new_string":"r"}`, nil,
			[]toolContent{{Type: "diff", Path: "/a", OldText: &old, NewText: "y"}, {Type: "diff", Path: "/b", NewText: "z"}},
			tool.Input{Path: "/a", Edits: []tool.Replace{{Old: "x", New: "y"}, {Path: "/b", New: "z"}}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := readInput(tt.kind, jsontext.Value(tt.raw), tt.locs, tt.content); !reflect.DeepEqual(got, tt.want) {
				t.Errorf("got %+v, want %+v", got, tt.want)
			}
		})
	}
}

func TestPatch(t *testing.T) {
	tests := []struct {
		old  *string
		new  string
		want tool.Patch
	}{
		{nil, "a\nb\n", tool.Patch{OldStart: 0, OldLines: 0, NewStart: 1, NewLines: 2, Lines: []string{"+a", "+b"}}},
		{ptr("a\nb\nc"), "a\nc", tool.Patch{OldStart: 2, OldLines: 1, NewStart: 1, NewLines: 0, Lines: []string{"-b"}}},
		{ptr("a"), "a\nb", tool.Patch{OldStart: 1, OldLines: 0, NewStart: 2, NewLines: 1, Lines: []string{"+b"}}},
	}
	for i, tt := range tests {
		if got := patch(toolContent{OldText: tt.old, NewText: tt.new}); !reflect.DeepEqual(got, tt.want) {
			t.Errorf("%d: got %+v, want %+v", i, got, tt.want)
		}
	}
}

func ptr(s string) *string { return &s }

func TestKnownAgentsRegister(t *testing.T) {
	for _, k := range []agent.Kind{"kimi", "opencode"} {
		a, ok := agent.Get(k)
		if !ok {
			t.Errorf("%s isn't registered", k)
			continue
		}
		if _, ok := a.(agent.Driver); !ok {
			t.Errorf("%s can't be driven", k)
		}
	}
	if _, err := (Agent{ID: "x", Command: "x"}).Start(context.Background(), agent.StartOptions{Fork: true}); err != ErrNoFork {
		t.Errorf("fork = %v, want ErrNoFork", err)
	}
}

func TestThinkingConfig(t *testing.T) {
	s, f := start(t)
	nextOf[event.Init](t, s)
	f.update(map[string]any{"sessionUpdate": "config_option_update", "configOptions": []any{map[string]any{"type": "select", "id": "thinking", "category": "thinking", "currentValue": "low"}}})
	if got := nextOf[event.Init](t, s); got.Effort != "low" {
		t.Fatalf("initial effort: %+v", got)
	}
	done := make(chan error, 1)
	go func() { done <- s.SetEffort("high") }()
	m := f.expect("session/set_config_option")
	if string(m.Params) != `{"configId":"thinking","sessionId":"s1","value":"high"}` {
		t.Fatalf("thinking request: %s", m.Params)
	}
	f.result(m.ID, map[string]any{"configOptions": []any{map[string]any{"type": "select", "id": "thinking", "category": "thinking", "currentValue": "high"}}})
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if got := nextOf[event.Init](t, s); got.Effort != "high" {
		t.Fatalf("updated effort: %+v", got)
	}
}
