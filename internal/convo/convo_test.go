package convo

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agtools"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/jsonx"
)

var t0 = time.Date(2026, 9, 23, 22, 0, 0, 0, time.UTC)

func at(sec int) time.Time { return t0.Add(time.Duration(sec) * time.Second) }

func raw(v any) jsontext.Value { b, _ := jsonx.Marshal(v); return b }

func toolUse(id, name string, in any) headless.Message {
	return headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: id, Name: name, Input: raw(in)}}}
}

func toolResult(id, text string, isErr bool, structured any) headless.Message {
	m := headless.Message{Role: "user", Blocks: []headless.Block{{Type: "tool_result", ToolUseID: id, Text: text, IsError: isErr}}}
	if structured != nil {
		m.ToolResult = raw(structured)
	}
	return m
}

func say(text string) headless.Message {
	return headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "text", Text: text}}}
}

// session plays two turns: a finished one with three clean steps and an
// answer, and a live one with a failed command, an edit and a subagent.
func session() *Session {
	s := New()
	s.Info.Cwd = "/work/rush"
	evs := []struct {
		sec int
		ev  any
	}{
		{0, host.Sent{Text: "the ux right now is totally broken when i attach"}},
		{1, say("Looking at how attach restores the terminal modes.")},
		{2, toolUse("r1", "Read", map[string]any{"file_path": "/work/rush/internal/daemon/attach.go"})},
		{3, toolResult("r1", "…", false, map[string]any{"type": "text", "file": map[string]any{"numLines": 166, "startLine": 1, "totalLines": 166}})},
		{3, toolUse("g1", "Grep", map[string]any{"pattern": "1049h", "path": "/work/rush/internal"})},
		{4, toolResult("g1", "internal/daemon/attach.go\ninternal/ui/live.go", false, nil)},
		{4, toolUse("b1", "Bash", map[string]any{"command": "go build ./..."})},
		{6, toolResult("b1", "", false, map[string]any{"stdout": "", "stderr": ""})},
		{7, toolUse("e1", "Edit", map[string]any{"file_path": "/work/rush/internal/daemon/attach.go"})},
		{8, toolResult("e1", "ok", false, map[string]any{"structuredPatch": []map[string]any{{"oldStart": 60, "oldLines": 2, "newStart": 60, "newLines": 3,
			"lines": []string{" \tfor _, m := range info.DecModes {", "-\t\tfmt.Fprint(out, x)", "+\t\tfmt.Fprint(out, y)", "+\t\tfmt.Fprint(out, z)"}}}})},
		{9, say("Fixed the **alt screen**. Attach now clears to the alternate screen first.")},
		{10, headless.Result{Subtype: "success", CostUSD: 0.52}},

		{20, host.Sent{Text: "add modern key stuff to input too"}},
		{21, headless.Delta{Thinking: true, Text: "hmm"}},
		{22, toolUse("b2", "Bash", map[string]any{"command": "cd /work/rush/internal/ui && go vet ./... 2>&1 | head -50"})},
		{23, toolResult("b2", "Exit code 1\ninternal/ui/editor.go:41:2: unreachable code", true,
			map[string]any{"stdout": "internal/ui/editor.go:41:2: unreachable code", "stderr": ""})},
		{24, toolUse("w1", "Write", map[string]any{"file_path": "/work/rush/internal/ui/editor.go", "content": "package ui\n\nfunc x() {}\n"})},
		{25, toolResult("w1", "ok", false, map[string]any{"type": "create", "content": "package ui\n\nfunc x() {}\n"})},
		{26, toolUse("a1", "Task", map[string]any{"subagent_type": "Explore", "description": "find the preview pane"})},
		{27, headless.Message{Role: "assistant", ParentToolUseID: "a1", Blocks: []headless.Block{{Type: "tool_use", ID: "a1g", Name: "Grep", Input: raw(map[string]any{"pattern": "previewLines"})}}}},
		{28, toolUse("b3", "Bash", map[string]any{"command": "go test -race ./..."})},
		{29, headless.PermissionRequest{ID: "req-1", Tool: "Bash", ToolUseID: "b3"}},
		{30, headless.Delta{Text: "Building the line editor"}},
	}
	for _, e := range evs {
		s.Apply(e.ev, at(e.sec))
	}
	return s
}

func plain(lines []Line) string {
	var b strings.Builder
	for _, l := range lines {
		b.WriteString(strings.TrimRight(ansi.Strip(l.Text), " "))
		b.WriteByte('\n')
	}
	return b.String()
}

func TestModel(t *testing.T) {
	s := session()
	if len(s.Turns) != 2 {
		t.Fatalf("turns: %d", len(s.Turns))
	}
	done, live := s.Turns[0], s.Turns[1]
	if done.Live || !live.Live {
		t.Fatalf("live flags: %v %v", done.Live, live.Live)
	}
	if got := done.Outcome(); got != "Fixed the alt screen. Attach now clears to the alternate screen first." {
		t.Errorf("outcome: %q", got)
	}
	if done.Cost != 0.52 || done.Steps() != 4 {
		t.Errorf("cost %v steps %d", done.Cost, done.Steps())
	}
	if st := s.byID["b2"]; st.Status != Failed || st.Exit != 1 {
		t.Errorf("b2: %v exit %d", st.Status, st.Exit)
	}
	if st := s.byID["a1"]; len(st.Children) != 1 || st.Children[0].ID != "a1g" {
		t.Errorf("subagent children: %+v", st.Children)
	}
	if p := s.Pending(); len(p) != 1 || p[0].ID != "b3" {
		t.Errorf("pending: %+v", p)
	}
	s.Apply(host.Answered{ID: "req-1"}, at(31))
	if st := s.byID["b3"]; st.Status != Running || st.Approval != nil {
		t.Errorf("after answer: %v", st.Status)
	}
}

