package codex

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"reflect"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// fake is an app-server on the far end of two pipes.
type fake struct {
	t   *testing.T
	in  *bufio.Scanner
	out io.Writer
}

// line is the next line rush sends, answering the account and rate-limit
// reads a new thread makes on its own with an error.
func (f *fake) line() []byte {
	f.t.Helper()
	for f.in.Scan() {
		var m message
		if err := jsonx.Unmarshal(f.in.Bytes(), &m); err != nil {
			f.t.Fatalf("bad line %q: %v", f.in.Text(), err)
		}
		if m.Method == "account/rateLimits/read" || m.Method == "account/read" {
			f.write(map[string]any{"id": m.ID, "error": map[string]any{"code": -32000, "message": "not signed in"}})
			continue
		}
		return f.in.Bytes()
	}
	f.t.Fatal("pipe closed")
	return nil
}

// expect reads the next message rush sends, which must be method.
func (f *fake) expect(method string) message {
	f.t.Helper()
	b := f.line()
	var m message
	_ = jsonx.Unmarshal(b, &m)
	if m.Method != method {
		f.t.Fatalf("got %s, want %s", b, method)
	}
	return m
}

func (f *fake) write(v any) {
	f.t.Helper()
	b, _ := jsonx.Marshal(v)
	if _, err := f.out.Write(append(b, '\n')); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fake) respond(id jsontext.Value, result any) {
	f.write(map[string]any{"id": id, "result": result})
}

func (f *fake) notify(method string, params any) {
	f.write(map[string]any{"method": method, "params": params, "emittedAtMs": 1})
}

func (f *fake) request(id any, method string, params any) {
	f.write(map[string]any{"id": id, "method": method, "params": params})
}

// start runs a Conn against a fake through the handshake and thread/start.
func start(t *testing.T, o agent.StartOptions) (*Conn, *fake, message) {
	t.Helper()
	cr, fw := io.Pipe() // fake → conn
	fr, cw := io.Pipe() // conn → fake
	f := &fake{t: t, in: bufio.NewScanner(fr), out: fw}
	f.in.Buffer(nil, 1<<20)
	c := newConn(context.Background())
	rpc := newClient(cr, cw, c.handle)
	errc := make(chan error, 1)
	go func() { errc <- c.begin(rpc, o) }()

	init := f.expect("initialize")
	f.respond(init.ID, map[string]any{"userAgent": "rush/0.155.1 (Mac OS 27.0.0; arm64) test", "codexHome": "/x", "platformFamily": "unix", "platformOs": "macos"})
	f.expect("initialized")
	method := "thread/start"
	if o.Resume {
		method = "thread/resume"
	}
	ts := f.expect(method)
	f.notify("thread/started", map[string]any{"thread": map[string]any{"id": "th1"}})
	f.respond(ts.ID, map[string]any{"thread": map[string]any{"id": "th1"}, "model": "gpt-5.5", "cwd": "/work", "approvalPolicy": "on-request"})
	if err := <-errc; err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = c.Close(); _ = fw.Close() })
	return c, f, init
}

// next is the next event the Conn sends.
func next(t *testing.T, c *Conn) event.Event {
	t.Helper()
	select {
	case e, ok := <-c.Events():
		if !ok {
			t.Fatal("events closed")
		}
		return e
	case <-time.After(2 * time.Second):
		t.Fatal("no event")
	}
	return nil
}

func params(t *testing.T, m message) map[string]any {
	t.Helper()
	var p map[string]any
	if err := jsonx.Unmarshal(m.Params, &p); err != nil {
		t.Fatal(err)
	}
	return p
}

func TestHandshake(t *testing.T) {
	c, _, init := start(t, agent.StartOptions{Dir: "/work", Model: "gpt-5.5", Mode: "auto"})
	p := params(t, init)
	if name := p["clientInfo"].(map[string]any)["name"]; name != "rush" {
		t.Errorf("clientInfo.name = %v", name)
	}
	got := next(t, c)
	want := event.Init{SessionID: "th1", Model: "gpt-5.5", Cwd: "/work", Mode: "auto", Version: "0.155.1"}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("init = %#v, want %#v", got, want)
	}
}

