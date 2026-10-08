package codex

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// rollout builds a synthetic rollout file, a line a second from t0.
type rollout struct {
	t0    time.Time
	lines []string
}

var t0 = time.Date(2026, 9, 20, 10, 0, 0, 0, time.UTC)

func (r *rollout) at(i int) time.Time { return r.t0.Add(time.Duration(i) * time.Second) }

func (r *rollout) add(typ string, payload any) *rollout {
	b, err := jsonx.Marshal(map[string]any{
		"timestamp": r.at(len(r.lines)).Format("2006-01-02T15:04:05.000Z"), "ordinal": len(r.lines), "type": typ, "payload": payload,
	})
	if err != nil {
		panic(err)
	}
	r.lines = append(r.lines, string(b))
	return r
}

func (r *rollout) event(typ string, p map[string]any) *rollout {
	p["type"] = typ
	return r.add("event_msg", p)
}

func (r *rollout) item(it map[string]any) *rollout {
	return r.event("item_completed", map[string]any{"thread_id": "t", "turn_id": "u", "item": it})
}

func (r *rollout) response(typ string, p map[string]any) *rollout {
	p["type"] = typ
	return r.add("response_item", p)
}

func (r *rollout) text() string { return strings.Join(r.lines, "\n") + "\n" }

func (r *rollout) write(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(r.text()), 0o644); err != nil {
		t.Fatal(err)
	}
}

func userMsg(texts ...string) map[string]any {
	var c []map[string]any
	for _, t := range texts {
		c = append(c, map[string]any{"type": "input_text", "text": t})
	}
	return map[string]any{"id": "m", "role": "user", "content": c}
}

func meta(id string, source any) map[string]any {
	return map[string]any{"id": id, "timestamp": t0.Format(time.RFC3339), "cwd": "/work", "cli_version": "0.1.0", "source": source,
		"git": map[string]any{"branch": "main"}}
}

func tokenCount(in, cached, out, reasoning int, used float64) map[string]any {
	u := map[string]any{"input_tokens": in, "cached_input_tokens": cached, "output_tokens": out, "reasoning_output_tokens": reasoning,
		"total_tokens": in + out}
	return map[string]any{
		"info": map[string]any{"total_token_usage": u, "last_token_usage": u, "model_context_window": 1000},
		"rate_limits": map[string]any{"limit_id": "codex", "limit_name": nil, "plan_type": "pro",
			"primary": map[string]any{"used_percent": used, "window_minutes": 300, "resets_at": 1790000000}, "secondary": nil},
	}
}

// describe is an event in a line, for comparing.
func describe(e event.Event) string {
	switch e := e.(type) {
	case event.Init:
		return fmt.Sprintf("init %s %s %s %s %s", e.SessionID, e.Model, e.Cwd, e.Mode, e.Version)
	case event.Message:
		var parts []string
		for _, p := range e.Parts {
			switch p.Kind {
			case event.Text:
				parts = append(parts, "text:"+p.Text)
			case event.Thinking:
				parts = append(parts, "thinking:"+p.Text)
			case event.Image:
				parts = append(parts, "image:"+p.Image.Path+p.Image.MediaType)
			case event.ToolCall:
				c := p.Call
				parts = append(parts, fmt.Sprintf("call:%s %s %s %s%s%s", c.ID, c.Name, c.Kind, c.Input.Command, c.Input.Path, c.Input.Query))
			case event.ToolResult:
				o := p.Output
				s := fmt.Sprintf("result:%s %q err=%v", o.CallID, o.Text, o.IsError)
				if o.Exit != nil {
					s += fmt.Sprintf(" exit=%d", *o.Exit)
				}
				for _, pt := range o.Patches {
					s += fmt.Sprintf(" patch:%s@%d+%d:%s", pt.Path, pt.NewStart, pt.NewLines, strings.Join(pt.Lines, "|"))
				}
				parts = append(parts, s)
			}
		}
		return fmt.Sprintf("%s(%s) %s", e.Role, e.Model, strings.Join(parts, "; "))
	case event.TurnEnd:
		return fmt.Sprintf("end %s %q in=%d out=%d cache=%d reason=%d %s", e.Reason, e.Err, e.Tokens.Input, e.Tokens.Output, e.Tokens.CacheRead,
			e.Tokens.Reasoning, e.Duration)
	case event.Quota:
		w := e.Windows[0]
		return fmt.Sprintf("quota %s %s %.0f%% %s", e.Plan, w.Label, w.Percent, e.FetchedAt.Format(time.TimeOnly))
	case event.Context:
		return fmt.Sprintf("context %d/%d", e.Tokens, e.Window)
	case event.StartNotice:
		return "start " + e.Event + " " + e.Text
	case event.Compacted:
		return fmt.Sprintf("compacted %d", e.Before)
	case event.Limited:
		return "limited"
	case event.Other:
		return "other " + e.Type
	}
	return fmt.Sprintf("%T", e)
}