func TestRender(t *testing.T) {
	s := session()
	out := plain(s.Render(Options{Width: 110, Now: at(40), History: HistoryOpen}))
	if os.Getenv("CONVO_SHOW") != "" {
	}
	want := []string{
		"▌  the ux right now is totally broken when i attach",      // your message, on its fill
		"4 steps   10s   $0.52",                                    // the turn's foot
		"Looking at how attach restores the terminal modes.",       // narration
		"▸ 4 steps  read, search, go build, edit  +2 −1 in 1 file", // its steps, a batch
		"▏ Fixed the alt screen.",                                  // answer on the conversation axis, markdown stripped
		"▌  add modern key stuff to input too",
		"✗ $ in internal/ui · go vet ./...", // cd leads, quieter
		"exit 1",
		"editor.go:41 unreachable code", // a failed build says where, in its card
		"✎ internal/ui/editor.go",
		"new · 3 lines",
		"⇉ Explore  find the preview pane",
		"1 step ",
		"⌕ previewLines", // a running subagent shows its steps
		"● $ go test -race ./...",
		"waiting on you",
		"Building the line editor",
		"Building the line editor",
	}
	for _, w := range want {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	if strings.Contains(out, "60 ") && strings.Contains(out, "fmt.Fprint(out, y)") {
		t.Errorf("a successful edit's diff should stay closed by default")
	}
}

func TestFoldAndOpen(t *testing.T) {
	s := session()
	// Three more finished turns push the first one out of the recent pair.
	for i := 0; i < 1; i++ {
		s.Apply(headless.Result{Subtype: "success"}, at(50))
		s.Apply(host.Sent{Text: "and another"}, at(51))
		s.Apply(say("Done."), at(52))
		s.Apply(headless.Result{Subtype: "success"}, at(53))
	}
	out := plain(s.Render(Options{Width: 110, Now: at(60), Open: map[string]bool{"t1": false}}))
	if !strings.Contains(out, "▸ 4 steps · 10s · $0.52 · show") || !strings.Contains(out, "the ux right now is totally broken when i attach") || strings.Contains(out, "Fixed the alt screen.") {
		t.Errorf("a collapsed turn keeps its message and hides the rest:\n%s", out)
	}
	// Opening a step shows its diff; opening the folded run lists its steps.
	o := Options{Width: 110, Now: at(60), Open: map[string]bool{"t1": true, "t1:s:e1": true, "t1:run:1": true}}
	out = plain(s.Render(o))
	for _, w := range []string{"61 + \t\tfmt.Fprint(out, y)", "61 − \t\tfmt.Fprint(out, x)", "◧ internal/daemon/attach.go"} {
		w = strings.ReplaceAll(w, "\t", "    ")
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
}

func TestStreamingAndCrash(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "go"}, at(0))
	s.Apply(headless.Delta{Text: "Work"}, at(1))
	s.Apply(headless.Delta{Text: "ing on it"}, at(1))
	if got := s.Turns[0].Items[0].Text; got != "Working on it" {
		t.Fatalf("stream: %q", got)
	}
	// The whole message replaces the streamed text rather than repeating it.
	s.Apply(say("Working on it."), at(2))
	if n := len(s.Turns[0].Items); n != 1 || s.Turns[0].Items[0].Text != "Working on it." {
		t.Fatalf("items after whole message: %d", n)
	}
	s.Apply(toolUse("x", "Bash", map[string]any{"command": "sleep 100"}), at(3))
	s.Apply(host.InfoEvent{Info: host.Info{State: "idle", Error: "exit status 1"}}, at(4))
	tn := s.Turns[0]
	if tn.Live || !strings.HasPrefix(tn.Err, "claude exited mid-turn") || s.byID["x"].Status != Lost {
		t.Fatalf("crash: live=%v err=%q status=%v", tn.Live, tn.Err, s.byID["x"].Status)
	}
	out := plain(s.Render(Options{Width: 100, Now: at(5)}))
	if !strings.Contains(out, "✗ The agent stopped mid-turn") || !strings.Contains(out, "│ claude exited mid-turn: exit status 1") || !strings.Contains(out, "next message starts it again") || !strings.Contains(out, "◌ $ sleep 100") {
		t.Errorf("crash render:\n%s", out)
	}
}

func TestTasks(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "plan"}, at(0))
	s.Apply(toolUse("c1", "TaskCreate", map[string]any{"subject": "Build editor", "activeForm": "Building editor"}), at(1))
	s.Apply(toolResult("c1", "Task #1 created successfully: Build editor", false, nil), at(1))
	s.Apply(toolUse("c2", "TaskCreate", map[string]any{"subject": "Wire keys"}), at(2))
	s.Apply(toolResult("c2", "Task #2 created successfully: Wire keys", false, nil), at(2))
	s.Apply(toolUse("u1", "TaskUpdate", map[string]any{"taskId": "1", "status": "in_progress"}), at(3))
	now, done, total := s.Current()
	if now == nil || now.Subject != "Build editor" || done != 0 || total != 2 {
		t.Fatalf("current: %+v %d/%d", now, done, total)
	}
	out := plain(s.Render(Options{Width: 100, Now: at(4)}))
	if strings.Contains(out, "TaskCreate") || strings.Contains(out, "TaskUpdate") {
		t.Errorf("task bookkeeping should not show as steps:\n%s", out)
	}
}

func TestCacheReuse(t *testing.T) {
	s := session()
	s.Apply(headless.Result{Subtype: "success"}, at(50))
	o := Options{Width: 100, Now: at(60)}
	a := s.Render(o)
	b := s.Render(o)
	if &a[0] == &b[0] {
		t.Skip("slices alias; content equality is what matters")
	}
	if plain(a) != plain(b) {
		t.Error("cached render differs")
	}
	first := s.cache[s.Turns[0]]
	s.Render(Options{Width: 100, Now: at(70), Tick: 3})
	if s.cache[s.Turns[0]].key != first.key {
		t.Error("a finished turn should not redraw when only the clock moves")
	}
}

func TestTint(t *testing.T) {
	got := tint(`go test -run "Foo Bar" ./... && echo $HOME | head`)
	if stripANSI(got) != `go test -run "Foo Bar" ./... && echo $HOME | head` {
		t.Fatalf("tint changed the text: %q", stripANSI(got))
	}
	if !strings.Contains(got, cOrange+"&&") || !strings.Contains(got, cGreen+`"Foo Bar"`) || !strings.Contains(got, cBlue+"$HOME") {
		t.Errorf("tint colours: %q", got)
	}
}

func TestOverview(t *testing.T) {
	s := New()
	s.Info = host.Info{Cwd: "/work", Effort: "high", PermissionMode: "auto", Model: "claude-opus-5-5[1m]"}
	s.Apply(host.Sent{Text: "go"}, at(0))
	use := func(id, model, parent string, read, write int, sec int) {
		s.Apply(headless.Message{Role: "assistant", ID: id, Model: model, ParentToolUseID: parent,
			Usage:  &headless.Usage{InputTokens: 10, OutputTokens: 50, CacheReadInputTokens: read, CacheCreationInputTokens: write},
			Blocks: []headless.Block{{Type: "text", Text: "…"}}}, at(sec))
	}
	use("m1", "claude-opus-5-5[1m]", "", 0, 20000, 1) // first request: cold is expected
	s.Apply(toolUse("b1", "Bash", map[string]any{"command": "ls"}), at(2))
	s.Apply(toolResult("b1", "x", false, nil), at(3))
	s.Apply(toolUse("b2", "Bash", map[string]any{"command": "false"}), at(4))
	s.Apply(toolResult("b2", "Exit code 1", true, nil), at(5))
	s.Apply(toolUse("a1", "Task", map[string]any{"subagent_type": "Explore", "description": "look"}), at(6))
	use("s1", "claude-haiku-4-5-20251001", "a1", 0, 9000, 7)
	use("m2", "claude-opus-5-5[1m]", "", 20000, 500, 8)
	use("m3", "claude-opus-5-5[1m]", "", 100, 21000, 8+3700) // an hour idle: the cache went cold
	s.Apply(headless.Result{Subtype: "success", CostUSD: 1.25}, at(3710))

	cold := s.ColdStarts()
	// The session's first request and the subagent's first are expected;
	// the one after an hour idle is the cache's hour running out.
	if len(cold) != 3 || !cold[0].Expected || !strings.HasPrefix(cold[1].Reason, "new subagent") ||
		cold[2].Gap != 3700*time.Second || !strings.HasPrefix(cold[2].Reason, "idle") {
		t.Fatalf("cold starts: %+v", cold)
	}
	out := plain(s.Overview(Options{Width: 110, Now: at(3720)}))
	if os.Getenv("CONVO_SHOW") != "" {
	}
	for _, w := range []string{
		"now  opus 5.5 · 1M · effort high · permissions auto",
		"context of 1M",
		"$1.25",
		"1h 01m",
		"3  ✗1",
		"Tools  3 calls",
		"Bash",
		"3 cold starts, all expected",
		"1 × new subagent · 1 × session start",
		"idle past the cache's hour · 1h 01m idle · main · rewrote 21k",
		"Explore   ×1    haiku 4.5",
	} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	if PrettyModel("claude-sonnet-5") != "sonnet 5" {
		t.Errorf("pretty: %s", PrettyModel("claude-sonnet-5"))
	}
}