func TestThreadStartParams(t *testing.T) {
	cr, fw := io.Pipe()
	fr, cw := io.Pipe()
	f := &fake{t: t, in: bufio.NewScanner(fr), out: fw}
	c := newConn(context.Background())
	defer c.Close()
	go func() {
		_ = c.begin(newClient(cr, cw, c.handle), agent.StartOptions{Dir: "/w", Model: "m", Mode: "full-access", SessionID: "old", Resume: true})
	}()
	f.respond(f.expect("initialize").ID, map[string]any{"userAgent": "rush/1"})
	f.expect("initialized")
	p := params(t, f.expect("thread/resume"))
	want := map[string]any{"threadId": "old", "cwd": "/w", "model": "m", "approvalPolicy": "never", "sandbox": "danger-full-access", "excludeTurns": true}
	if !reflect.DeepEqual(p, want) {
		t.Errorf("thread/resume params = %v, want %v", p, want)
	}
}

func TestTurn(t *testing.T) {
	c, f, _ := start(t, agent.StartOptions{Effort: "high"})
	next(t, c) // Init
	_ = c.SetModel("gpt-5.6")
	_ = c.SetMode("read-only")

	sent := make(chan error, 1)
	go func() { sent <- c.Send(agent.Input{Text: "hi", Images: []string{"/tmp/a.png"}}) }()
	ts := f.expect("turn/start")
	p := params(t, ts)
	if p["model"] != "gpt-5.6" || p["effort"] != "high" || p["approvalPolicy"] != "on-request" || p["sandboxPolicy"].(map[string]any)["type"] != "readOnly" {
		t.Errorf("turn/start overrides = %v", p)
	}
	in := p["input"].([]any)
	if len(in) != 2 || in[0].(map[string]any)["text"] != "hi" || in[1].(map[string]any)["type"] != "localImage" {
		t.Errorf("input = %v", in)
	}
	f.notify("turn/started", map[string]any{"threadId": "th1", "turn": map[string]any{"id": "tu1", "status": "inProgress"}})
	f.respond(ts.ID, map[string]any{"turn": map[string]any{"id": "tu1"}})
	if err := <-sent; err != nil {
		t.Fatal(err)
	}
	ids := map[string]any{"threadId": "th1", "turnId": "tu1"}
	with := func(kv map[string]any) map[string]any {
		out := map[string]any{}
		for k, v := range ids {
			out[k] = v
		}
		for k, v := range kv {
			out[k] = v
		}
		return out
	}
	f.notify("item/started", with(map[string]any{"item": map[string]any{"type": "reasoning", "id": "r1", "summary": []string{}, "content": []string{}}}))
	f.notify("item/reasoning/summaryTextDelta", with(map[string]any{"itemId": "r1", "delta": "hmm", "summaryIndex": 0}))
	f.notify("item/completed", with(map[string]any{"item": map[string]any{"type": "reasoning", "id": "r1", "summary": []string{"hmm"}, "content": []string{}}}))
	f.notify("item/agentMessage/delta", with(map[string]any{"itemId": "m1", "delta": "Hel"}))
	f.notify("item/agentMessage/delta", with(map[string]any{"itemId": "m1", "delta": "lo"}))
	f.notify("item/completed", with(map[string]any{"item": map[string]any{"type": "agentMessage", "id": "m1", "text": "Hello"}}))
	f.notify("turn/plan/updated", with(map[string]any{"explanation": nil, "plan": []any{map[string]any{"step": "Read", "status": "completed"}, map[string]any{"step": "Write", "status": "inProgress"}}}))
	f.notify("thread/tokenUsage/updated", with(map[string]any{"tokenUsage": map[string]any{
		"total":              map[string]any{"totalTokens": 1500},
		"last":               map[string]any{"totalTokens": 1500, "inputTokens": 1200, "cachedInputTokens": 1000, "cacheWriteInputTokens": 0, "outputTokens": 300, "reasoningOutputTokens": 100},
		"modelContextWindow": 272000}}))
	f.notify("turn/completed", map[string]any{"threadId": "th1", "turn": map[string]any{"id": "tu1", "status": "completed", "error": nil, "durationMs": 2500}})

	want := []event.Event{
		event.Status{Busy: true},
		event.MessageStart{ID: "r1", Model: "gpt-5.6"},
		event.PartStart{Index: 0, Kind: event.Thinking},
		event.Delta{Index: 0, Kind: event.Thinking, Text: "hmm"},
		event.Message{Role: "assistant", ID: "r1", Model: "gpt-5.6", Parts: []event.Part{{Kind: event.Thinking, Text: "hmm"}}},
		event.MessageStart{ID: "m1", Model: "gpt-5.6"},
		event.PartStart{Index: 0, Kind: event.Text},
		event.Delta{Index: 0, Kind: event.Text, Text: "Hel"},
		event.Delta{Index: 0, Kind: event.Text, Text: "lo"},
		event.Message{Role: "assistant", ID: "m1", Model: "gpt-5.6", Parts: []event.Part{{Kind: event.Text, Text: "Hello"}}},
		event.Plan{Todos: []tool.TodoItem{{Label: "Read", Status: "completed"}, {Label: "Write", Status: "in_progress"}}},
		event.Context{Tokens: 1500, Window: 272000},
		event.TurnEnd{Reason: "done", Tokens: usage.TokenUsage{Input: 200, Output: 300, CacheRead: 1000, Reasoning: 100}, Duration: 2500 * time.Millisecond, Turns: 1},
	}
	for i, w := range want {
		if got := next(t, c); !reflect.DeepEqual(got, w) {
			t.Fatalf("event %d = %#v, want %#v", i, got, w)
		}
	}

	// Overrides stick in Codex, so the next turn sends none.
	go func() { sent <- c.Send(agent.Input{Text: "again"}) }()
	ts = f.expect("turn/start")
	if p := params(t, ts); p["model"] != nil || p["approvalPolicy"] != nil {
		t.Errorf("second turn/start resent overrides: %v", p)
	}
	f.respond(ts.ID, map[string]any{"turn": map[string]any{"id": "tu2"}})
	<-sent

	// A message while a turn runs steers it.
	go func() { sent <- c.Send(agent.Input{Text: "also"}) }()
	st := f.expect("turn/steer")
	if p := params(t, st); p["expectedTurnId"] != "tu2" {
		t.Errorf("turn/steer = %v", p)
	}
	f.respond(st.ID, map[string]any{"turnId": "tu2"})
	<-sent

	go func() { sent <- c.Interrupt() }()
	ti := f.expect("turn/interrupt")
	if p := params(t, ti); p["turnId"] != "tu2" || p["threadId"] != "th1" {
		t.Errorf("turn/interrupt = %v", p)
	}
	f.respond(ti.ID, map[string]any{})
	if err := <-sent; err != nil {
		t.Fatal(err)
	}
}

