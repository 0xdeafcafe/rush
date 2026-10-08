package headless

import (
	"encoding/base64"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Lines as Claude Code 2.1.280 writes them, trimmed of fields we ignore.
const (
	lineInit       = `{"type":"system","subtype":"init","cwd":"/work","session_id":"s-1","tools":["Bash","Read"],"model":"claude-haiku-4-5-20251001","permissionMode":"default","slash_commands":["compact"],"claude_code_version":"2.1.280"}`
	lineStart      = `{"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[]}},"session_id":"s-1"}`
	lineDelta      = `{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}},"session_id":"s-1"}`
	lineThink      = `{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}},"session_id":"s-1"}`
	lineToolUse    = `{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi","description":"Say hi"}}],"usage":{"input_tokens":10,"output_tokens":4,"cache_read_input_tokens":13689,"cache_creation_input_tokens":7787}},"parent_tool_use_id":null,"session_id":"s-1","uuid":"u-1"}`
	lineAsk        = `{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"echo hi","description":"Say hi"},"description":"Say hi","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"echo hi"}],"behavior":"allow","destination":"localSettings"}],"decision_reason":"This command requires approval","decision_reason_type":"other","tool_use_id":"toolu_1"}}`
	lineToolResult = `{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":[{"type":"text","text":"hi"}],"is_error":false}]},"parent_tool_use_id":null,"session_id":"s-1","uuid":"u-2"}`
	lineDenied     = `{"type":"system","subtype":"permission_denied","tool_name":"Bash","tool_use_id":"toolu_2","decision_reason_type":"safetyCheck","decision_reason":"sensitive file","session_id":"s-1"}`
	lineResult     = `{"type":"result","subtype":"success","is_error":false,"duration_ms":6900,"num_turns":2,"result":"Done.","stop_reason":"end_turn","session_id":"s-1","total_cost_usd":0.0247,"usage":{"input_tokens":18,"output_tokens":289}}`
	lineCancel     = `{"type":"control_cancel_request","request_id":"req-1"}`
	lineHook       = `{"type":"system","subtype":"hook_started","hook_name":"SessionStart:startup","session_id":"s-1"}`
)

func decode(t *testing.T, line string) Event {
	t.Helper()
	ev, err := Decode([]byte(line))
	if err != nil {
		t.Fatalf("decode %s: %v", line, err)
	}
	return ev
}

func TestDecode(t *testing.T) {
	if ev := decode(t, lineInit).(Init); ev.SessionID != "s-1" || ev.Model != "claude-haiku-4-5-20251001" || ev.Version != "2.1.280" || len(ev.Tools) != 2 {
		t.Errorf("init: %+v", ev)
	}
	if ev := decode(t, lineStart).(MessageStart); ev.MessageID != "msg_1" {
		t.Errorf("start: %+v", ev)
	}
	if ev := decode(t, lineDelta).(Delta); ev.Text != "Hel" || ev.Index != 1 || ev.Thinking {
		t.Errorf("delta: %+v", ev)
	}
	if ev := decode(t, lineThink).(Delta); ev.Text != "hmm" || !ev.Thinking {
		t.Errorf("thinking: %+v", ev)
	}
	msg := decode(t, lineToolUse).(Message)
	if msg.Role != "assistant" || len(msg.Blocks) != 1 || msg.Blocks[0].Name != "Bash" || msg.Usage.CacheReadInputTokens != 13689 {
		t.Errorf("tool use: %+v", msg)
	}
	ask := decode(t, lineAsk).(PermissionRequest)
	if ask.ID != "req-1" || ask.Tool != "Bash" || ask.ToolUseID != "toolu_1" || ask.Reason != "This command requires approval" || len(ask.Suggestions) == 0 {
		t.Errorf("ask: %+v", ask)
	}
	res := decode(t, lineToolResult).(Message)
	if b := res.Blocks[0]; b.Type != "tool_result" || b.Text != "hi" || b.ToolUseID != "toolu_1" {
		t.Errorf("tool result: %+v", b)
	}
	if ev := decode(t, lineDenied).(PermissionDenied); ev.Tool != "Bash" || ev.Reason != "sensitive file" {
		t.Errorf("denied: %+v", ev)
	}
	if ev := decode(t, lineResult).(Result); ev.Text != "Done." || ev.CostUSD != 0.0247 || ev.NumTurns != 2 {
		t.Errorf("result: %+v", ev)
	}
	if ev := decode(t, lineCancel).(PermissionCancelled); ev.ID != "req-1" {
		t.Errorf("cancel: %+v", ev)
	}
	if ev := decode(t, lineHook).(Other); ev.Subtype != "hook_started" {
		t.Errorf("hook: %+v", ev)
	}
}

// fakeClaude writes a script that plays one turn: it asks permission for a
// tool, then echoes back the answer it got, and logs every stdin line.
func fakeClaude(t *testing.T) (bin, log string) {
	dir := t.TempDir()
	log = filepath.Join(dir, "stdin.log")
	script := `#!/bin/sh
printf '%s\n' "$*" > "` + log + `.args"
read -r prompt; printf '%s\n' "$prompt" >> "` + log + `"
echo '` + lineInit + `'
echo '` + lineToolUse + `'
echo '` + lineAsk + `'
read -r answer; printf '%s\n' "$answer" >> "` + log + `"
echo '` + lineToolResult + `'
echo '` + lineResult + `'
while read -r more; do printf '%s\n' "$more" >> "` + log + `"; done
`
	bin = filepath.Join(dir, "claude")
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return bin, log
}

func TestSessionRoundTrip(t *testing.T) {
	bin, log := fakeClaude(t)
	s, err := Start(Options{Dir: t.TempDir(), Resume: "s-1", PermissionMode: "acceptEdits", Binary: bin})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Send("say hi"); err != nil {
		t.Fatal(err)
	}
	var got []Event
	timeout := time.After(5 * time.Second)
loop:
	for {
		select {
		case ev := <-s.Events:
			got = append(got, ev)
			switch ev := ev.(type) {
			case PermissionRequest:
				if err := s.Allow(ev, nil, true); err != nil {
					t.Fatal(err)
				}
			case Result:
				break loop
			}
		case <-timeout:
			t.Fatalf("timed out after %d events", len(got))
		}
	}
	if len(got) != 5 {
		t.Errorf("got %d events, want 5: %#v", len(got), got)
	}
	if err := s.Interrupt(); err != nil {
		t.Fatal(err)
	}
	if err := s.Stop(2 * time.Second); err != nil {
		t.Fatalf("stop: %v", err)
	}

	args, _ := os.ReadFile(log + ".args")
	for _, want := range []string{"--permission-prompt-tool stdio", "--resume s-1", "--permission-mode acceptEdits", "--input-format stream-json"} {
		if !strings.Contains(string(args), want) {
			t.Errorf("args %q missing %q", args, want)
		}
	}
	b, _ := os.ReadFile(log)
	lines := strings.Split(strings.TrimSpace(string(b)), "\n")
	if len(lines) != 3 {
		t.Fatalf("stdin lines: %q", lines)
	}
	var prompt struct {
		Type    string                   `json:"type"`
		Message struct{ Content string } `json:"message"`
	}
	_ = jsonx.Unmarshal([]byte(lines[0]), &prompt)
	if prompt.Type != "user" || prompt.Message.Content != "say hi" {
		t.Errorf("prompt: %s", lines[0])
	}
	var answer struct {
		Type     string `json:"type"`
		Response struct {
			Subtype   string `json:"subtype"`
			RequestID string `json:"request_id"`
			Response  struct {
				Behavior           string         `json:"behavior"`
				UpdatedInput       jsontext.Value `json:"updatedInput"`
				UpdatedPermissions jsontext.Value `json:"updatedPermissions"`
			} `json:"response"`
		} `json:"response"`
	}
	_ = jsonx.Unmarshal([]byte(lines[1]), &answer)
	r := answer.Response
	if answer.Type != "control_response" || r.Subtype != "success" || r.RequestID != "req-1" || r.Response.Behavior != "allow" ||
		!strings.Contains(string(r.Response.UpdatedInput), "echo hi") || len(r.Response.UpdatedPermissions) == 0 {
		t.Errorf("answer: %s", lines[1])
	}
	if !strings.Contains(lines[2], `"subtype":"interrupt"`) {
		t.Errorf("interrupt: %s", lines[2])
	}
}

// TestRealClaude runs one approved tool call through the installed claude.
// It spends a few cents of Haiku, so it only runs with RUSH_REAL_CLAUDE=1.
func TestRealClaude(t *testing.T) {
	if os.Getenv("RUSH_REAL_CLAUDE") == "" {
		t.Skip("set RUSH_REAL_CLAUDE=1 to run against the installed claude")
	}
	// Outside ~/.claude, which Claude Code treats as sensitive and never asks about.
	dir, err := os.MkdirTemp("/tmp", "rush-headless-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(dir)
	s, err := Start(Options{Dir: dir, Model: "haiku", PermissionMode: "default"})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Stop(5 * time.Second)
	if err := s.Send("Run the shell command `curl -s -o /dev/null -w %{http_code} https://example.com` and reply with only the code."); err != nil {
		t.Fatal(err)
	}
	asked, deltas := false, 0
	timeout := time.After(90 * time.Second)
	for {
		select {
		case ev, ok := <-s.Events:
			if !ok {
				t.Fatalf("session ended: %v", s.Err())
			}
			switch ev := ev.(type) {
			case Delta:
				deltas++
			case PermissionRequest:
				asked = true
				if err := s.Allow(ev, nil, false); err != nil {
					t.Fatal(err)
				}
			case Result:
				if !asked || deltas == 0 || !strings.Contains(ev.Text, "200") {
					t.Fatalf("asked=%v deltas=%d result=%+v", asked, deltas, ev)
				}
				return
			}
		case <-timeout:
			t.Fatal("timed out")
		}
	}
}

func TestDecodeMCPMessage(t *testing.T) {
	ev, err := Decode([]byte(`{"type":"control_request","request_id":"r9","request":{"subtype":"mcp_message","server_name":"rush","message":{"jsonrpc":"2.0","id":1,"method":"tools/list"}}}`))
	if err != nil {
		t.Fatal(err)
	}
	m, ok := ev.(MCPRequest)
	if !ok || m.ID != "r9" || m.Server != "rush" || !strings.Contains(string(m.Message), "tools/list") {
		t.Fatalf("got %#v", ev)
	}
}

// A message's images go where its text names them, each right after its
// marker; the ones it doesn't name come at the end, and a text naming none
// keeps the images first.
func TestContentPlacesImagesAtTheirMarkers(t *testing.T) {
	a, b, c := Image{MediaType: "image/png", Data: []byte("a")}, Image{MediaType: "image/png", Data: []byte("b")}, Image{MediaType: "image/png", Data: []byte("c")}
	kinds := func(blocks []map[string]any) string {
		var out []string
		for _, bl := range blocks {
			if bl["type"] == "text" {
				out = append(out, "text:"+bl["text"].(string))
				continue
			}
			data := bl["source"].(map[string]any)["data"].(string)
			raw, _ := base64.StdEncoding.DecodeString(data)
			out = append(out, "image:"+string(raw))
		}
		return strings.Join(out, " | ")
	}
	got := kinds(Content("see [Image #2] then [Image #1] ok", []Image{a, b, c}))
	want := "text:see [Image #2] | image:b | text: then [Image #1] | image:a | text: ok | image:c"
	if got != want {
		t.Fatalf("interleaved:\n got %s\nwant %s", got, want)
	}
	if got := kinds(Content("no markers here", []Image{a, b})); got != "image:a | image:b | text:no markers here" {
		t.Fatalf("no markers: %s", got)
	}
	// A marker for an image that isn't there stays text.
	if got := kinds(Content("[Image #5] and [Image #1]", []Image{a})); got != "text:[Image #5] and [Image #1] | image:a" {
		t.Fatalf("unknown marker: %s", got)
	}
}

func TestRunsSession(t *testing.T) {
	sid := "8c76706f-1c00-4aed-9c6d-7509f3033943"
	for _, c := range []struct {
		args []string
		want bool
	}{
		{[]string{"claude", "-p", "--input-format", "stream-json", "--resume", sid}, true},
		{[]string{"claude", "-p", "--input-format", "stream-json", "--session-id", sid}, true},
		{[]string{"claude", "--resume", sid}, false},
		{[]string{"claude", "-p", "--input-format", "stream-json", "--resume", "other"}, false},
		{[]string{"vim", sid}, false},
	} {
		if got := RunsSession(c.args, sid); got != c.want {
			t.Errorf("%v: %v", c.args, got)
		}
	}
}