func TestTailTranscript(t *testing.T) { tailTranscript(t, (*Tail).Read) }

// Fetch then Take, as the UI follows a transcript, reads the same as Read.
func TestTailFetchTake(t *testing.T) {
	tailTranscript(t, func(tl *Tail) (bool, error) {
		f, err := tl.Fetch()
		if err != nil {
			return false, err
		}
		return tl.Take(f), nil
	})
}

func tailTranscript(t *testing.T, read func(*Tail) (bool, error)) {
	dir := t.TempDir()
	path := dir + "/s.jsonl"
	lines := []string{
		`{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"fix the test"},"origin":{"kind":"human"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:00:02Z","effort":"high","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"go test ./..."}}],"usage":{"input_tokens":5,"output_tokens":3}}}`,
		`{"type":"user","timestamp":"2026-09-23T20:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Exit code 1\nFAIL x","is_error":true}]},"toolUseResult":{"stdout":"FAIL x","stderr":""}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Fixed it."}]}}`,
		`{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:00:13Z"}`,
		`{"type":"user","isMeta":true,"message":{"role":"user","content":"caveat"}}`,
		`{"type":"user","timestamp":"2026-09-23T20:05:00Z","message":{"role":"user","content":"<command-name>/compact</command-name><command-args></command-args>"}}`,
	}
	if err := os.WriteFile(path, []byte(strings.Join(lines[:3], "\n")+"\n"+lines[3][:20]), 0o644); err != nil {
		t.Fatal(err)
	}
	tl := NewTail(path)
	if ch, err := read(tl); err != nil || !ch {
		t.Fatalf("first read: %v %v", ch, err)
	}
	if len(tl.Sess.Turns) != 1 || tl.Sess.byID["t1"].Status != Failed || tl.Sess.Turns[0].Effort != "high" {
		t.Fatalf("after part: %+v", tl.Sess.Turns)
	}
	// The rest of the half-written line and the lines after it arrive later.
	f, _ := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0)
	f.WriteString(lines[3][20:] + "\n" + strings.Join(lines[4:], "\n") + "\n")
	f.Close()
	if ch, _ := read(tl); !ch {
		t.Fatal("second read saw nothing")
	}
	s := tl.Sess
	if len(s.Turns) != 2 || s.Turns[0].Live || s.Turns[0].Outcome() != "Fixed it." || s.Turns[1].Cause != "/compact" {
		t.Fatalf("turns: %d, first live=%v outcome=%q second=%q", len(s.Turns), s.Turns[0].Live, s.Turns[0].Outcome(), s.Turns[1].Cause)
	}
	if ch, _ := read(tl); ch {
		t.Error("nothing new should mean no change")
	}
	// Rewritten shorter: the conversation starts over.
	if err := os.WriteFile(path, []byte(lines[0]+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if ch, _ := read(tl); !ch || len(tl.Sess.Turns) != 1 || tl.Sess.Turns[0].Prompt != "fix the test" {
		t.Fatalf("after a rewrite: changed=%v turns=%d", ch, len(tl.Sess.Turns))
	}
}

func TestSubagents(t *testing.T) {
	dir := t.TempDir()
	main := dir + "/sess.jsonl"
	sub := dir + "/sess/subagents"
	_ = os.MkdirAll(sub, 0o755)
	_ = os.WriteFile(main, []byte(""), 0o644)
	_ = os.WriteFile(sub+"/agent-a1.meta.json", []byte(`{"agentType":"Explore","description":"find the pane","toolUseId":"toolu_1","model":"haiku"}`), 0o644)
	_ = os.WriteFile(sub+"/agent-a1.jsonl", []byte(strings.Join([]string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find where the pane is drawn"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"g1","name":"Grep","input":{"pattern":"previewLines"}}]}}`,
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"internal/ui/view.go"}]}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:03Z","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"It's in view.go."}]}}`,
	}, "\n")+"\n"), 0o644)
	subs := ListSubagents(main)
	if len(subs) != 1 || subs[0].Type != "Explore" || subs[0].ToolUseID != "toolu_1" || subs[0].Model != "haiku" {
		t.Fatalf("subagents: %+v", subs)
	}
	tl := SubagentTail(subs[0].Path)
	if _, err := tl.Read(); err != nil {
		t.Fatal(err)
	}
	tl.Sess.Apply(headless.Result{Subtype: "success"}, at(10))
	out := plain(tl.Sess.Render(Options{Width: 100, Now: at(10)}))
	for _, w := range []string{"find where the pane is drawn", "⌕ previewLines", "It's in view.go."} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
}

// A message rush's inbox handed a subagent at a tool call's end is yours,
// mid-turn, without the note the hook put before it; other hooks' aren't.
func TestSubagentToldMidTurn(t *testing.T) {
	path := t.TempDir() + "/agent-a1.jsonl"
	note, _ := jsonx.Marshal(host.TellNote + "also echo pineapple")
	_ = os.WriteFile(path, []byte(strings.Join([]string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"count to five"}}`,
		`{"type":"attachment","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","attachment":{"type":"hook_additional_context","content":["PONYTAIL MODE ACTIVE"],"hookEvent":"SubagentStart"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"echo one"}}]}}`,
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"b1","content":"one"}]}}`,
		`{"type":"attachment","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","attachment":{"type":"hook_success","content":"","hookEvent":"PostToolUse"}}`,
		`{"type":"attachment","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","attachment":{"type":"hook_additional_context","content":[` + string(note) + `],"hookName":"PostToolUse:Bash","hookEvent":"PostToolUse"}}`,
	}, "\n")+"\n"), 0o644)
	tl := SubagentTail(path)
	if _, err := tl.Read(); err != nil {
		t.Fatal(err)
	}
	var said []string
	for _, it := range tl.Sess.Turns[len(tl.Sess.Turns)-1].Items {
		if it.Kind == KInterject {
			said = append(said, it.Text)
		}
	}
	if len(tl.Sess.Turns) != 1 || len(said) != 1 || said[0] != "also echo pineapple" {
		t.Fatalf("%d turns, said %q", len(tl.Sess.Turns), said)
	}
}

// A run's row numbers read the same whether the run was read in full or
// only for its numbers.
func TestSubagentStatsMatchFull(t *testing.T) {
	path := t.TempDir() + "/agent-a1.jsonl"
	_ = os.WriteFile(path, []byte(strings.Join([]string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find where the pane is drawn\nand say why"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","model":"claude-haiku-4-5","role":"assistant","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":900},"content":[{"type":"tool_use","id":"g1","name":"Grep","input":{"pattern":"previewLines"}}]}}`,
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"g1","content":"internal/ui/view.go"}]}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:03Z","message":{"id":"m2","model":"claude-haiku-4-5","role":"assistant","usage":{"input_tokens":12,"output_tokens":40},"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"false"}}]}}`,
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:05Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"b1","is_error":true,"content":"Exit code 1"}]}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:06Z","message":{"id":"m3","model":"claude-haiku-4-5","role":"assistant","usage":{"input_tokens":1,"output_tokens":9},"content":[{"type":"text","text":"**It's in** view.go.\nMore detail."}]}}`,
	}, "\n")+"\n"), 0o644)
	full, light := SubagentTail(path), SubagentStats(path)
	_, _ = full.Read()
	_, _ = light.Read()
	now := at(100)
	if f, l := full.Sess.Totals(now), light.Sess.Totals(now); f != l {
		t.Errorf("totals: full %+v, light %+v", f, l)
	}
	if f, l := full.Sess.Cost(), light.Sess.Cost(); f != l || f == 0 {
		t.Errorf("cost: full %v, light %v", f, l)
	}
	if f, l := full.Sess.LastWords(), light.Sess.LastWords(); f != l || l != "It's in view.go." {
		t.Errorf("last words: full %q, light %q", f, l)
	}
	if !full.Sess.First.Equal(light.Sess.First) || !full.Sess.Last.Equal(light.Sess.Last) {
		t.Errorf("span differs")
	}
	if n := len(light.Sess.byID); n != 0 {
		t.Errorf("light kept %d steps", n)
	}
}