func TestCommandApproval(t *testing.T) {
	c, f, _ := start(t, agent.StartOptions{})
	next(t, c)
	item := map[string]any{"type": "commandExecution", "id": "c1", "command": "go test ./...", "cwd": "/work", "status": "inProgress",
		"commandActions": []any{map[string]any{"type": "unknown", "command": "go test ./..."}}, "aggregatedOutput": nil, "exitCode": nil}
	f.notify("item/started", map[string]any{"threadId": "th1", "turnId": "tu1", "item": item})
	call := tool.Call{ID: "c1", Name: "shell", Kind: tool.Shell, Input: tool.Input{Command: "go test ./...", Cwd: "/work"}}
	m := next(t, c).(event.Message)
	if got := *m.Parts[0].Call; got.Kind != tool.Shell || !reflect.DeepEqual(got.Input, call.Input) {
		t.Errorf("call = %#v", got)
	}

	f.request("req-7", reqCommand, map[string]any{"kind": "command", "threadId": "th1", "turnId": "tu1", "itemId": "c1", "startedAtMs": 1,
		"reason": "needs network", "command": "go test ./...", "cwd": "/work"})
	a := next(t, c).(event.Approval)
	if a.ID != "req-7" || a.Reason != "needs network" || a.Call.Input.Command != "go test ./..." || a.Call.ID != "c1" {
		t.Errorf("approval = %#v", a)
	}
	kinds := []event.OptionKind{}
	for _, o := range a.Options {
		kinds = append(kinds, o.Kind)
	}
	if !reflect.DeepEqual(kinds, []event.OptionKind{event.AllowOnce, event.AllowAlways, event.RejectOnce, event.RejectAlways}) {
		t.Errorf("option kinds = %v", kinds)
	}
	if err := c.Answer("req-7", "nonsense"); err == nil {
		t.Error("Answer took an option it doesn't have")
	}
	go func() { _ = c.Answer("req-7", a.Options[1].ID) }()
	var reply struct {
		ID     string `json:"id"`
		Result struct {
			Decision string `json:"decision"`
		} `json:"result"`
	}
	if b := f.line(); jsonx.Unmarshal(b, &reply) != nil || reply.ID != "req-7" || reply.Result.Decision != "acceptForSession" {
		t.Errorf("reply = %s", b)
	}
	if err := c.Answer("req-7", "accept"); err == nil {
		t.Error("answered twice")
	}

	// A request resolved without an answer is withdrawn.
	f.request(8, reqFileChange, map[string]any{"threadId": "th1", "turnId": "tu1", "itemId": "f1", "startedAtMs": 1})
	if a := next(t, c).(event.Approval); a.ID != "8" || a.Call.Kind != tool.Edit {
		t.Errorf("file approval = %#v", a)
	}
	f.notify("serverRequest/resolved", map[string]any{"threadId": "th1", "requestId": 8})
	if got := next(t, c); got != (event.ApprovalCancelled{ID: "8"}) {
		t.Errorf("got %#v, want ApprovalCancelled", got)
	}

	item["status"], item["aggregatedOutput"], item["exitCode"] = "completed", "FAIL\n", 1
	f.notify("item/completed", map[string]any{"threadId": "th1", "turnId": "tu1", "item": item})
	out := next(t, c).(event.Message)
	o := out.Parts[0].Output
	if out.Role != "user" || o.CallID != "c1" || o.Stdout != "FAIL\n" || o.Exit == nil || *o.Exit != 1 || !o.IsError {
		t.Errorf("output = %#v", o)
	}
}

