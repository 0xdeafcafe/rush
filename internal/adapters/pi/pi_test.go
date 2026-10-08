package pi

import (
	"bufio"
	"context"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// feed runs a recorded RPC session through a Conn's handler, and is what
// it said.
func feed(t *testing.T, c *Conn, name string) []event.Event {
	t.Helper()
	f, err := os.Open(filepath.Join("testdata", name))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	var out []event.Event
	err = readLines(f, func(b []byte) bool {
		var head struct {
			Type string `json:"type"`
		}
		if err := jsonx.Unmarshal(b, &head); err != nil {
			t.Fatalf("%s: %v", b, err)
		}
		out = append(out, c.events1(head.Type, b)...)
		return true
	})
	if err != nil {
		t.Fatal(err)
	}
	return out
}

func only[T event.Event](evs []event.Event) []T {
	var out []T
	for _, ev := range evs {
		if e, ok := ev.(T); ok {
			out = append(out, e)
		}
	}
	return out
}

// A turn on a thinking model that reads a file, edits it, reads it again
// and says so: pi 0.85 with qwen3.8 on Ollama.
func TestEditTurn(t *testing.T) { //nolint:gocognit,gocyclo // one recorded turn, checked part by part
	c := newConn(context.Background())
	c.model = model{ID: "qwen3.8:27b-mlx", Provider: "ollama", ContextWindow: 65536}
	c.sent = []string{"Use the edit tool to change the line two to three in b.txt. Think briefly first."}
	evs := feed(t, c, "edit.jsonl")

	if s, ok := evs[0].(event.Status); !ok || !s.Busy {
		t.Fatalf("first event %#v, want busy", evs[0])
	}
	if len(c.sent) != 0 {
		t.Errorf("the prompt sent wasn't taken as said back: %q", c.sent)
	}
	var calls []tool.Call
	var outs []tool.Output
	for _, m := range only[event.Message](evs) {
		if m.Role == "user" && m.Parts[0].Kind == event.Text {
			t.Errorf("the prompt rush sent came back as a message: %+v", m)
		}
		for _, p := range m.Parts {
			switch p.Kind {
			case event.ToolCall:
				calls = append(calls, *p.Call)
			case event.ToolResult:
				outs = append(outs, *p.Output)
			}
		}
	}
	if len(calls) != 3 || len(outs) != 3 {
		t.Fatalf("%d calls, %d results", len(calls), len(outs))
	}
	edit := calls[1]
	if edit.Kind != tool.Edit || edit.Input.Path != "b.txt" || !reflect.DeepEqual(edit.Input.Edits, []tool.Replace{{Old: "two", New: "three"}}) {
		t.Errorf("edit call: %+v", edit)
	}
	if calls[0].Kind != tool.Read || calls[0].Input.Path != "b.txt" {
		t.Errorf("read call: %+v", calls[0])
	}
	want := []tool.Patch{{OldStart: 1, OldLines: 2, NewStart: 1, NewLines: 2, Lines: []string{" one", "-two", "+three"}}}
	if outs[1].CallID != edit.ID || !reflect.DeepEqual(outs[1].Patches, want) {
		t.Errorf("edit result: %+v", outs[1])
	}
	if outs[0].Text != "one\ntwo\n" || outs[0].IsError {
		t.Errorf("read result: %+v", outs[0])
	}

	starts := only[event.PartStart](evs)
	if !slices.ContainsFunc(starts, func(p event.PartStart) bool { return p.Kind == event.Thinking }) {
		t.Error("no thinking part started")
	}
	thinking := false
	for _, d := range only[event.Delta](evs) {
		thinking = thinking || d.Kind == event.Thinking && d.Text != ""
	}
	if !thinking {
		t.Error("no thinking streamed")
	}
	if ms := only[event.MessageStart](evs); len(ms) != 4 || ms[0].ID != "chatcmpl-300" || ms[0].Model != "qwen3.8:27b-mlx" {
		t.Errorf("message starts: %+v", ms)
	}
	if cx := only[event.Context](evs); len(cx) != 4 || cx[3].Window != 65536 || cx[3].Tokens != 1941 {
		t.Errorf("context: %+v", cx)
	}

	ends := only[event.TurnEnd](evs)
	if len(ends) != 1 {
		t.Fatalf("turn ends: %+v", ends)
	}
	end := ends[0]
	if end.Reason != "done" || end.Err != "" || !strings.HasPrefix(end.Text, "Done. `b.txt` now contains") || end.Turns != 1 {
		t.Errorf("turn end: %+v", end)
	}
	var sum usage.TokenUsage
	for _, m := range only[event.Message](evs) {
		if m.Tokens != nil {
			sum.Add(*m.Tokens)
		}
	}
	if end.Tokens != sum || sum.Output == 0 {
		t.Errorf("turn's tokens %+v, its messages' %+v", end.Tokens, sum)
	}
	if _, last := evs[len(evs)-1].(event.TurnEnd); !last {
		t.Errorf("last event %#v", evs[len(evs)-1])
	}
}

// An abort mid-answer ends that run as interrupted; the message steered in
// meanwhile then runs as a run of its own.
func TestAbortThenSteer(t *testing.T) {
	c := newConn(context.Background())
	c.model = model{ID: "qwen2.5:1.5b", Provider: "ollama", ContextWindow: 32768}
	c.sent = []string{"Write a 500 word poem.", "shorter"}
	evs := feed(t, c, "abort.jsonl")
	ends := only[event.TurnEnd](evs)
	if len(ends) != 2 || ends[0].Reason != "interrupted" || ends[0].Err != "" || ends[1].Reason != "done" {
		t.Fatalf("turn ends: %+v", ends)
	}
	if len(only[event.Status](evs)) != 2 {
		t.Errorf("want busy at each run's start: %+v", only[event.Status](evs))
	}
	for _, m := range only[event.Message](evs) {
		if m.Role == "user" {
			t.Errorf("a message rush sent came back: %+v", m)
		}
	}
}

// fake is a pi on the far end of two pipes.
type fake struct {
	t   *testing.T
	in  *bufio.Scanner
	out io.Writer
}

// next is the next command rush sends, which must be of type typ.
func (f *fake) next(typ string) map[string]any {
	f.t.Helper()
	if !f.in.Scan() {
		f.t.Fatal("pipe closed")
	}
	var m map[string]any
	if err := jsonx.Unmarshal(f.in.Bytes(), &m); err != nil {
		f.t.Fatalf("bad line %q: %v", f.in.Text(), err)
	}
	if m["type"] != typ {
		f.t.Fatalf("got %s, want %s", f.in.Bytes(), typ)
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

func (f *fake) ok(cmd map[string]any, data any) {
	f.write(map[string]any{"id": cmd["id"], "type": "response", "command": cmd["type"], "success": true, "data": data})
}

func (f *fake) fail(cmd map[string]any, msg string) {
	f.write(map[string]any{"id": cmd["id"], "type": "response", "command": cmd["type"], "success": false, "error": msg})
}

// start runs a Conn against a fake through get_state and get_commands.
func start(t *testing.T) (*Conn, *fake) {
	t.Helper()
	cr, fw := io.Pipe() // fake → conn
	fr, cw := io.Pipe() // conn → fake
	f := &fake{t: t, in: bufio.NewScanner(fr), out: fw}
	f.in.Buffer(nil, 1<<20)
	c := newConn(context.Background())
	rpc := newClient(cr, cw, c.handle)
	errc := make(chan error, 1)
	go func() { errc <- c.begin(rpc, &agent.StartOptions{Dir: "/work"}) }()
	f.ok(f.next("get_state"), map[string]any{"sessionId": "s1", "sessionFile": "/x/s1.jsonl",
		"model": map[string]any{"id": "qwen2.5:1.5b", "provider": "ollama", "contextWindow": 32768}})
	if err := <-errc; err != nil {
		t.Fatal(err)
	}
	f.ok(f.next("get_commands"), map[string]any{"commands": []any{map[string]any{"name": "skill:x", "description": "does x", "source": "skill"}}})
	t.Cleanup(func() { _ = fw.Close(); _ = fr.Close(); _ = c.Close() })
	return c, f
}

func next[T event.Event](t *testing.T, c *Conn) T {
	t.Helper()
	timeout := time.After(5 * time.Second)
	for {
		select {
		case ev, ok := <-c.Events():
			if !ok {
				t.Fatal("events ended")
			}
			if e, ok := ev.(T); ok {
				return e
			}
		case <-timeout:
			var zero T
			t.Fatalf("no %T", zero)
		}
	}
}

func TestConn(t *testing.T) { //nolint:gocognit,gocyclo // one session, step by step
	c, f := start(t)
	if init := next[event.Init](t, c); init.SessionID != "s1" || init.Model != "qwen2.5:1.5b" || init.Cwd != "/work" {
		t.Fatalf("init: %+v", init)
	}
	if cx := next[event.Context](t, c); cx.Window != 32768 {
		t.Errorf("context: %+v", cx)
	}
	if cmds := next[event.Commands](t, c); len(cmds.List) != 1 || cmds.List[0].Name != "skill:x" {
		t.Errorf("commands: %+v", cmds)
	}

	// Idle, a message is a prompt of its own.
	done := make(chan error, 1)
	go func() { done <- c.Send(agent.Input{Text: "hi"}) }()
	p := f.next("prompt")
	if p["message"] != "hi" || p["streamingBehavior"] != nil {
		t.Errorf("prompt: %v", p)
	}
	f.ok(p, nil)
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	f.write(map[string]any{"type": "agent_start"})
	if s := next[event.Status](t, c); !s.Busy {
		t.Errorf("status: %+v", s)
	}

	// Busy, it steers.
	go func() { done <- c.Send(agent.Input{Text: "and this"}) }()
	p = f.next("prompt")
	if p["message"] != "and this" || p["streamingBehavior"] != "steer" {
		t.Errorf("steer: %v", p)
	}
	f.ok(p, nil)
	if err := <-done; err != nil {
		t.Fatal(err)
	}

	// An interrupt doesn't wait for pi's answer.
	go func() { done <- c.Interrupt() }()
	if a := f.next("abort"); a["id"] != nil {
		t.Errorf("abort waits on an answer: %v", a)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	f.write(map[string]any{"type": "agent_end", "messages": []any{}})
	if e := next[event.TurnEnd](t, c); e.Reason != "done" {
		t.Errorf("turn end: %+v", e)
	}

	// Pi busy on its own when rush thought it idle: the prompt steers.
	go func() { done <- c.Send(agent.Input{Text: "late"}) }()
	f.fail(f.next("prompt"), "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message.")
	if p = f.next("prompt"); p["streamingBehavior"] != "steer" {
		t.Errorf("retry: %v", p)
	}
	f.ok(p, nil)
	if err := <-done; err != nil {
		t.Fatal(err)
	}

	// A model by its id, the provider's own first.
	go func() { done <- c.SetModel("qwen3.8:27b-mlx") }()
	f.ok(f.next("get_available_models"), map[string]any{"models": []any{
		map[string]any{"id": "qwen3.8:27b-mlx", "provider": "other"},
		map[string]any{"id": "qwen3.8:27b-mlx", "provider": "ollama", "contextWindow": 65536},
	}})
	set := f.next("set_model")
	if set["provider"] != "ollama" || set["modelId"] != "qwen3.8:27b-mlx" {
		t.Errorf("set_model: %v", set)
	}
	f.ok(set, map[string]any{"id": "qwen3.8:27b-mlx", "provider": "ollama", "contextWindow": 65536})
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if cx := next[event.Context](t, c); cx.Window != 65536 {
		t.Errorf("context after the switch: %+v", cx)
	}
	if err := c.SetMode("auto"); err == nil {
		t.Error("pi took a mode")
	}

	// /compact compacts, and ends the turn it began.
	if err := c.Send(agent.Input{Text: "/compact keep the tests"}); err != nil {
		t.Fatal(err)
	}
	cp := f.next("compact")
	if cp["customInstructions"] != "keep the tests" {
		t.Errorf("compact: %v", cp)
	}
	f.write(map[string]any{"type": "compaction_start", "reason": "manual"})
	f.write(map[string]any{"type": "compaction_end", "reason": "manual", "result": map[string]any{"summary": "s", "tokensBefore": 900}})
	f.ok(cp, map[string]any{"summary": "s", "tokensBefore": 900})
	if cm := next[event.Compacted](t, c); cm.Trigger != "manual" || cm.Before != 900 {
		t.Errorf("compacted: %+v", cm)
	}
	if e := next[event.TurnEnd](t, c); e.Reason != "done" {
		t.Errorf("compact's turn end: %+v", e)
	}

	// An extension's select is a question.
	f.write(map[string]any{"type": "extension_ui_request", "id": "u1", "method": "select", "title": "Allow rm?", "options": []string{"Allow", "Block"}})
	q := next[event.Question](t, c)
	if q.ID != "u1" || len(q.Asks) != 1 || len(q.Asks[0].Options) != 2 || q.Asks[0].Text != "Allow rm?" {
		t.Fatalf("question: %+v", q)
	}
	go func() { done <- c.AnswerQuestion("u1", map[string][]string{"Allow rm?": {"Block"}}) }()
	if r := f.next("extension_ui_response"); r["id"] != "u1" || r["value"] != "Block" {
		t.Errorf("answer: %v", r)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	f.write(map[string]any{"type": "extension_ui_request", "id": "u2", "method": "confirm", "title": "Clear?", "message": "All is lost."})
	if q := next[event.Question](t, c); q.Asks[0].Header != "Clear?" || q.Asks[0].Text != "All is lost." {
		t.Errorf("confirm: %+v", q)
	}
	go func() { done <- c.AnswerQuestion("u2", map[string][]string{"All is lost.": {"Yes"}}) }()
	if r := f.next("extension_ui_response"); r["confirmed"] != true {
		t.Errorf("confirm answer: %v", r)
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	f.write(map[string]any{"type": "extension_ui_request", "id": "u3", "method": "input", "title": "Name?"})
	if r := f.next("extension_ui_response"); r["id"] != "u3" || r["cancelled"] != true {
		t.Errorf("input isn't cancelled: %v", r)
	}
	if err := c.Answer("u1", "x"); err == nil {
		t.Error("pi took an approval")
	}
}

func TestHistory(t *testing.T) {
	b, err := os.ReadFile(filepath.Join("testdata", "session.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	evs, err := readSession(strings.NewReader(string(b)), time.Time{})
	if err != nil {
		t.Fatal(err)
	}
	init, ok := evs[0].(event.Init)
	if !ok || init.SessionID != "01a0ed43-bcca-7039-ad79-9d3cd0d16072" || init.Model != "qwen3.8:27b-mlx" || init.Cwd != "/private/tmp/pitest/work" {
		t.Fatalf("init: %#v", evs[0])
	}
	msgs := only[event.Message](evs)
	if msgs[0].Role != "user" || msgs[0].Parts[0].Text != "Use the edit tool to change the line two to three in b.txt. Think briefly first." {
		t.Errorf("first message: %+v", msgs[0])
	}
	var calls int
	for _, m := range msgs {
		for _, p := range m.Parts {
			if p.Kind == event.ToolCall {
				calls++
			}
			if p.Kind == event.ToolResult && p.Output.Patches != nil && p.Output.Patches[0].Lines[2] != "+three" {
				t.Errorf("patch: %+v", p.Output.Patches)
			}
		}
	}
	ends := only[event.TurnEnd](evs)
	if calls != 3 || len(ends) != 1 || ends[0].Reason != "done" || ends[0].Duration <= 0 {
		t.Errorf("%d calls, ends %+v", calls, ends)
	}

	// A branch left by going back isn't the conversation: a new prompt
	// made a child of the first leaves the rest behind.
	var first struct {
		ID string `json:"id"`
	}
	for l := range strings.SplitSeq(string(b), "\n") {
		if strings.Contains(l, `"role":"user"`) {
			_ = jsonx.Unmarshal([]byte(l), &first)
			break
		}
	}
	branched := string(b) + `{"type":"message","id":"zz000001","parentId":"` + first.ID + `","timestamp":"2026-09-29T13:10:00.000Z","message":{"role":"user","content":"try again","timestamp":1}}` + "\n"
	evs, _ = readSession(strings.NewReader(branched), time.Time{})
	msgs = only[event.Message](evs)
	if len(msgs) != 2 || msgs[1].Parts[0].Text != "try again" {
		t.Errorf("branch: %+v", msgs)
	}

	// Before stops at the first entry written at or after it.
	evs, _ = readSession(strings.NewReader(string(b)), time.Date(2026, 9, 29, 13, 5, 0, 0, time.UTC))
	if n := len(only[event.Message](evs)); n != 3 {
		t.Errorf("%d messages before 13:05", n)
	}
}

func TestPastAndResume(t *testing.T) {
	dir := t.TempDir()
	p := agent.Profile{Kind: "ollama-pi", Dir: dir}
	b, _ := os.ReadFile(filepath.Join("testdata", "session.jsonl"))
	path := filepath.Join(dir, "sessions", "--private-tmp-pitest-work--", "2026-09-29T13-03-54-573Z_01a0ed43-bcca-7039-ad79-9d3cd0d16072.jsonl")
	_ = os.MkdirAll(filepath.Dir(path), 0o755)
	if err := os.WriteFile(path, b, 0o644); err != nil {
		t.Fatal(err)
	}
	past := Adapter{}.Past(p)
	if len(past) != 1 {
		t.Fatalf("past: %+v", past)
	}
	s := past[0]
	if s.ID != "01a0ed43-bcca-7039-ad79-9d3cd0d16072" || s.Kind != "ollama-pi" || s.Model != "qwen3.8:27b-mlx" ||
		s.Cwd != "/private/tmp/pitest/work" || !strings.HasPrefix(s.Name, "Use the edit tool") || s.Transcript != path {
		t.Errorf("session: %+v", s)
	}
	// A name given with /name wins over the first prompt.
	lines := strings.Split(strings.TrimSpace(string(b)), "\n")
	var tip struct {
		ID string `json:"id"`
	}
	_ = jsonx.Unmarshal([]byte(lines[len(lines)-1]), &tip)
	named := string(b) + `{"type":"session_info","id":"n1","parentId":"` + tip.ID + `","timestamp":"2026-09-29T13:10:00.000Z","name":"Edit b"}` + "\n"
	_ = os.WriteFile(path, []byte(named), 0o644)
	if s := (Adapter{}).Past(p)[0]; s.Name != "Edit b" {
		t.Errorf("named %q", s.Name)
	}
	if evs, err := (Adapter{}).History(agent.Session{Profile: p, ID: s.ID}, time.Time{}); err != nil || len(evs) < 5 {
		t.Errorf("history by id: %v, %d events", err, len(evs))
	}

	args, err := argsFor(&agent.StartOptions{Profile: p, SessionID: s.ID, Resume: true, Model: "ollama/q", Effort: "low"})
	if err != nil {
		t.Fatal(err)
	}
	if want := []string{"--mode", "rpc", "--session", path, "--model", "ollama/q", "--thinking", "low"}; !reflect.DeepEqual(args, want) {
		t.Errorf("resume args %q", args)
	}
	if !(Adapter{}).RunsSession(append([]string{"node", "pi"}, args...), s.ID) || (Adapter{}).RunsSession(args, "other") {
		t.Error("RunsSession")
	}
	args, _ = argsFor(&agent.StartOptions{Profile: p, SessionID: s.ID, Fork: true})
	if !slices.Contains(args, "--fork") {
		t.Errorf("fork args %q", args)
	}
	args, _ = argsFor(&agent.StartOptions{Profile: p, SessionID: "new-one"})
	if !reflect.DeepEqual(args, []string{"--mode", "rpc", "--session-id", "new-one"}) {
		t.Errorf("new args %q", args)
	}
	if _, err := argsFor(&agent.StartOptions{Profile: p, SessionID: "gone", Resume: true}); err == nil {
		t.Error("resumed a session that isn't there")
	}
}

func TestParseDiff(t *testing.T) {
	diff := "  1 a\n- 2 b\n+ 2 B\n+ 3 C\n  4 c\n    ...\n  9 h\n-10 i\n  11 j"
	got := parseDiff(diff)
	want := []tool.Patch{
		{OldStart: 1, OldLines: 3, NewStart: 1, NewLines: 4, Lines: []string{" a", "-b", "+B", "+C", " c"}},
		{OldStart: 9, OldLines: 3, NewStart: 10, NewLines: 2, Lines: []string{" h", "-i", " j"}},
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %+v\nwant %+v", got, want)
	}
}

// A pi folder left by an uninstalled pi isn't a profile.
func TestProfilesNeedPiInstalled(t *testing.T) {
	home, bin := t.TempDir(), t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("PATH", bin)
	t.Setenv("PI_CODING_AGENT_DIR", "")
	_ = os.MkdirAll(filepath.Join(home, ".pi", "agent"), 0o755)
	defer agent.Recheck()
	agent.Recheck()
	if p := agent.Path(Kind); p != "" {
		t.Skipf("pi is installed where every machine looks: %s", p)
	}
	if ps := (Adapter{}).Profiles(); len(ps) != 0 {
		t.Fatalf("pi isn't installed, but Profiles = %+v", ps)
	}
	if err := os.WriteFile(filepath.Join(bin, "pi"), []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	agent.Recheck()
	if ps := (Adapter{}).Profiles(); len(ps) != 1 || ps[0].Dir != filepath.Join(home, ".pi", "agent") {
		t.Fatalf("pi is installed, Profiles = %+v", ps)
	}
}

// Powershell runs a command as bash does.
func TestPowershellIsShell(t *testing.T) {
	c := callOf(&block{ID: "c1", Name: "powershell", Arguments: []byte(`{"command":"Get-ChildItem","timeout":5}`)})
	if c.Kind != tool.Shell || c.Input.Command != "Get-ChildItem" || c.Input.Timeout != 5000 {
		t.Errorf("%+v", c)
	}
}

// A session read back draws the subagent extension's calls as subagents:
// one agent's task, and several in parallel.
func TestSubagentHistory(t *testing.T) {
	session := `{"type":"session","id":"s1","cwd":"/w"}
{"type":"message","id":"a1","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"c1","name":"subagent","arguments":{"agent":"scout","task":"Find the auth code\nand list it"}},{"type":"toolCall","id":"c2","name":"subagent","arguments":{"tasks":[{"agent":"scout","task":"models"},{"agent":"planner","task":"providers"}]}}]}}
`
	evs, _ := readSession(strings.NewReader(session), time.Time{})
	var calls []tool.Call
	for _, m := range only[event.Message](evs) {
		for _, p := range m.Parts {
			if p.Call != nil {
				calls = append(calls, *p.Call)
			}
		}
	}
	if len(calls) != 2 {
		t.Fatalf("calls: %+v", calls)
	}
	if c := calls[0]; c.Kind != tool.Subagent || c.Input.Agent != "scout" || c.Input.Prompt != "Find the auth code\nand list it" || c.Input.Description != "Find the auth code and list it" {
		t.Errorf("single: %+v", c.Input)
	}
	if c := calls[1]; c.Kind != tool.Subagent || c.Input.Agent != "scout, planner" || c.Input.Prompt != "scout: models\n\nplanner: providers" {
		t.Errorf("parallel: %+v", c.Input)
	}
}

// Live, a subagent call's run is a task the turn waits on, from its start
// through its progress to its end; other tools' runs aren't tasks.
func TestSubagentTask(t *testing.T) {
	c := newConn(t.Context())
	defer c.Close()
	lines := []string{
		`{"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{"command":"ls"}}`,
		`{"type":"tool_execution_start","toolCallId":"c2","toolName":"subagent","args":{"agent":"scout","task":"find auth"}}`,
		`{"type":"tool_execution_update","toolCallId":"c2","toolName":"subagent","partialResult":{"content":[{"type":"text","text":"reading"}],"details":{"mode":"single","results":[{"agent":"scout","usage":{"input":100,"output":20},"messages":[{"role":"assistant","content":[{"type":"toolCall","id":"x","name":"read","arguments":{}},{"type":"toolCall","id":"y","name":"grep","arguments":{}}]}]}]}}}`,
		`{"type":"tool_execution_end","toolCallId":"c2","toolName":"subagent","result":{"content":[]},"isError":true}`,
	}
	var evs []event.Event
	for _, l := range lines {
		var head struct {
			Type string `json:"type"`
		}
		_ = jsonx.Unmarshal([]byte(l), &head)
		evs = append(evs, c.events1(head.Type, []byte(l))...)
	}
	want := []event.Event{
		event.TaskStarted{ID: "c2", CallID: "c2", Kind: event.SubagentTask, Label: "find auth", Agent: "scout"},
		event.TaskProgress{ID: "c2", Summary: "reading", LastTool: "grep", Tokens: 120, ToolUses: 2},
		event.TaskDone{ID: "c2", CallID: "c2", Status: "failed"},
	}
	if !reflect.DeepEqual(evs, want) {
		t.Errorf("got %+v", evs)
	}
}