// A run read only for its numbers still says what it's doing: the call
// still out, in words, and nothing once it has come back.
func TestSubagentStatsDoing(t *testing.T) {
	path := t.TempDir() + "/agent-a1.jsonl"
	lines := []string{
		`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"find where the pane is drawn"}}`,
		`{"type":"assistant","isSidechain":true,"timestamp":"2026-09-23T20:00:01Z","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/w/internal/ui/view.go"}}]}}`,
	}
	_ = os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644)
	light := SubagentStats(path)
	_, _ = light.Read()
	if d, since := light.Sess.Doing(); d != "reading view.go" || !since.Equal(time.Date(2026, 9, 23, 20, 0, 1, 0, time.UTC)) {
		t.Errorf("doing: %q since %v", d, since)
	}
	f, _ := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0)
	_, _ = f.WriteString(`{"type":"user","isSidechain":true,"timestamp":"2026-09-23T20:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content":"package ui"}]}}` + "\n")
	_ = f.Close()
	_, _ = light.Read()
	if d, _ := light.Sess.Doing(); d != "" {
		t.Errorf("doing after its result: %q", d)
	}
	if did := light.Sess.Did(); len(did) != 1 || did[0] != "reading view.go" {
		t.Errorf("did: %q", did)
	}
}

func TestChanges(t *testing.T) {
	s := session()
	s.Apply(headless.Result{Subtype: "success"}, at(50))
	ch := s.Changes()
	if len(ch) != 2 || ch[0].Add != 2 || ch[0].Del != 1 || !ch[1].New || ch[1].Add != 3 {
		t.Fatalf("changes: %+v %+v", ch[0], ch[1])
	}
	out := plain(s.ChangesView(Options{Width: 110, Now: at(60), Open: map[string]bool{"chg:/work/rush/internal/daemon/attach.go": true}}))
	for _, w := range []string{"This session  2 files · +5 −1", "internal/daemon/attach.go", "#1", "61 + ", "new · 3 lines"} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
}

func TestSearch(t *testing.T) {
	s := session()
	cases := []struct {
		q    string
		want []string // refs
	}{
		{"is:failed", []string{"t2:s:b2"}},
		{"is:edit", []string{"t1:s:e1", "t2:s:w1"}},
		{"file:editor.go", []string{"t2:s:w1"}},
		{"alt screen", []string{"t1"}},
		{"is:you attach", []string{"t1"}},
		{"unreachable", []string{"t2:s:b2"}},
		{"turn:2 is:cmd", []string{"t2:s:b2", "t2:s:b3"}},
	}
	for _, c := range cases {
		var got []string
		for _, h := range s.Search(c.q) {
			got = append(got, h.Ref)
		}
		if strings.Join(got, ",") != strings.Join(c.want, ",") {
			t.Errorf("%q: got %v want %v", c.q, got, c.want)
		}
	}
}

func TestShellTurnsAndStyling(t *testing.T) {
	dir := t.TempDir()
	path := dir + "/s.jsonl"
	lines := []string{
		`{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"<bash-input>ls /tmp</bash-input>"}}`,
		`{"type":"user","timestamp":"2026-09-23T20:00:01Z","message":{"role":"user","content":"<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr>"}}`,
		`{"type":"user","timestamp":"2026-09-23T20:01:00Z","message":{"role":"user","content":"/design:design-critique look at @internal/ui/view.go and https://example.com [Image #1]"}}`,
	}
	_ = os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644)
	tl := NewTail(path)
	if _, err := tl.Read(); err != nil {
		t.Fatal(err)
	}
	s := tl.Sess
	if len(s.Turns) != 2 || s.Turns[0].Prompt != "! ls /tmp" || s.Turns[0].Live {
		t.Fatalf("turns: %d %+v", len(s.Turns), s.Turns[0])
	}
	st := s.byID["you-1"]
	if st == nil || st.Status != OK || !strings.Contains(st.Output, "a\nb") {
		t.Fatalf("shell step: %+v", st)
	}
	s.Apply(headless.Result{Subtype: "success"}, at(100))
	raw := s.Render(Options{Width: 120, Now: at(100)})
	out := plain(raw)
	if !strings.Contains(out, "▌  $ ls /tmp") || strings.Contains(out, "bash-input") {
		t.Errorf("shell heading:\n%s", out)
	}
	joined := ""
	for _, l := range raw {
		joined += l.Text
	}
	for _, want := range []string{cWhite + bold + "/design:design-critique", cWhite + bold + "@internal/ui/view.go", "\x1b]8;;https://example.com", dim("▣ Image #1")} {
		if !strings.Contains(joined, want) {
			t.Errorf("missing styled %q", want)
		}
	}
}

func TestInjectedPrompts(t *testing.T) {
	cases := []struct{ in, from, text string }{
		{"<task-notification>\n<task-id>a1</task-id>\n<status>completed</status>\n<summary>Agent \"lint lane\" finished</summary>\n</task-notification>", "background task · completed", `Agent "lint lane" finished`},
		{`<cross-session-message from="uds:/x" from-name="agent wrapper">rush: my edits are committed</cross-session-message>`, "message from agent wrapper", "rush: my edits are committed"},
	}
	for _, c := range cases {
		from, text, ok := Injected(c.in)
		if !ok || from != c.from || text != c.text {
			t.Errorf("%q: got %q %q %v", c.in[:20], from, text, ok)
		}
	}
	if _, _, ok := Injected("please fix <b>this</b>"); ok {
		t.Error("ordinary text starting with a word isn't injected")
	}
}

func TestSubagentNumbers(t *testing.T) {
	s := New()
	t0 := time.Unix(1000, 0)
	// Claude Code writes one line per content block of the same response;
	// each carries the usage so far and must count once.
	for i, out := range []int{5, 40, 12} {
		s.Apply(headless.Message{Role: "assistant", ID: "msg_1", Model: "claude-opus-4-1",
			Usage:  &headless.Usage{InputTokens: 100, OutputTokens: out},
			Blocks: []headless.Block{{Type: "text", Text: "hi"}}}, t0.Add(time.Duration(i)*time.Minute))
	}
	s.Apply(headless.Message{Role: "assistant", ID: "x", Model: "<synthetic>", Usage: &headless.Usage{InputTokens: 9}}, t0)
	if len(s.Requests) != 1 || s.Requests[0].Usage.Output != 40 || s.Requests[0].Usage.Input != 100 {
		t.Fatalf("requests = %+v", s.Requests)
	}
	if s.Last.Sub(s.First) != 2*time.Minute {
		t.Fatalf("span = %v", s.Last.Sub(s.First))
	}
	s.noteTask(raw("<task-notification><task-id>abc</task-id><status>killed</status></task-notification>"))
	// Claude Code's killed is one you stopped.
	if s.TaskStatus["abc"] != "stopped" {
		t.Fatalf("status = %q", s.TaskStatus["abc"])
	}
}