func TestPermissionsAndQuestion(t *testing.T) {
	c, f, _ := start(t, agent.StartOptions{})
	next(t, c)
	f.request(1, reqPermissions, map[string]any{"threadId": "th1", "turnId": "tu1", "itemId": "p1", "environmentId": nil, "startedAtMs": 1,
		"cwd": "/work", "reason": "write outside", "permissions": map[string]any{"network": nil, "fileSystem": map[string]any{"read": nil, "write": []string{"/etc/hosts"}}}})
	a := next(t, c).(event.Approval)
	if a.Path != "/etc/hosts" || len(a.Options) != 3 {
		t.Errorf("permissions approval = %#v", a)
	}
	go func() { _ = c.Answer("1", "session") }()
	b := f.line()
	var perm struct {
		Result struct {
			Permissions map[string]any `json:"permissions"`
			Scope       string         `json:"scope"`
		} `json:"result"`
	}
	_ = jsonx.Unmarshal(b, &perm)
	if perm.Result.Scope != "session" || perm.Result.Permissions["fileSystem"] == nil || len(perm.Result.Permissions) != 1 {
		t.Errorf("permissions reply = %s", b)
	}

	f.request(2, reqUserInput, map[string]any{"threadId": "th1", "turnId": "tu1", "itemId": "q1", "isBlocking": true, "autoResolutionMs": nil,
		"questions": []any{map[string]any{"id": "color", "header": "Colour", "question": "Which colour?", "isOther": false, "isSecret": false,
			"options": []any{map[string]any{"label": "Red", "description": "warm"}, map[string]any{"label": "Blue", "description": "cool"}}}}})
	q := next(t, c).(event.Question)
	want := event.Question{ID: "2", CallID: "q1", Asks: []event.Ask{{ID: "color", Header: "Colour", Text: "Which colour?",
		Options: []event.Choice{{Label: "Red", Description: "warm"}, {Label: "Blue", Description: "cool"}}}}}
	if !reflect.DeepEqual(q, want) {
		t.Errorf("question = %#v", q)
	}
	if err := c.Answer("2", "Red"); err == nil {
		t.Error("Answer answered a question")
	}
	go func() { _ = c.AnswerQuestion("2", map[string][]string{"Which colour?": {"Blue"}}) }()
	b = f.line()
	var ans struct {
		Result struct {
			Answers map[string]struct{ Answers []string } `json:"answers"`
		} `json:"result"`
	}
	_ = jsonx.Unmarshal(b, &ans)
	if got := ans.Result.Answers["color"].Answers; !reflect.DeepEqual(got, []string{"Blue"}) {
		t.Errorf("answer reply = %s", b)
	}

	// What rush can't answer is refused rather than left hanging.
	f.request(3, "item/tool/call", map[string]any{})
	b = f.line()
	var ref message
	_ = jsonx.Unmarshal(b, &ref)
	if string(ref.ID) != "3" || ref.Error == nil {
		t.Errorf("refusal = %s", b)
	}
}

