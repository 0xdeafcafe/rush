package headless

import (
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

func neutral(t *testing.T, n *Neutral, line string) []event.Event {
	t.Helper()
	return n.Event(decode(t, line))
}

func TestNeutralFollowsATurn(t *testing.T) {
	var n Neutral
	init := neutral(t, &n, lineInit)[0].(event.Init)
	if init.SessionID != "s-1" || init.Mode != "default" || init.Version != "2.1.280" || len(init.Commands) != 1 {
		t.Errorf("Init = %+v", init)
	}
	if d := neutral(t, &n, lineThink)[0].(event.Delta); d.Kind != event.Thinking || d.Text != "hmm" {
		t.Errorf("thinking Delta = %+v", d)
	}
	m := neutral(t, &n, lineToolUse)[0].(event.Message)
	call := m.Parts[0].Call
	if call == nil || call.Kind != tool.Shell || call.Input.Command != "echo hi" || m.Tokens.CacheRead != 13689 {
		t.Fatalf("tool use = %+v", m)
	}
	a := neutral(t, &n, lineAsk)[0].(event.Approval)
	if a.ID != "req-1" || a.Call.Kind != tool.Shell || len(a.Options) != 3 || a.Options[1].Kind != event.AllowAlways {
		t.Errorf("Approval = %+v", a)
	}
	r := neutral(t, &n, lineToolResult)[0].(event.Message)
	out := r.Parts[0].Output
	if out == nil || out.CallID != "toolu_1" || out.Exit == nil || *out.Exit != 0 {
		t.Errorf("tool result = %+v", r.Parts[0])
	}
	if c := neutral(t, &n, lineCancel)[0].(event.ApprovalCancelled); c.ID != "req-1" {
		t.Errorf("cancel = %+v", c)
	}
	end := neutral(t, &n, lineResult)[0].(event.TurnEnd)
	if end.Reason != "done" || end.Cost != 0.0247 || end.Duration != 6900*time.Millisecond || end.Tokens.Output != 289 {
		t.Errorf("TurnEnd = %+v", end)
	}
	if o := neutral(t, &n, lineHook)[0].(event.Other); o.Adapter != "claude" {
		t.Errorf("hook = %+v", o)
	}
}

func TestNeutralQuestionsAndLimits(t *testing.T) {
	var n Neutral
	ask := `{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","header":"Pick","multiSelect":false,"options":[{"label":"A","description":"a"},{"label":"B","description":"b"}]}]},"tool_use_id":"toolu_9"}}`
	q := neutral(t, &n, ask)[0].(event.Question)
	if q.ID != "req-2" || q.CallID != "toolu_9" || len(q.Asks) != 1 || q.Asks[0].Text != "Which?" || len(q.Asks[0].Options) != 2 {
		t.Errorf("Question = %+v", q)
	}
	limit := `{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1790000000,"unifiedWindows":{"five_hour":{"utilization":1,"resetsAt":1790000000}}}}`
	evs := neutral(t, &n, limit)
	if len(evs) != 3 {
		t.Fatalf("rate limit = %+v", evs)
	}
	if q := evs[0].(event.Quota); q.Used("") != 100 {
		t.Errorf("Quota = %+v", q)
	}
	if b := evs[1].(event.Billing); b.Billing != usage.Plan {
		t.Errorf("Billing = %+v", b)
	}
	if l := evs[2].(event.Limited); l.Window != "five_hour" || l.ResetsAt.Unix() != 1790000000 {
		t.Errorf("Limited = %+v", l)
	}
}

// A sign-in's session is billed by what its requests say; an API key's
// from the start.
func TestNeutralBilling(t *testing.T) {
	var n Neutral
	over := `{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","isUsingOverage":true,"unifiedWindows":{"five_hour":{"utilization":1,"resetsAt":1790000000}}}}`
	if evs := neutral(t, &n, over); len(evs) != 2 || evs[1].(event.Billing).Billing != usage.Overage {
		t.Errorf("overage = %+v", evs)
	}
	for src, want := range map[string]int{"none": 1, "ANTHROPIC_API_KEY": 2} {
		evs := neutral(t, &n, `{"type":"system","subtype":"init","session_id":"s","apiKeySource":"`+src+`"}`)
		if len(evs) != want {
			t.Errorf("%s: %+v", src, evs)
		} else if want == 2 && evs[1].(event.Billing).Billing != usage.Metered {
			t.Errorf("%s: billing = %+v", src, evs[1])
		}
	}
}

// Claude Code's tasks and its background list read as rush's own.
func TestNeutralTasks(t *testing.T) {
	var n Neutral
	st := neutral(t, &n, `{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"tu1","task_type":"local_bash","description":"npm test","is_backgrounded":true}`)[0].(event.TaskStarted)
	if st.ID != "b1" || st.CallID != "tu1" || st.Kind != event.ShellTask || st.Label != "npm test" || !st.Background {
		t.Errorf("TaskStarted = %+v", st)
	}
	bg := neutral(t, &n, `{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash","description":"npm test"}]}`)[0].(event.Background)
	if len(bg.Tasks) != 1 || bg.Tasks[0].Kind != event.ShellTask || bg.Tasks[0].Type != "local_bash" || bg.Tasks[0].Label != "npm test" {
		t.Errorf("Background = %+v", bg)
	}
	// A subagent's command is in the list too, with the subagent's task.
	sub := neutral(t, &n, `{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b2","task_type":"local_bash","description":"bench","parent_task_id":"a1"}]}`)[0].(event.Background)
	if len(sub.Tasks) != 1 || sub.Tasks[0].Parent != "a1" {
		t.Errorf("a subagent's task = %+v", sub)
	}
	none := neutral(t, &n, `{"type":"system","subtype":"background_tasks_changed","tasks":[]}`)[0].(event.Background)
	if none.Tasks == nil || len(none.Tasks) != 0 {
		t.Errorf("an emptied list = %+v", none)
	}
	d := neutral(t, &n, `{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"tu1","status":"completed","output_file":"/t/b1.out"}`)[0].(event.TaskDone)
	if d.ID != "b1" || d.Status != "completed" || d.OutputFile != "/t/b1.out" {
		t.Errorf("TaskDone = %+v", d)
	}
	end := neutral(t, &n, `{"type":"result","subtype":"success","result":"All green.","total_cost_usd":0.5}`)[0].(event.TurnEnd)
	if end.Text != "All green." || end.Err != "" || end.Reason != "done" {
		t.Errorf("TurnEnd = %+v", end)
	}
}

// Claude Code's word that a request is tried again is rush's Retry.
func TestNeutralRetry(t *testing.T) {
	evs := neutral(t, &Neutral{}, `{"type":"system","subtype":"api_retry","attempt":3,"max_retries":10,"retry_delay_ms":4200,"error_status":529,"error":"overloaded_error"}`)
	r, ok := evs[0].(event.Retry)
	if len(evs) != 1 || !ok || r.Attempt != 3 || r.Max != 10 || r.Delay != 4200*time.Millisecond || r.Status != 529 {
		t.Fatalf("got %#v", evs)
	}
}