// A running turn's rail ends at the last thing drawn, folded or open.
func TestRailEnds(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "merge origin"}, at(0))
	for _, open := range []bool{true, false} {
		ref := ""
		lines := s.Render(Options{Width: 90, Now: at(5)})
		for _, l := range lines {
			if l.Ref != "" {
				ref = l.Ref
			}
		}
		lines = s.Render(Options{Width: 90, Now: at(5), Open: map[string]bool{ref: open}})
		// The rule never ends on a bare row.
		if n := len(lines); n >= 1 && strings.TrimSpace(stripANSI(lines[n-1].Text)) == "▏" {
			t.Fatalf("open=%v: the rail ends on an empty row\n%s", open, plain(lines))
		}
	}
}

func TestLiveLine(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "think hard"}, at(0))
	s.Apply(headless.BlockStart{Index: 0, Type: "thinking"}, at(1))
	out := plain(s.Render(Options{Width: 100, Now: at(3)}))
	if !strings.Contains(out, pick(musings, at(1))+"…  2.0s") || !s.Fast {
		t.Fatalf("a timer under 5s shows tenths and asks for fast frames (fast %v):\n%s", s.Fast, out)
	}
	out = plain(s.Render(Options{Width: 100, Now: at(9)}))
	if !strings.Contains(out, pick(musings, at(1))+"…  8s") || !strings.Contains(out, "8s  turn 9s") {
		t.Fatalf("no thinking line:\n%s", out)
	}
	s.Apply(headless.BlockStart{Index: 1, Type: "text"}, at(10))
	s.Apply(headless.Delta{Index: 1, Text: strings.Repeat("word ", 800)}, at(10))
	out = plain(s.Render(Options{Width: 100, Now: at(12)}))
	if !strings.Contains(out, pick(writings, at(0))+"…  12s") || strings.Contains(out, "▏\n▏\n▌   ") {
		t.Fatalf("no writing line:\n%s", out)
	}
	s.Apply(headless.Result{Subtype: "success"}, at(13))
	if out = plain(s.Render(Options{Width: 100, Now: at(14)})); strings.Contains(out, pick(writings, at(0))+"…") {
		t.Fatal("a finished turn has no live line")
	}
}

func TestFailureInBrief(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "compare"}, at(0))
	s.Apply(toolUse("b1", "Bash", map[string]any{"command": "python3 - <<'EOF'\nimport yaml\nprint(1)\nEOF", "description": "Compare lockfile"}), at(1))
	s.Apply(toolResult("b1", "Exit code 1\nTraceback (most recent call last):\n  File \"<stdin>\", line 1\nModuleNotFoundError: No module named 'yaml'", true,
		map[string]any{"stdout": "", "stderr": "Traceback (most recent call last):\n  File \"<stdin>\", line 1\nModuleNotFoundError: No module named 'yaml'"}), at(2))
	// A step after it, so the failure isn't the newest (which shows opened).
	s.Apply(toolUse("r1", "Read", map[string]any{"file_path": "/work/go.mod"}), at(2))
	s.Apply(toolResult("r1", "module x", false, nil), at(2))
	out := plain(s.Render(Options{Width: 110, Now: at(3)}))
	if !strings.Contains(out, "▸ ModuleNotFoundError: No module named 'yaml'") || strings.Contains(out, "import yaml") {
		t.Fatalf("brief failure:\n%s", out)
	}
	ref := ""
	for _, l := range s.Render(Options{Width: 110, Now: at(3)}) {
		if strings.Contains(l.Ref, ":s:b1") {
			ref = l.Ref
			break
		}
	}
	out = plain(s.Render(Options{Width: 110, Now: at(3), Open: map[string]bool{ref: true}}))
	if !strings.Contains(out, "$ python3 - <<'EOF'") || !strings.Contains(out, "1▸ import yaml") || !strings.Contains(out, "✗ ModuleNotFoundError") || !strings.Contains(out, "2  print(1)") || strings.Contains(out, "$ import yaml") {
		t.Fatalf("opened failure, numbered, at the line it stopped:\n%s", out)
	}
}

func TestSearchEdges(t *testing.T) {
	if i, j := findFold("İstanbul — Ünïcode ERROR here", "error"); i < 0 || "İstanbul — Ünïcode ERROR here"[i:j] != "ERROR" {
		t.Fatalf("findFold = %d %d", i, j)
	}
	long := strings.Repeat("— ", 40) + "needle"
	if sn := snippet(long, []string{"needle"}); !utf8.ValidString(sn) {
		t.Fatalf("snippet cut a character: %q", sn)
	}
	p := parseQuery("turn:13-10 is:foo x")
	if p.from != 10 || p.to != 13 || len(p.unknown) != 1 {
		t.Fatalf("query = %+v", p)
	}
	if p := parseQuery("turn:5-"); p.from != 5 || p.to < 1000 {
		t.Fatalf("open range = %+v", p)
	}
	s := session()
	if len(s.Search("is:agent")) == 0 || len(s.Search("is:claude")) != len(s.Search("is:agent")) {
		t.Fatal("is:agent alone lists what the agent said, and is:claude still does")
	}
	_ = s.SearchView("İ", Options{Width: 100}) // mustn't panic
}

func TestConvoReviewFixes(t *testing.T) {
	if isRejection("open /etc/x: permission denied") {
		t.Error("EACCES isn't a rejection")
	}
	if !isRejection("The user doesn't want to proceed with this tool use.") {
		t.Error("a refusal is a rejection")
	}
	if got := cleanOutput("a\x1b]0;title\x07b\x1b[2Jc\x1b[31md\te"); got != "abcd\te" {
		t.Errorf("cleanOutput = %q", got)
	}
	st := &Step{Status: OK, Output: "Exit code 3 is what the docs say"}
	if exitCode(st) != 0 {
		t.Error("exit code parsed from a success's output")
	}
	// A subagent message whose parent we never saw stays out of the turn.
	s := New()
	s.Apply(host.Sent{Text: "go"}, at(0))
	s.Apply(headless.Message{Role: "assistant", ParentToolUseID: "unknown", Blocks: []headless.Block{{Type: "text", Text: "sub words"}}}, at(1))
	if strings.Contains(plain(s.Render(Options{Width: 90, Now: at(2)})), "sub words") {
		t.Error("an unknown subagent's words leaked into the turn")
	}
	s.Apply(headless.Message{Role: "user", Blocks: []headless.Block{{Type: "text", Text: "[Request interrupted by user]"}}}, at(3))
	if len(s.Turns) != 1 || !s.Turns[0].Stopped {
		t.Errorf("interrupt: %d turns, stopped=%v", len(s.Turns), s.Turns[0].Stopped)
	}
}

func TestAnswerTable(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "compare"}, at(0))
	s.Apply(say("Here:\n\n| Benchmark | Before | After |\n|---|---|---|\n| Frame | **1.38ms** | 0.33ms |\n| Rail | 202µs | 14µs |"), at(1))
	s.Apply(headless.Result{Subtype: "success"}, at(2))
	out := plain(s.Render(Options{Width: 100, Now: at(3)}))
	for _, want := range []string{
		"Benchmark  Before  After", "Frame      1.38ms  0.33ms", "Rail       202µs   14µs",
	} {
		if !strings.Contains(out, want) {
			t.Fatalf("missing %q:\n%s", want, out)
		}
	}
	if strings.Contains(out, "|---") {
		t.Fatal("separator row drawn raw")
	}
}