func TestFileChange(t *testing.T) {
	c, f, _ := start(t, agent.StartOptions{})
	next(t, c)
	item := map[string]any{"type": "fileChange", "id": "e1", "status": "inProgress", "changes": []any{
		map[string]any{"path": "/work/new.txt", "kind": map[string]any{"type": "add"}, "diff": "one\ntwo\n"},
		map[string]any{"path": "/work/main.go", "kind": map[string]any{"type": "update", "move_path": nil},
			"diff": "@@ -1,3 +1,3 @@\n package main\n-var x = 1\n+var x = 2\n \n"},
	}}
	f.notify("item/started", map[string]any{"threadId": "th1", "turnId": "tu1", "item": item})
	call := next(t, c).(event.Message).Parts[0].Call
	if call.Kind != tool.Edit || call.Input.Path != "/work/new.txt" {
		t.Errorf("call = %#v", call)
	}
	item["status"] = "completed"
	f.notify("item/completed", map[string]any{"threadId": "th1", "turnId": "tu1", "item": item})
	o := next(t, c).(event.Message).Parts[0].Output
	want := []tool.Patch{
		{Path: "/work/new.txt", NewStart: 1, NewLines: 2, Lines: []string{"+one", "+two"}},
		{Path: "/work/main.go", OldStart: 1, OldLines: 3, NewStart: 1, NewLines: 3, Lines: []string{" package main", "-var x = 1", "+var x = 2", " "}},
	}
	if !o.Created || o.IsError || !reflect.DeepEqual(o.Patches, want) {
		t.Errorf("output = %#v", o)
	}
}