func describeAll(evs []event.Event) []string {
	out := make([]string, len(evs))
	for i, e := range evs {
		out[i] = describe(e)
	}
	return out
}

func history(t *testing.T, r *rollout, before time.Time) []string {
	t.Helper()
	evs, err := readRollout(strings.NewReader(r.text()), before)
	if err != nil {
		t.Fatal(err)
	}
	return describeAll(evs)
}

func same(t *testing.T, got, want []string) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got:\n  %s\nwant:\n  %s", strings.Join(got, "\n  "), strings.Join(want, "\n  "))
	}
}

// itemTurn is a turn as a current Codex writes it: everything as a
// response_item and again as an item.
func itemTurn(r *rollout) *rollout {
	return r.
		add("session_meta", meta("thread-1", "cli")).
		event("task_started", map[string]any{"turn_id": "u", "model_context_window": 1000}).
		response("message", map[string]any{"id": "d", "role": "developer", "content": []any{map[string]any{"type": "input_text", "text": "<permissions>x</permissions>"}}}).
		response("message", userMsg("<environment_context>\n  <cwd>/work</cwd>\n</environment_context>")).
		add("world_state", map[string]any{"full": true}).
		add("turn_context", map[string]any{"model": "gpt-5", "approval_policy": "never", "effort": "high"}).
		response("message", userMsg("fix the build")).
		item(map[string]any{"type": "UserMessage", "id": "um", "content": []any{
			map[string]any{"type": "text", "text": "fix the build", "text_elements": []any{}},
			map[string]any{"type": "local_image", "path": "/tmp/a.png"},
		}}).
		event("user_message", map[string]any{"message": "fix the build"}).
		response("reasoning", map[string]any{"id": "rs", "summary": []any{}, "encrypted_content": "gAAA"}).
		item(map[string]any{"type": "Reasoning", "id": "rs", "summary_text": []any{}, "raw_content": []any{}}).
		item(map[string]any{"type": "Reasoning", "id": "rs2", "summary_text": []any{"Looking"}, "raw_content": []any{}}).
		response("message", map[string]any{"id": "am", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "On it."}}}).
		item(map[string]any{"type": "AgentMessage", "id": "am", "content": []any{map[string]any{"type": "Text", "text": "On it."}}, "phase": "commentary"}).
		event("agent_message", map[string]any{"message": "On it."}).
		response("custom_tool_call", map[string]any{"id": "ct", "call_id": "call_1", "name": "exec", "input": "await tools.exec_command({cmd: 'make'})"}).
		item(map[string]any{"type": "CommandExecution", "id": "exec-1", "command": []any{"/bin/zsh", "-lc", "make"}, "cwd": "/work",
			"parsed_cmd": []any{map[string]any{"type": "unknown", "cmd": "make"}}, "status": "failed", "aggregated_output": "boom\n", "exit_code": 2}).
		item(map[string]any{"type": "CommandExecution", "id": "exec-2", "command": []any{"/bin/zsh", "-lc", "cat go.mod"}, "cwd": "/work",
			"parsed_cmd": []any{map[string]any{"type": "read", "cmd": "cat go.mod", "name": "go.mod", "path": "go.mod"}}, "status": "completed",
			"aggregated_output": "module x\n", "exit_code": 0}).
		response("custom_tool_call_output", map[string]any{"id": "co", "call_id": "call_1", "output": []any{map[string]any{"type": "input_text", "text": "boom"}}}).
		add("token_usage_record", map[string]any{"usage": map[string]any{"input_tokens": 100}}).
		event("token_count", tokenCount(100, 40, 10, 5, 12)).
		item(map[string]any{"type": "FileChange", "id": "fc", "status": "completed", "changes": map[string]any{
			"/work/b.go": map[string]any{"type": "update", "unified_diff": "@@ -1,2 +1,2 @@\n-old\n+new\n ctx\n", "move_path": nil},
			"/work/a.go": map[string]any{"type": "add", "content": "package a\n"},
		}}).
		item(map[string]any{"type": "Extension", "id": "ws", "kind": "web.search", "query": "go generics",
			"action": map[string]any{"type": "search", "query": "go generics"}, "results": []any{}}).
		item(map[string]any{"type": "SubAgentActivity", "id": "sa", "kind": "started"}).
		event("token_count", tokenCount(200, 150, 20, 0, 12)).
		event("task_complete", map[string]any{"turn_id": "u", "duration_ms": 1500, "error": nil})
}