// Narration, words with steps after them, draws its tables too.
func TestNarrationTable(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "audit"}, at(0))
	s.Apply(say("Found:\n\n| # | Screen |\n|---|---|\n| 1 | **Every page** |"), at(1))
	s.Apply(toolUse("b1", "Bash", map[string]any{"command": "go build ./..."}), at(2))
	s.Apply(toolResult("b1", "", false, nil), at(3))
	s.Apply(say("Done."), at(4))
	s.Apply(headless.Result{Subtype: "success"}, at(5))
	out := plain(s.Render(Options{Width: 100, Now: at(6), Open: map[string]bool{"t1": true}}))
	for _, want := range []string{"# · Screen", "1 · Every page"} {
		if !strings.Contains(out, want) {
			t.Fatalf("missing %q:\n%s", want, out)
		}
	}
	if strings.Contains(out, "|---") || strings.Contains(out, "| 1 |") {
		t.Fatalf("narration table drawn raw:\n%s", out)
	}
}

func TestCompactDivider(t *testing.T) {
	path := filepath.Join(t.TempDir(), "s.jsonl")
	lines := []string{
		`{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"do the thing"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:00:05Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Done."}]}}`,
		`{"type":"system","subtype":"turn_duration","timestamp":"2026-09-23T20:00:06Z"}`,
		`{"type":"system","subtype":"compact_boundary","timestamp":"2026-09-23T20:01:00Z","compactMetadata":{"trigger":"auto","preTokens":592789,"postTokens":11658}}`,
		`{"type":"user","timestamp":"2026-09-23T20:01:01Z","message":{"role":"user","content":"This session is being continued from a previous conversation that ran out of context. Summary: stuff"}}`,
	}
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	tl := NewTail(path)
	if _, err := tl.Read(); err != nil {
		t.Fatal(err)
	}
	s := tl.Sess
	if len(s.Turns) != 1 {
		t.Fatalf("the summary opened a turn: %d turns", len(s.Turns))
	}
	out := plain(s.Render(Options{Width: 120, Now: at(0), Open: map[string]bool{"t1": true}}))
	if !strings.Contains(out, "◇ context compacted  auto · 593k → 12k tokens · ctrl+o shows the summary") {
		t.Fatalf("no divider:\n%s", out)
	}
}

func TestChangesHunks(t *testing.T) {
	s := session()
	var ref string
	for _, l := range s.ChangesView(Options{Width: 110, Now: at(40)}) {
		if strings.HasPrefix(l.Ref, "chg:") {
			ref = l.Ref
			break
		}
	}
	if ref == "" {
		t.Fatal("no file rows")
	}
	lines := s.ChangesView(Options{Width: 110, Now: at(40), Open: map[string]bool{ref: true}, Marks: map[string]bool{strings.TrimPrefix(ref, "chg:"): true}})
	out, jump := plain(lines), false
	for _, l := range lines {
		if strings.HasPrefix(l.Ref, "jump:t") && strings.Contains(l.Ref, ":s:") {
			jump = true
		}
	}
	if !jump || !strings.Contains(out, "@ line") || !strings.Contains(out, "✓") || !strings.Contains(out, "1 of") {
		t.Fatalf("hunks and marks:\n%s", out)
	}
}

func TestNotice(t *testing.T) {
	path := filepath.Join(t.TempDir(), "s.jsonl")
	lines := []string{
		`{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"/rush"}}`,
		`{"type":"system","subtype":"informational","level":"warning","timestamp":"2026-09-23T20:00:01Z","content":"Unknown command: /rush. Did you mean /stop?"}`,
	}
	_ = os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644)
	tl := NewTail(path)
	_, _ = tl.Read()
	if out := plain(tl.Sess.Render(Options{Width: 100, Now: at(0)})); !strings.Contains(out, "● Unknown command: /rush. Did you mean /stop?") {
		t.Fatalf("notice missing:\n%s", out)
	}
}

func TestHistory(t *testing.T) {
	path := filepath.Join(t.TempDir(), "s.jsonl")
	lines := []string{
		`{"type":"user","timestamp":"2026-09-23T20:00:00Z","message":{"role":"user","content":"first ask"}}`,
		`{"type":"assistant","timestamp":"2026-09-23T20:00:05Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Done."}]}}`,
		`{"type":"user","timestamp":"2026-09-23T21:00:00Z","message":{"role":"user","content":"after the host started"}}`,
	}
	_ = os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644)
	s := History(path, time.Date(2026, 9, 23, 20, 30, 0, 0, time.UTC))
	if len(s.Turns) != 1 || s.Turns[0].Prompt != "first ask" || s.Turns[0].Live {
		t.Fatalf("history = %d turns, %+v", len(s.Turns), s.Turns)
	}
}

func TestShowDrawsAFigure(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "draw the flow"}, at(0))
	drawing := "\n┌──────┐    ┌──────┐\n│ host │ ─▶ │ ui   │\n└──────┘    └──────┘\n\n"
	for i := 0; i < 3; i++ { // three clean steps before it, so a run folds
		id := fmt.Sprint("r", i)
		s.Apply(toolUse(id, "Read", map[string]any{"file_path": "/x"}), at(1))
		s.Apply(toolResult(id, "…", false, nil), at(1))
	}
	s.Apply(toolUse("d1", agtools.Show, map[string]any{"title": "Message flow", "drawing": drawing}), at(2))
	s.Apply(toolResult("d1", "Shown to the user.", false, nil), at(2))
	s.Apply(say("That's the flow."), at(3))
	s.Apply(headless.Result{Subtype: "success"}, at(3))
	lines := s.Render(Options{Width: 90, Now: at(4), Open: map[string]bool{"t1": true}})
	out := plain(lines)
	for _, want := range []string{"╭─ ◇ Message flow ", "│ ┌──────┐    ┌──────┐", "│ │ host │ ─▶ │ ui   │", "╰─"} {
		if !strings.Contains(out, want) {
			t.Fatalf("missing %q:\n%s", want, out)
		}
	}
	// The frame is square: every row of it the same width.
	var ws []int
	for _, l := range strings.Split(out[strings.LastIndex(out[:strings.Index(out, "╭─ ◇")], "\n")+1:], "\n") {
		if strings.ContainsAny(l, "╭│╰") && strings.ContainsAny(l, "╮│╯") && strings.Contains(l, "─") || strings.HasPrefix(strings.TrimSpace(l), "│") {
			ws = append(ws, cellw.String(l))
		}
	}
	for _, w := range ws {
		if w != ws[0] {
			t.Fatalf("ragged frame %v:\n%s", ws, out)
		}
	}
	// Its top edge is the row you pick.
	picked := false
	for _, l := range lines {
		if strings.HasSuffix(l.Ref, ":s:d1") && strings.Contains(stripANSI(l.Text), "Message flow") {
			picked = true
		}
	}
	if !picked {
		t.Fatalf("no pickable edge:\n%s", out)
	}
	// Wider than the pane: cut with a marker, and the bottom edge says so.
	wide := strings.Repeat("=", 200)
	s.Apply(host.Sent{Text: "wider"}, at(5))
	s.Apply(toolUse("d2", agtools.Show, map[string]any{"drawing": wide}), at(6))
	s.Apply(toolResult("d2", "ok", false, nil), at(6))
	out = plain(s.Render(Options{Width: 90, Now: at(7)}))
	if !strings.Contains(out, "=›") || !strings.Contains(out, "more columns") || strings.Contains(out, wide) {
		t.Fatalf("wide drawing:\n%s", out)
	}
	for _, l := range strings.Split(out, "\n") {
		if cellw.String(l) > 90 {
			t.Fatalf("row wider than the pane: %q", l)
		}
	}
}