func TestCallOf(t *testing.T) {
	tests := []struct {
		name string
		item string
		want tool.Call
	}{
		{"read", `{"type":"commandExecution","id":"a","command":"cat x.go","cwd":"/w","commandActions":[{"type":"read","command":"cat x.go","name":"x.go","path":"/w/x.go"}]}`,
			tool.Call{ID: "a", Name: "shell", Kind: tool.Read, Input: tool.Input{Command: "cat x.go", Cwd: "/w", Path: "/w/x.go"}}},
		{"write", `{"type":"fileChange","id":"b","changes":[{"path":"/w/n","kind":{"type":"add"},"diff":"hi\n"}]}`,
			tool.Call{ID: "b", Name: "apply_patch", Kind: tool.Write, Input: tool.Input{Path: "/w/n", Content: "hi\n"}}},
		{"move", `{"type":"fileChange","id":"c","changes":[{"path":"/w/a","kind":{"type":"update","move_path":"/w/b"},"diff":""}]}`,
			tool.Call{ID: "c", Name: "apply_patch", Kind: tool.Move, Input: tool.Input{Path: "/w/a", To: "/w/b"}}},
		{"mcp", `{"type":"mcpToolCall","id":"d","server":"linear","tool":"get_issue","arguments":{"id":1}}`,
			tool.Call{ID: "d", Name: "mcp__linear__get_issue", Kind: tool.MCP, Input: tool.Input{Server: "linear", Tool: "get_issue"}, Raw: jsontext.Value(`{"id":1}`)}},
		{"search", `{"type":"webSearch","id":"e","query":"","action":{"type":"search","query":"go generics"}}`,
			tool.Call{ID: "e", Name: "web_search", Kind: tool.WebSearch, Input: tool.Input{Query: "go generics"}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var it threadItem
			if err := jsonx.Unmarshal([]byte(tt.item), &it); err != nil {
				t.Fatal(err)
			}
			got, ok := callOf(it, nil)
			if tt.want.Raw == nil {
				got.Raw = nil
			}
			if !ok || !reflect.DeepEqual(got, tt.want) {
				t.Errorf("callOf = %#v, want %#v", got, tt.want)
			}
		})
	}
}

func TestParseDiff(t *testing.T) {
	tests := []struct {
		name string
		diff string
		want []tool.Patch
	}{
		{"two hunks", "@@ -1,2 +1,2 @@\n-a\n+b\n c\n@@ -10 +10,2 @@ func f()\n x\n+y\n", []tool.Patch{
			{Path: "p", OldStart: 1, OldLines: 2, NewStart: 1, NewLines: 2, Lines: []string{"-a", "+b", " c"}},
			{Path: "p", OldStart: 10, OldLines: 1, NewStart: 10, NewLines: 2, Lines: []string{" x", "+y"}},
		}},
		{"file headers", "diff --git a/p b/p\nindex 1..2\n--- a/p\n+++ b/p\n@@ -3,1 +3,1 @@\n-old\n+new\n\\ No newline at end of file\n", []tool.Patch{
			{Path: "p", OldStart: 3, OldLines: 1, NewStart: 3, NewLines: 1, Lines: []string{"-old", "+new"}},
		}},
		{"new file", "@@ -0,0 +1,2 @@\n+a\n+b\n", []tool.Patch{
			{Path: "p", NewStart: 1, NewLines: 2, Lines: []string{"+a", "+b"}},
		}},
		{"two files", "--- a/p\n+++ b/p\n@@ -1 +1 @@\n-a\n+b\n--- a/q\n+++ b/q\n@@ -1 +1 @@\n-c\n+d\n", []tool.Patch{
			{Path: "p", OldStart: 1, OldLines: 1, NewStart: 1, NewLines: 1, Lines: []string{"-a", "+b"}},
			{Path: "p", OldStart: 1, OldLines: 1, NewStart: 1, NewLines: 1, Lines: []string{"-c", "+d"}},
		}},
		{"bare header", "@@\n-a\n+b\n+c\n", []tool.Patch{
			{Path: "p", OldLines: 1, NewLines: 2, Lines: []string{"-a", "+b", "+c"}},
		}},
		{"no hunks", "", nil},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := parseDiff("p", tt.diff); !reflect.DeepEqual(got, tt.want) {
				t.Errorf("parseDiff = %#v\nwant %#v", got, tt.want)
			}
		})
	}
}