func TestHistoryItems(t *testing.T) {
	got := history(t, itemTurn(&rollout{t0: t0}), time.Time{})
	same(t, got, []string{
		"init thread-1 gpt-5 /work never 0.1.0",
		"start SessionStart <permissions>x</permissions>",
		"start SessionStart <environment_context>\n  <cwd>/work</cwd>\n</environment_context>",
		"user() text:fix the build; image:/tmp/a.png",
		"assistant(gpt-5) thinking:Looking",
		"assistant(gpt-5) text:On it.",
		"assistant(gpt-5) call:exec-1 shell shell make",
		`user() result:exec-1 "boom\n" err=true exit=2`,
		"assistant(gpt-5) call:exec-2 shell read cat go.modgo.mod",
		`user() result:exec-2 "module x\n" err=false exit=0`,
		"quota pro 5h 12% 10:00:20",
		"assistant(gpt-5) call:fc apply_patch edit /work/a.go",
		`user() result:fc "" err=false patch:/work/a.go@1+1:+package a patch:/work/b.go@1+2:-old|+new| ctx`,
		"assistant(gpt-5) call:ws web_search websearch go generics",
		`user() result:ws "" err=false`,
		"other item/SubAgentActivity",
		"context 220/1000",
		"end done \"\" in=110 out=30 cache=190 reason=5 1.5s",
	})
}