func TestPastesAndImagesFold(t *testing.T) {
	in := "look at this:\n\n<pasted_content id=\"54b6\">\nline1\nline2\nline3\nline4\n</pasted_content id=\"54b6\">\n and fix it"
	if got := FoldPastes(in); got != "look at this: [pasted text #1 · 4 lines: line1 … line4] and fix it" {
		t.Fatalf("FoldPastes = %q", got)
	}
	var got []string
	EachPaste(in, func(s string) string { got = append(got, s); return "" })
	if len(got) != 1 || got[0] != "line1\nline2\nline3\nline4" {
		t.Fatalf("EachPaste = %q", got)
	}
	s := New()
	at := func(sec int) time.Time { return time.Unix(int64(sec), 0) }
	s.Apply(host.Sent{Text: in + "\n[image: /tmp/rush-images/shot.png]"}, at(0))
	s.Apply(headless.Result{Subtype: "success"}, at(1))
	s.Apply(host.Sent{Images: []string{"image", "image"}}, at(2))
	out := plain(s.Render(Options{Width: 120, Now: at(3)}))
	for _, want := range []string{"▤ pasted\n", "▌  line1\n", "▌  line4\n", "▣ shot.png", "▣ Image #1   ▣ Image #2"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	if strings.Contains(out, "pasted_content") || strings.Contains(out, "pasted text #") {
		t.Errorf("a short paste is its own text, not a chip:\n%s", out)
	}
}

// A subagent's row names the agent Claude Code says ran, which it resolves
// when the call left subagent_type out.
func TestAgentNameFromResult(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "look around"}, at(0))
	s.Apply(toolUse("a1", "Agent", map[string]any{"description": "find the pane"}), at(1))
	if out := plain(s.Render(Options{Width: 100, Now: at(2)})); !strings.Contains(out, "⇉ subagent  find the pane") {
		t.Fatalf("before its result:\n%s", out)
	}
	s.Apply(toolResult("a1", "done", false, map[string]any{"status": "completed", "agentType": "Explore"}), at(3))
	s.Apply(headless.Result{Subtype: "success"}, at(4))
	if out := plain(s.Render(Options{Width: 100, Now: at(4)})); !strings.Contains(out, "⇉ Explore  find the pane") {
		t.Fatalf("after its result:\n%s", out)
	}
}