func mins(n int64) *int64 { return &n }

func TestQuotaFrom(t *testing.T) {
	reset := int64(1790000000)
	tests := []struct {
		name string
		snap rateLimitSnapshot
		want []usage.Window
	}{
		{"weekly primary, no secondary", rateLimitSnapshot{LimitID: "codex", Primary: &rateLimitWindow{UsedPercent: 12, WindowDurationMins: mins(10080), ResetsAt: &reset}},
			[]usage.Window{{ID: "primary", Label: "7d", Name: "weekly", Span: 7 * 24 * time.Hour, Percent: 12, ResetsAt: time.Unix(reset, 0)}}},
		{"missing primary", rateLimitSnapshot{Secondary: &rateLimitWindow{UsedPercent: 40, WindowDurationMins: mins(300)}},
			[]usage.Window{{ID: "secondary", Label: "5h", Name: "5-hour", Span: 5 * time.Hour, Percent: 40}}},
		{"both", rateLimitSnapshot{Primary: &rateLimitWindow{UsedPercent: 1, WindowDurationMins: mins(300)}, Secondary: &rateLimitWindow{UsedPercent: 2, WindowDurationMins: mins(43200)}},
			[]usage.Window{{ID: "primary", Label: "5h", Name: "5-hour", Span: 5 * time.Hour, Percent: 1}, {ID: "secondary", Label: "30d", Name: "30-day", Span: 30 * 24 * time.Hour, Percent: 2}}},
		{"no duration", rateLimitSnapshot{Primary: &rateLimitWindow{UsedPercent: 5}},
			[]usage.Window{{ID: "primary", Label: "limit", Name: "usage limit", Percent: 5}}},
		{"per model", rateLimitSnapshot{LimitID: "base_model_inference", LimitName: "gpt-reserve", NormalModelSlug: "gpt-5.6-luna", Primary: &rateLimitWindow{UsedPercent: 3, WindowDurationMins: mins(10080)}},
			[]usage.Window{{ID: "base_model_inference:primary", Label: "gpt-reserve 7d", Name: "gpt-reserve weekly", Span: 7 * 24 * time.Hour, Percent: 3, Scope: usage.Scope{Models: []string{"gpt-5.6-luna"}}}}},
		{"nothing", rateLimitSnapshot{}, nil},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := quotaFrom(tt.snap).Windows; !reflect.DeepEqual(got, tt.want) {
				t.Errorf("windows = %#v\nwant %#v", got, tt.want)
			}
		})
	}
}

func TestBilling(t *testing.T) {
	full, part := &rateLimitWindow{UsedPercent: 100}, &rateLimitWindow{UsedPercent: 40}
	credits := &struct {
		HasCredits bool `json:"hasCredits"`
		Unlimited  bool `json:"unlimited"`
	}{HasCredits: true}
	for name, tt := range map[string]struct {
		snap rateLimitSnapshot
		want usage.Billing
		ok   bool
	}{
		"within the plan":      {rateLimitSnapshot{Primary: part, Credits: credits}, usage.Plan, true},
		"spent, credits pay":   {rateLimitSnapshot{Primary: part, Secondary: full, Credits: credits}, usage.Overage, true},
		"spent, no credits":    {rateLimitSnapshot{Primary: full}, usage.Plan, true},
		"reached, credits pay": {rateLimitSnapshot{ReachedType: "rate_limit_reached", Credits: credits}, usage.Overage, true},
		"usage-based plan":     {rateLimitSnapshot{Primary: part, PlanType: "enterprise_cbp_usage_based"}, usage.Metered, true},
		"says nothing":         {rateLimitSnapshot{LimitID: "codex"}, "", false},
	} {
		if got, ok := tt.snap.billing(); got != tt.want || ok != tt.ok {
			t.Errorf("%s: billing = %q, %v", name, got, ok)
		}
	}
	var a accountResponse
	_ = jsonx.Unmarshal([]byte(`{"account":{"type":"apiKey"}}`), &a)
	if b, _ := accountBilling(a); b != usage.Metered {
		t.Errorf("api key = %q", b)
	}
	_ = jsonx.Unmarshal([]byte(`{"account":{"type":"chatgpt","planType":"pro"}}`), &a)
	if b, _ := accountBilling(a); b != usage.Plan {
		t.Errorf("chatgpt = %q", b)
	}
}