// A turn without items, as older and guardian rollouts are, is read from
// its response_items.
func TestHistoryResponses(t *testing.T) {
	r := (&rollout{t0: t0}).
		add("session_meta", meta("thread-2", "exec")).
		add("session_meta", meta("parent", "exec")).
		add("turn_context", map[string]any{"model": "gpt-5-mini", "approval_policy": "on-request"}).
		event("task_started", map[string]any{}).
		response("message", userMsg("# AGENTS.md instructions for /work\n\n<INSTRUCTIONS>be nice</INSTRUCTIONS>", "<environment_context>x</environment_context>")).
		response("message", map[string]any{"id": "u1", "role": "user", "content": []any{
			map[string]any{"type": "input_text", "text": "<image name=[Image #1]>"},
			map[string]any{"type": "input_image", "image_url": "data:image/png;base64,iVBORw=="},
			map[string]any{"type": "input_text", "text": "</image>"},
			map[string]any{"type": "input_text", "text": "what is <b>this</b>?"},
		}}).
		event("user_message", map[string]any{"message": "what is this?"}).
		response("reasoning", map[string]any{"id": "r", "summary": []any{map[string]any{"type": "summary_text", "text": "Hmm"}}, "encrypted_content": "x"}).
		response("function_call", map[string]any{"id": "f", "call_id": "c1", "name": "exec_command", "arguments": `{"cmd":"ls","workdir":"/work"}`}).
		response("function_call_output", map[string]any{"id": "fo", "call_id": "c1", "output": "a.go"}).
		response("custom_tool_call", map[string]any{"id": "p", "call_id": "c2", "name": "apply_patch", "input": "*** Begin Patch\n*** Add File: x.go\n+package x\n*** End Patch"}).
		response("custom_tool_call_output", map[string]any{"id": "po", "call_id": "c2", "output": []any{map[string]any{"type": "input_text", "text": "Done"}}}).
		response("message", map[string]any{"id": "a", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": "An image."}}}).
		event("agent_message", map[string]any{"message": "An image."}).
		event("turn_aborted", map[string]any{"reason": "interrupted", "duration_ms": 2000})
	same(t, history(t, r, time.Time{}), []string{
		"init thread-2 gpt-5-mini /work on-request 0.1.0",
		"start SessionStart # AGENTS.md instructions for /work\n\n<INSTRUCTIONS>be nice</INSTRUCTIONS>",
		"start SessionStart <environment_context>x</environment_context>",
		"user() image:image/png; text:what is <b>this</b>?",
		"assistant(gpt-5-mini) thinking:Hmm",
		"assistant(gpt-5-mini) call:c1 exec_command shell ls",
		`user() result:c1 "a.go" err=false`,
		"assistant(gpt-5-mini) call:c2 apply_patch write x.go",
		`user() result:c2 "Done" err=false`,
		"assistant(gpt-5-mini) text:An image.",
		`end interrupted "" in=0 out=0 cache=0 reason=0 2s`,
	})
}

func TestHistoryEnds(t *testing.T) {
	r := (&rollout{t0: t0}).
		add("session_meta", meta("thread-3", "cli")).
		add("turn_context", map[string]any{"model": "gpt-5"}).
		event("task_started", map[string]any{}).
		item(map[string]any{"type": "UserMessage", "id": "u1", "content": []any{map[string]any{"type": "text", "text": "one"}}}).
		event("token_count", tokenCount(10, 0, 1, 0, 50)).
		event("token_count", tokenCount(10, 0, 1, 0, 50)). // the same limits again
		event("task_complete", map[string]any{"duration_ms": 10, "error": map[string]any{"message": "limit hit", "codex_error_info": "usage_limit_exceeded"}}).
		add("compacted", map[string]any{"message": "", "replacement_history": []any{userMsg("old")},
			"latest_token_usage_record": map[string]any{"usage": map[string]any{"total_tokens": 900}}}).
		item(map[string]any{"type": "ContextCompaction", "id": "cc"}).
		add("turn_context", map[string]any{"model": "gpt-5.1"}).
		event("task_started", map[string]any{}).
		item(map[string]any{"type": "UserMessage", "id": "u2", "content": []any{map[string]any{"type": "text", "text": "two"}}}).
		item(map[string]any{"type": "AgentMessage", "id": "a2", "content": []any{map[string]any{"type": "Text", "text": "ok"}}})
	all := []string{
		"init thread-3 gpt-5 /work  0.1.0",
		"user() text:one",
		"quota pro 5h 50% 10:00:04",
		"context 11/1000",
		"limited",
		`end error "limit hit" in=20 out=2 cache=0 reason=0 10ms`,
		"compacted 900",
		"user() text:two",
		"assistant(gpt-5.1) text:ok",
	}
	for _, tc := range []struct {
		name   string
		before time.Time
		want   []string
	}{
		{"whole", time.Time{}, all},
		{"before the second turn", r.at(9), all[:7]},
		{"before the first reply", r.at(4), all[:2]},
		{"before anything", t0, nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := history(t, r, tc.before)
			if len(got) == 0 && len(tc.want) == 0 {
				return
			}
			same(t, got, tc.want)
		})
	}
}

// A line of several megabytes, as a command's output can be, is read
// whole.
func TestHistoryLongLine(t *testing.T) {
	big := strings.Repeat("0123456789abcdef", 3<<20/16*2) // 6 MB
	r := (&rollout{t0: t0}).
		add("session_meta", meta("thread-4", "cli")).
		item(map[string]any{"type": "CommandExecution", "id": "e", "command": []any{"sh", "-c", "yes"}, "status": "completed",
			"aggregated_output": big, "exit_code": 0}).
		item(map[string]any{"type": "AgentMessage", "id": "a", "content": []any{map[string]any{"type": "Text", "text": "after"}}})
	path := filepath.Join(t.TempDir(), "rollout.jsonl")
	r.write(t, path)
	evs, err := Adapter{}.History(agent.Session{Transcript: path}, time.Time{})
	if err != nil {
		t.Fatal(err)
	}
	if len(evs) != 4 {
		t.Fatalf("got %d events: %v", len(evs), describeAll(evs))
	}
	if o := evs[2].(event.Message).Parts[0].Output; len(o.Text) != len(big) {
		t.Errorf("output is %d bytes, want %d", len(o.Text), len(big))
	}
	if got := describe(evs[3]); got != "assistant() text:after" {
		t.Errorf("after the long line: %s", got)
	}
}

func TestInjected(t *testing.T) {
	for _, tc := range []struct {
		text string
		want bool
	}{
		{"<environment_context>\n  <cwd>/x</cwd>\n</environment_context>", true},
		{"  <user_instructions>be brief</user_instructions>\n", true},
		{"<turn_aborted>\nThe user interrupted.\n</turn_aborted>", true},
		{"# AGENTS.md instructions for /x\n\n<INSTRUCTIONS>...</INSTRUCTIONS>", true},
		{"<image name=[Image #1]>", true},
		{"</image>", true},
		{"fix the build", false},
		{"<div> is broken", false},
		{"<b>bold</b> is wrong", false},
		{"what does <br> do", false},
		{"# Plan\n\ndo it", false},
	} {
		if got := injected(tc.text); got != tc.want {
			t.Errorf("injected(%q) = %v, want %v", tc.text, got, tc.want)
		}
	}
}

func TestArgv(t *testing.T) {
	for _, tc := range []struct{ raw, want string }{
		{`["/bin/zsh","-lc","go test ./..."]`, "go test ./..."},
		{`["bash","-c","ls"]`, "ls"},
		{`["git","status"]`, "git status"},
		{`"make"`, "make"},
	} {
		if got := argv(jsontext.Value(tc.raw)); got != tc.want {
			t.Errorf("argv(%s) = %q, want %q", tc.raw, got, tc.want)
		}
	}
}

func TestReadCallFromItem(t *testing.T) {
	it := rolloutItem{Type: "CommandExecution", ID: "e", Command: jsontext.Value(`["sh","-lc","rg foo src"]`),
		ParsedCmd: []parsedCommand{{Type: "search", Query: "foo", Path: "src"}}}
	ti, _ := it.threadItem()
	c, _ := callOf(ti, nil)
	if c.Kind != tool.Search || c.Input.Pattern != "foo" || c.Input.Path != "src" {
		t.Errorf("got %+v", c)
	}
}

func TestPast(t *testing.T) {
	dir := t.TempDir()
	day := filepath.Join(dir, "sessions", "2026", "09", "20")
	uuid := func(n int) string { return fmt.Sprintf("01a0c9a2-0000-7000-8000-%012d", n) }
	path := func(n int) string {
		return filepath.Join(day, "rollout-2026-09-20T10-00-0"+fmt.Sprint(n)+"-"+uuid(n)+".jsonl")
	}

	long := "please   look at\n\nthe " + strings.Repeat("very ", 30) + "long thing"
	(&rollout{t0: t0}).
		add("session_meta", meta(uuid(1), "cli")).
		add("turn_context", map[string]any{"model": "gpt-5"}).
		response("message", userMsg("<environment_context>x</environment_context>")).
		response("message", userMsg(long)).
		add("response_item", map[string]any{"type": "message", "role": "assistant", "content": []any{}}).
		write(t, path(1))
	(&rollout{t0: t0.Add(time.Hour)}).
		add("session_meta", meta(uuid(2), "exec")).
		add("turn_context", map[string]any{"model": "gpt-5-mini"}).
		response("message", userMsg("short one")).
		write(t, path(2))
	(&rollout{t0: t0}). // a spawned agent's thread
				add("session_meta", meta(uuid(3), map[string]any{"subagent": map[string]any{"thread_spawn": map[string]any{}}})).
				write(t, path(3))
	(&rollout{t0: t0}). // no session_meta: the id is the file name's
				add("turn_context", map[string]any{"model": "gpt-5"}).
				response("message", userMsg("renamed")).
				write(t, path(4))
	if err := os.WriteFile(filepath.Join(day, "notes.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	index := fmt.Sprintf(`{"id":%q,"thread_name":"Old name","updated_at":"2026-09-20T10:00:00Z"}
{"id":%q,"thread_name":"Fix the\nlogin","updated_at":"2026-09-20T11:00:00Z"}
`, uuid(4), uuid(4))
	if err := os.WriteFile(filepath.Join(dir, "session_index.jsonl"), []byte(index), 0o644); err != nil {
		t.Fatal(err)
	}
	for i, age := range map[int]time.Duration{1: 3 * time.Hour, 2: 2 * time.Hour, 4: time.Hour} {
		mt := time.Now().Add(-age)
		if err := os.Chtimes(path(i), mt, mt); err != nil {
			t.Fatal(err)
		}
	}

	p := agent.Profile{Kind: Kind, Name: ".codex", Dir: dir}
	got := Adapter{}.Past(p)
	type row struct{ ID, Name, Model, Cwd, State, Transcript string }
	var rows []row
	for _, s := range got {
		if s.Kind != Kind || s.Profile != p || s.CreatedAt.IsZero() || s.UpdatedAt.IsZero() {
			t.Errorf("%s: %+v", s.ID, s)
		}
		rows = append(rows, row{s.ID, s.Name, s.Model, s.Cwd, s.State, s.Transcript})
	}
	wantName := "please look at the " + strings.TrimSpace(strings.Repeat("very ", 12)) + "…" // 79 runes, cut at a space
	want := []row{
		{uuid(4), "Fix the login", "gpt-5", "", "done", path(4)},
		{uuid(2), "short one", "gpt-5-mini", "/work", "done", path(2)},
		{uuid(1), wantName, "gpt-5", "/work", "done", path(1)},
	}
	if !reflect.DeepEqual(rows, want) {
		t.Errorf("got  %+v\nwant %+v", rows, want)
	}
	if !got[1].Headless || got[2].Headless { // codex exec's, and a terminal's
		t.Errorf("headless %v %v", got[1].Headless, got[2].Headless)
	}
	if got[1].CreatedAt != t0 {
		t.Errorf("created %v, want the session_meta's %v", got[1].CreatedAt, t0)
	}
	if live := (Adapter{}).Live(p); live != nil {
		t.Errorf("live: %v", live)
	}
}

func TestHistoryFindsTheRolloutByThread(t *testing.T) {
	dir := t.TempDir()
	day := filepath.Join(dir, "sessions", "2026", "09", "27")
	if err := os.MkdirAll(day, 0o700); err != nil {
		t.Fatal(err)
	}
	id := "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000"
	line := `{"timestamp":"2026-09-27T10:00:00.000Z","type":"session_meta","payload":{"id":"` + id + `","cwd":"/w","cli_version":"0.155.1"}}`
	if err := os.WriteFile(filepath.Join(day, "rollout-2026-09-27T10-00-00-"+id+".jsonl"), []byte(line+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	evs, err := Adapter{}.History(agent.Session{ID: id, Profile: agent.Profile{Dir: dir}}, time.Time{})
	if err != nil || len(evs) == 0 {
		t.Fatalf("History by thread = %v, %v", evs, err)
	}
	if _, err := (Adapter{}).History(agent.Session{ID: "nope", Profile: agent.Profile{Dir: dir}}, time.Time{}); err == nil {
		t.Error("a thread with no rollout read as one")
	}
}

func TestLiveIsWhatWasWrittenLately(t *testing.T) {
	dir := t.TempDir()
	now := time.Now()
	day := filepath.Join(dir, "sessions", now.Format("2006"), now.Format("01"), now.Format("02"))
	if err := os.MkdirAll(day, 0o700); err != nil {
		t.Fatal(err)
	}
	write := func(id string, age time.Duration) {
		line := `{"timestamp":"2026-09-27T10:00:00.000Z","type":"session_meta","payload":{"id":"` + id + `","cwd":"/w"}}`
		path := filepath.Join(day, "rollout-2026-09-27T10-00-00-"+id+".jsonl")
		if err := os.WriteFile(path, []byte(line+"\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		_ = os.Chtimes(path, now.Add(-age), now.Add(-age))
	}
	write("0199aaaa-0000-7000-8000-000000000001", 5*time.Second)
	write("0199aaaa-0000-7000-8000-000000000002", time.Minute)
	write("0199aaaa-0000-7000-8000-000000000003", time.Hour)
	live := Adapter{}.Live(agent.Profile{Dir: dir})
	states := map[string]string{}
	for _, s := range live {
		states[s.ID[len(s.ID)-1:]] = s.State
	}
	if len(live) != 2 || states["1"] != "working" || states["2"] != "idle" {
		t.Errorf("live = %v", states)
	}
}

// oneLine collapses only the start of a prompt, and names it just as
// collapsing the whole of it would.
func TestOneLineMatchesWhole(t *testing.T) {
	whole := func(text string) string {
		s := strings.Join(strings.Fields(text), " ")
		if r := []rune(s); len(r) > nameLen {
			return strings.TrimSpace(string(r[:nameLen-1])) + "…"
		}
		return s
	}
	words := []string{"fix", "the", " ", "\n\n", "\t", "é", "日本語", "x", "  very  ", "long-word-" + strings.Repeat("y", 70)}
	for i := range 2000 {
		var b strings.Builder
		for j := range i % 60 {
			b.WriteString(words[(i*7+j*13)%len(words)])
		}
		if got, want := oneLine(b.String()), whole(b.String()); got != want {
			t.Fatalf("oneLine(%q) = %q, want %q", b.String(), got, want)
		}
	}
}

// A rollout read from part way through a turn keeps the thread's meta and
// reads its last turn as the whole rollout does.
func TestHistoryTail(t *testing.T) {
	turn := func(r *rollout, said, answer string) *rollout {
		return r.add("turn_context", map[string]any{"model": "gpt-5", "approval_policy": "never"}).
			event("task_started", map[string]any{}).
			response("message", userMsg(said)).
			response("function_call", map[string]any{"id": "f", "call_id": "c-" + said, "name": "exec_command", "arguments": `{"cmd":"ls"}`}).
			response("function_call_output", map[string]any{"id": "fo", "call_id": "c-" + said, "output": "a.go"}).
			response("message", map[string]any{"id": "a", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": answer}}}).
			event("task_complete", map[string]any{})
	}
	r := turn(turn((&rollout{t0: t0}).add("session_meta", meta("thread-9", "cli")), "first", "one"), "second", "two")
	path := filepath.Join(t.TempDir(), "rollout.jsonl")
	r.write(t, path)
	whole := history(t, r, time.Time{})
	all := r.text()
	evs, cut, err := Adapter{}.HistoryTail(agent.Session{Transcript: path}, int64(len(all)-strings.Index(all, `"c-first"`)))
	if err != nil || !cut {
		t.Fatalf("cut %v: %v", cut, err)
	}
	got := describeAll(evs)
	if !strings.HasPrefix(got[0], "init thread-9") || strings.Contains(strings.Join(got, "\n"), "text:first") {
		t.Errorf("start: %v", got)
	}
	same(t, got[len(got)-5:], whole[len(whole)-5:])
}

// A tail that starts after the rollout's only turn_context still says the
// model, as the whole rollout does.
func TestHistoryTailModel(t *testing.T) {
	turn := func(r *rollout, said, answer string) *rollout {
		return r.event("task_started", map[string]any{}).
			response("message", userMsg(said)).
			response("message", map[string]any{"id": "a", "role": "assistant", "content": []any{map[string]any{"type": "output_text", "text": answer}}}).
			event("task_complete", map[string]any{})
	}
	r := (&rollout{t0: t0}).add("session_meta", meta("thread-9", "cli")).add("turn_context", map[string]any{"model": "gpt-5", "approval_policy": "never"})
	r = turn(turn(r, "first", "one"), "second", "two")
	path := filepath.Join(t.TempDir(), "rollout.jsonl")
	r.write(t, path)
	all := r.text()
	evs, cut, err := Adapter{}.HistoryTail(agent.Session{Transcript: path}, int64(len(all)-strings.Index(all, `"second"`)))
	if err != nil || !cut || !strings.HasPrefix(describe(evs[0]), "init thread-9 gpt-5") {
		t.Fatalf("cut %v, %v: %v", cut, err, describeAll(evs))
	}
}

// Following a rollout as it's written reads only what's new, and gives
// what a whole read would each time; once stopped, it gives up.
func TestFollowHistory(t *testing.T) {
	r := itemTurn((&rollout{t0: t0}).add("session_meta", meta("thread-1", "cli")))
	whole := r.text()
	path := filepath.Join(t.TempDir(), "rollout.jsonl")
	half := len(whole) - len(r.lines[len(r.lines)-1])/2 // the last line still being written
	if err := os.WriteFile(path, []byte(whole[:half]), 0o644); err != nil {
		t.Fatal(err)
	}
	var stop atomic.Bool
	follow := Adapter{}.FollowHistory(agent.Session{Transcript: path}, &stop)
	evs, err := follow()
	if err != nil {
		t.Fatal(err)
	}
	cut := strings.LastIndex(whole[:half], "\n") + 1
	part, _ := readRollout(strings.NewReader(whole[:cut]), time.Time{})
	same(t, describeAll(evs), describeAll(part))
	if err := os.WriteFile(path, []byte(whole), 0o644); err != nil {
		t.Fatal(err)
	}
	evs, err = follow()
	if err != nil {
		t.Fatal(err)
	}
	same(t, describeAll(evs), history(t, r, time.Time{}))
	stop.Store(true)
	if err := os.WriteFile(path, []byte(whole+whole), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := follow(); err == nil {
		t.Error("a stopped follow read on")
	}
}