// Screenshots sent mid-turn show as chips under your words, each whole on
// its row and named by the time that tells them apart.
func TestInterjectedScreenshots(t *testing.T) {
	s := New()
	at := func(sec int) time.Time { return time.Unix(int64(sec), 0) }
	s.Apply(host.Sent{Text: "fix it"}, at(0))
	var shots []string
	for i := range 11 {
		shots = append(shots, fmt.Sprintf("/Users/me/Desktop/Screenshot 2026-09-24 at 10.21.%02d.png", i))
	}
	s.Apply(host.Sent{Text: "then update the readme", Images: shots}, at(1))
	out := plain(s.Render(Options{Width: 80, Now: at(2)}))
	if strings.Contains(out, "2026-09-24") {
		t.Errorf("screenshot dates shown:\n%s", out)
	}
	for _, sh := range shots {
		if want := "▣ " + ImageLabel(sh); !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	if got := ImageLabel("/x/Screenshot 2026-09-24 at 10.21.03 AM.png"); got != "screenshot 10.21.03 AM" {
		t.Errorf("ImageLabel = %q", got)
	}
	if got := ImageLabel("/x/diagram.png"); got != "diagram.png" {
		t.Errorf("ImageLabel = %q", got)
	}
}

func TestNewestStepOpensAndWraps(t *testing.T) {
	long := "A replacement for the whole Claude Code interface, written in Go and built to be very fast, and then some more words."
	edit := func(id string) map[string]any {
		return map[string]any{"structuredPatch": []map[string]any{{"oldStart": 4, "oldLines": 1, "newStart": 4, "newLines": 1,
			"lines": []string{"-short", "+" + long}}}}
	}
	s := New()
	s.Apply(host.Sent{Text: "tagline"}, at(0))
	s.Apply(toolUse("e1", "Edit", map[string]any{"file_path": "/work/README.md"}), at(1))
	s.Apply(toolResult("e1", "ok", false, edit("e1")), at(2))
	out := plain(s.Render(Options{Width: 80, Now: at(3)}))
	if !strings.Contains(out, "− short") || !strings.Contains(out, "more words.") || strings.Contains(out, "›") {
		t.Fatalf("the newest edit opens, its long line wrapped whole:\n%s", out)
	}
	s.Apply(toolUse("e2", "Edit", map[string]any{"file_path": "/work/other.md"}), at(3))
	s.Apply(toolResult("e2", "ok", false, edit("e2")), at(4))
	out = plain(s.Render(Options{Width: 80, Now: at(5)}))
	if n := strings.Count(out, "− short"); n != 1 {
		t.Fatalf("a newer step folds the one before (%d open):\n%s", n, out)
	}
	ref := ""
	for _, l := range s.Render(Options{Width: 80, Now: at(5)}) {
		if strings.Contains(l.Ref, ":s:e2") {
			ref = l.Ref
		}
	}
	if !s.StepOpen(ref, false) || s.StepOpen(strings.Replace(ref, "e2", "e1", 1), false) {
		t.Fatalf("StepOpen(%q) disagrees with the drawing", ref)
	}
}

// Wide lets a row's numbers run to the right edge of a wide pane instead of
// stopping at capRow.
func TestRenderWide(t *testing.T) {
	widest := func(o Options) int {
		w := 0
		for _, l := range strings.Split(plain(session().Render(o)), "\n") {
			w = max(w, cellw.String(strings.TrimRight(l, " ")))
		}
		return w
	}
	if w := widest(Options{Width: 200, Now: at(40)}); w > capRow {
		t.Fatalf("rows ran to %d, past %d", w, capRow)
	}
	if w := widest(Options{Width: 200, Now: at(40), Wide: true}); w <= capRow || w > 200 {
		t.Fatalf("wide rows ran to %d of 200", w)
	}
}

// The UI asks a tail's Size (to watch its file) while a Fetch runs in the
// background: run with -race, this fails if that's a race.
func TestTailSizeDuringFetch(t *testing.T) {
	path := t.TempDir() + "/s.jsonl"
	os.WriteFile(path, []byte(strings.Repeat(`{"type":"user","message":{"role":"user","content":"hi"}}`+"\n", 2000)), 0o644)
	tl := NewTail(path)
	done := make(chan Fresh)
	go func() {
		f, _ := tl.Fetch()
		done <- f
	}()
	for range 1000 {
		_ = tl.Size()
	}
	tl.Take(<-done)
	if st, _ := os.Stat(path); tl.Size() != st.Size() {
		t.Fatalf("size %d, file %d", tl.Size(), st.Size())
	}
}

// History keeps what was said before its moment, and stops reading once
// the transcript is well past it.
func TestHistoryStopsPastItsMoment(t *testing.T) {
	path := t.TempDir() + "/s.jsonl"
	line := func(at, text string) string {
		return `{"type":"user","timestamp":"` + at + `","message":{"role":"user","content":"` + text + `"}}`
	}
	os.WriteFile(path, []byte(strings.Join([]string{
		line("2026-09-23T20:00:00Z", "before"),
		line("2026-09-23T20:10:00Z", "after"),
		line("2026-09-23T20:30:00Z", "long after"),
		"not json at all, never reached",
	}, "\n")+"\n"), 0o644)
	tl := NewTail(path)
	tl.before = time.Date(2026, 9, 23, 20, 5, 0, 0, time.UTC)
	if _, err := tl.Read(); err != nil {
		t.Fatal(err)
	}
	if len(tl.Sess.Turns) != 1 || tl.Sess.Turns[0].Prompt != "before" {
		t.Fatalf("turns: %+v", tl.Sess.Turns)
	}
	if !tl.past {
		t.Error("it read on past its moment")
	}
}

// A message split around its images at their markers reads back as the
// text that was typed.
func TestPromptJoinsTextAroundImages(t *testing.T) {
	raw := jsontext.Value(`[{"type":"text","text":"see [Image #1]"},{"type":"image"},{"type":"text","text":" and [Image #2]"},{"type":"image"},{"type":"text","text":" ok"}]`)
	text, images, ok := prompt(raw)
	if !ok || text != "see [Image #1] and [Image #2] ok" || len(images) != 2 {
		t.Fatalf("prompt = %q %v %v", text, images, ok)
	}
	if text, _, _ := prompt(jsontext.Value(`[{"type":"text","text":"one"},{"type":"text","text":"two"}]`)); text != "one\ntwo" {
		t.Fatalf("separate blocks = %q", text)
	}
}

func answerLines(t *testing.T, md string, width int) []string {
	t.Helper()
	s := New()
	var out []string
	for _, l := range s.Answer(md, width) {
		out = append(out, strings.TrimRight(stripANSI(l.Text), " "))
	}
	return out
}

// Nested lists are drawn at their depth: two columns a level, a bullet of
// their own, numbers kept, ordered and unordered mixed either way.
func TestAnswerNestedLists(t *testing.T) {
	md := "- one\n  - one a\n    - one a i\n  - one b\n- two\n\n1. first\n   - first bullet\n     1. deep number\n2. second\n\n* top\n    * four-space child"
	got := strings.Join(answerLines(t, md, 80), "\n")
	for _, want := range []string{
		"  • one\n    ◦ one a\n      ▪ one a i\n    ◦ one b\n  • two",
		"  1. first\n    ◦ first bullet\n      1. deep number\n  2. second",
		"  • top\n    ◦ four-space child",
	} {
		if !strings.Contains(got, want) {
			t.Fatalf("missing\n%s\nin\n%s", want, got)
		}
	}
}

// A long nested item wraps with its lines under its text, not its bullet.
func TestAnswerNestedListWraps(t *testing.T) {
	long := strings.Repeat("word ", 20)
	lines := answerLines(t, "- top\n  - "+long+"\n    more of the same item", 40)
	var item []string
	for _, l := range lines {
		if strings.Contains(l, "word") || strings.Contains(l, "more") {
			item = append(item, l)
		}
	}
	if len(item) < 3 || !strings.HasPrefix(item[0], "    ◦ word") {
		t.Fatalf("item:\n%s", strings.Join(lines, "\n"))
	}
	for _, l := range item[1:] {
		if !strings.HasPrefix(l, "      ") || strings.HasPrefix(l, "       ") {
			t.Fatalf("not under the item's text: %q\n%s", l, strings.Join(lines, "\n"))
		}
	}
}

// An open turn shows the whole message you sent, however long: a URL
// pasted three times over, one unbroken word, is broken across rows, and
// the words after it are all there.
func TestOpenTurnShowsTheWholeMessage(t *testing.T) {
	url := "https://github.com/0xdeafcafe/rush/pull/2/files#diff-4f1c0d7e3a9b2c8d6e5f4a3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0"
	tail := "this one keeps opening the wrong file when I click it, and the one after it too, " +
		"so please look at how the links are resolved and fix both of them before the release"
	s := New()
	s.Apply(host.Sent{Text: url + url + url + " " + tail + "\n\nand a second paragraph"}, at(0))
	s.Apply(say("On it."), at(1))
	s.Apply(headless.Result{Subtype: "success"}, at(2))
	const width = 100
	lines := s.Render(Options{Width: width, Now: at(3), Open: map[string]bool{"t1": true}})
	var body strings.Builder
	for _, l := range lines {
		txt := stripANSI(l.Text)
		if w := ansi.StringWidth(txt); w > width {
			t.Fatalf("row %d wide: %q", w, txt)
		}
		if strings.Contains(txt, "On it.") {
			break
		}
		body.WriteString(strings.Trim(strings.TrimSpace(txt), "▏▌│╭╮╰╯─ "))
	}
	got := strings.ReplaceAll(body.String(), " ", "")
	for _, want := range []string{url + url + url, strings.ReplaceAll(tail, " ", ""), "asecondparagraph"} {
		if !strings.Contains(got, want) {
			t.Fatalf("missing %q in\n%s", want, plain(lines))
		}
	}
	if strings.Contains(plain(lines), "…") {
		t.Fatalf("cut short:\n%s", plain(lines))
	}
}

// A subagent's cache lives five minutes, so ten minutes idle is expected;
// so is a compacted prompt starting a new one. A prompt read only at its
// start was changed under it rather than dropped.
func TestColdStartReasons(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "go"}, at(0))
	s.Apply(toolUse("a1", "Task", map[string]any{"subagent_type": "Explore", "description": "look"}), at(1))
	use := func(id string, read, write, sec int) {
		u := &headless.Usage{InputTokens: 10, OutputTokens: 50, CacheReadInputTokens: read, CacheCreationInputTokens: write}
		u.CacheCreation = &struct {
			M5 int `json:"ephemeral_5m_input_tokens"`
			H1 int `json:"ephemeral_1h_input_tokens"`
		}{M5: write}
		s.Apply(headless.Message{Role: "assistant", ID: id, Model: "claude-sonnet-5-5", ParentToolUseID: "a1", Usage: u,
			Blocks: []headless.Block{{Type: "text", Text: "…"}}}, at(sec))
	}
	use("s1", 0, 100000, 2)
	use("s2", 100000, 500, 10)
	use("s3", 0, 100500, 10+600) // ten minutes idle
	use("s4", 0, 40000, 620)     // compacted: 100k to 40k
	use("s5", 5000, 40000, 630)  // only the start read
	var got []string
	for _, c := range s.ColdStarts() {
		got = append(got, fmt.Sprint(c.Expected, " ", c.Reason))
	}
	want := []string{
		"true new subagent: every subagent starts its own cache",
		"true idle past the cache's 5 minutes",
		"true compacted: the summary starts a new cache",
		"false the prompt changed after its first 5k, so the rest was written again",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("got\n%s", strings.Join(got, "\n"))
	}
}

// A turn gone past keeps what you'd look back for: your message, what came
// straight back, what it ended on, what it changed; the steps fold to a
// row, and opened, show again.
func TestSettledTurn(t *testing.T) {
	s := session()
	o := Options{Width: 110, Now: at(40), Open: map[string]bool{}}
	out := plain(s.Render(o))
	for _, w := range []string{
		"▌  the ux right now is totally broken when i attach",
		"Looking at how attach restores the terminal modes.", // straight after you
		"▸ 4 steps  read, search, go build, edit  +2 −1 in 1 file",
		"▏ Fixed the alt screen.",  // what it ended on
		"Building the line editor", // the newest turn stays whole
	} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	if strings.Contains(out, "show 3 steps") || strings.Contains(out, "✓ ✎ internal/daemon/attach.go") {
		t.Errorf("a turn gone past should fold its steps:\n%s", out)
	}
	o.Open["t1:run:1"] = true
	if out := plain(s.Render(o)); !strings.Contains(out, "✎ internal/daemon/attach.go") || !strings.Contains(out, "▾ hide 4 steps") {
		t.Errorf("opened, the run's steps show:\n%s", out)
	}
}