func TestQuotaFromResponse(t *testing.T) {
	var r rateLimitsResponse
	raw := `{"rateLimits":{"limitId":"codex","primary":{"usedPercent":10,"windowDurationMins":10080,"resetsAt":1},"secondary":null,"planType":"pro"},
		"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":10,"windowDurationMins":10080,"resetsAt":1},"secondary":null,"planType":"pro"},
		"other":{"limitId":"other","normalModelSlug":"gpt-x","primary":{"usedPercent":50,"windowDurationMins":300,"resetsAt":null},"secondary":null}},
		"accountId":"acct-1"}`
	if err := jsonx.Unmarshal([]byte(raw), &r); err != nil {
		t.Fatal(err)
	}
	q := quotaFromResponse(r)
	if q.Account != "codex:acct-1" || q.Plan != "pro" || q.Source != usage.Fetched || len(q.Windows) != 2 {
		t.Fatalf("quota = %#v", q)
	}
	if w, _ := q.Tightest("gpt-5.5"); w.ID != "primary" {
		t.Errorf("tightest for gpt-5.5 = %s", w.ID)
	}
	if w, _ := q.Tightest("gpt-x"); w.ID != "other:primary" {
		t.Errorf("tightest for gpt-x = %s", w.ID)
	}
}

func TestRateLimitNotification(t *testing.T) {
	c, f, _ := start(t, agent.StartOptions{})
	next(t, c)
	f.notify("account/rateLimits/updated", map[string]any{"rateLimits": map[string]any{"limitId": "codex", "primary": nil,
		"secondary": map[string]any{"usedPercent": 70, "windowDurationMins": 10080, "resetsAt": nil}}})
	q := next(t, c).(event.Quota)
	if q.Source != usage.Live || len(q.Windows) != 1 || q.Windows[0].ID != "secondary" || q.Windows[0].Label != "7d" {
		t.Errorf("quota = %#v", q)
	}
}

// TestLive reads the real account's limits: initialize and
// account/rateLimits/read only, neither of which spends model quota.
func TestLive(t *testing.T) {
	if os.Getenv("RUSH_CODEX_LIVE") != "1" {
		t.Skip("RUSH_CODEX_LIVE=1 runs the real codex app-server")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	q, err := Quota(ctx, agent.Profile{Kind: Kind}, "")
	if err != nil {
		t.Fatal(err)
	}
	if q.Account == "" || len(q.Windows) == 0 {
		t.Errorf("reading has no account or windows: %d windows", len(q.Windows))
	}
	for _, w := range q.Windows {
		t.Logf("window %s: %s (%s), span %s, models %v", w.ID, w.Label, w.Name, w.Span, w.Scope.Models)
	}
	t.Logf("plan %s", q.Plan)
}

func TestScript(t *testing.T) {
	for in, want := range map[string]string{
		`/bin/zsh -lc 'ls -la'`:                 "ls -la",
		`bash -c 'echo '\''hi'\'' && pwd'`:      "echo 'hi' && pwd",
		`/bin/zsh -lc 'a' extra`:                `/bin/zsh -lc 'a' extra`,
		`go test ./...`:                         "go test ./...",
		`/bin/zsh -lc "printf 'hi\\n' > a.txt"`: `printf 'hi\n' > a.txt`,
		`/bin/zsh -lc "echo \"\$HOME\""`:        `echo "$HOME"`,
	} {
		if got := script(in); got != want {
			t.Errorf("script(%q) = %q, want %q", in, got, want)
		}
	}
}
