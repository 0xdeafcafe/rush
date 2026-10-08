package convo

import (
	"encoding/json/jsontext"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Claude Code's own lines, from a real session: a foreground command
// moved to the background and stopped, then one started in the background.
var jobLines = []string{
	`{"type":"system","subtype":"task_started","task_id":"bomth3m0o","tool_use_id":"toolu_01","description":"for i in $(seq 1 30); do echo tick $i; sleep 1; done","is_backgrounded":false,"task_type":"local_bash"}`,
	`{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bomth3m0o","task_type":"local_bash","description":"for i in $(seq 1 30); do echo tick $i; sleep 1; done"}]}`,
	`{"type":"system","subtype":"task_updated","task_id":"bomth3m0o","patch":{"is_backgrounded":true}}`,
	`{"type":"system","subtype":"background_tasks_changed","tasks":[]}`,
	`{"type":"system","subtype":"task_updated","task_id":"bomth3m0o","patch":{"status":"killed","end_time":1790412912883}}`,
	`{"type":"system","subtype":"task_notification","task_id":"bomth3m0o","tool_use_id":"toolu_01","status":"stopped","output_file":"/tmp/x/tasks/bomth3m0o.output","summary":"for i in $(seq 1 30); do echo tick $i; sleep 1; done"}`,
	`{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b5hybj1hn","task_type":"local_bash","description":"tick loop progress (1-30)"}]}`,
	`{"type":"system","subtype":"task_started","task_id":"b5hybj1hn","tool_use_id":"toolu_02","description":"tick loop progress (1-30)","is_backgrounded":true,"task_type":"local_bash"}`,
}

func TestJobs(t *testing.T) {
	s := New()
	now := time.Now()
	apply := func(i int) {
		ev, err := headless.Decode([]byte(jobLines[i]))
		if err != nil {
			t.Fatal(err)
		}
		s.Apply(ev, now)
	}
	apply(0)
	j := s.Job("bomth3m0o")
	if j == nil || !j.Running() || j.Background || j.Kind() != "shell" || j.ToolUseID != "toolu_01" {
		t.Fatalf("started: %+v", j)
	}
	apply(1)
	apply(2)
	if !j.Background || !j.Running() {
		t.Fatalf("backgrounded: %+v", j)
	}
	apply(3)
	apply(4)
	apply(5)
	if j.Status != "stopped" || j.OutputFile == "" || s.TaskStatus["bomth3m0o"] != "stopped" {
		t.Fatalf("stopped: %+v", j)
	}
	apply(6)
	apply(7)
	run := s.RunningJobs()
	if len(run) != 1 || run[0].ID != "b5hybj1hn" || !run[0].Background {
		t.Fatalf("running: %+v", run)
	}
	// A turn ending doesn't end what runs in the background.
	s.Apply(headless.Result{Subtype: "success"}, now)
	if len(s.RunningJobs()) != 1 {
		t.Fatal("a background job ended with the turn")
	}
	// The host says Claude Code is gone: so is everything it ran.
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, State: "idle"}}, now)
	if len(s.RunningJobs()) != 0 || s.Job("b5hybj1hn").Status != "ended" {
		t.Fatalf("after exit: %+v", s.Job("b5hybj1hn"))
	}
}

// A replay that no longer reaches a task's start still has it, from the
// host's list.
func TestJobsFromInfo(t *testing.T) {
	s := New()
	at := time.Now().Add(-time.Hour)
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, Background: []host.Task{{ID: "m1", Type: "monitor_mcp", Label: "watch CI", StartedAt: at}}}}, time.Now())
	j := s.Job("m1")
	if j == nil || !j.Running() || j.Kind() != "monitor" || !j.Start.Equal(at) {
		t.Fatalf("%+v", j)
	}
	// A foreground command the turn was waiting on ends with it.
	ev, _ := headless.Decode([]byte(`{"type":"system","subtype":"task_started","task_id":"f1","tool_use_id":"t","description":"make","task_type":"local_bash"}`))
	s.Apply(ev, time.Now())
	s.Apply(headless.Result{Subtype: "success"}, time.Now())
	if s.Job("f1").Running() || !j.Running() {
		t.Fatal("foreground job outlived its turn, or the background one didn't")
	}
}

// A subagent's run is a task to Claude Code too, but not background work:
// the background view's tasks leave it out, known by its type or, heard of
// only from its end, by the Agent call that started it.
func TestSubagentJobs(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(host.Sent{Text: "go"}, now)
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{
		{Type: "tool_use", ID: "tA", Name: "Agent", Input: jsontext.Value(`{"description":"look","subagent_type":"Explore"}`)},
		{Type: "tool_use", ID: "tB", Name: "Agent", Input: jsontext.Value(`{"description":"older"}`)},
	}}, now)
	for _, l := range []string{
		`{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"tA","description":"look","subagent_type":"Explore","is_backgrounded":true,"task_type":"local_agent"}`,
		`{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"tS","description":"npm run dev","is_backgrounded":true,"task_type":"local_bash"}`,
		`{"type":"system","subtype":"task_notification","task_id":"a0","tool_use_id":"tB","status":"completed","summary":"older"}`,
	} {
		ev, err := headless.Decode([]byte(l))
		if err != nil {
			t.Fatal(err)
		}
		s.Apply(ev, now)
	}
	if w := s.WorkJobs(); len(w) != 1 || w[0].ID != "b1" {
		t.Fatalf("background work: %+v", w)
	}
	if j := s.SubagentJob("a1", ""); j == nil || !j.Running() {
		t.Fatalf("by agent id: %+v", j)
	}
	if j := s.SubagentJob("", "tB"); j == nil || j.ID != "a0" || j.Running() {
		t.Fatalf("by its call: %+v", j)
	}
	if s.SubagentJob("b1", "tS") != nil {
		t.Fatal("a shell is no subagent")
	}
}

// A message sent to a subagent after it finished runs it again under the
// same id: Claude Code starts its task anew, and it's running again.
func TestWokenSubagentRunsAgain(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(headless.TaskStarted{ID: "a1", ToolUseID: "tA", Type: "local_agent", Backgrounded: true}, now)
	s.Apply(headless.TaskDone{ID: "a1", ToolUseID: "tA", Status: "completed"}, now)
	if j := s.Job("a1"); j.Running() || s.TaskStatus["a1"] != "completed" {
		t.Fatalf("finished: %+v", j)
	}
	s.Apply(headless.TaskStarted{ID: "a1", Type: "local_agent", Backgrounded: true}, now.Add(time.Minute))
	if j := s.Job("a1"); !j.Running() || s.TaskStatus["a1"] != "" || !j.End.IsZero() {
		t.Fatalf("woken: %+v, status %q", j, s.TaskStatus["a1"])
	}
	s.Apply(headless.TaskDone{ID: "a1", Status: "completed"}, now.Add(2*time.Minute))
	// The host's list of what runs in the background says so too.
	s.Apply(headless.BackgroundTasks{Tasks: []headless.BackgroundTask{{ID: "a1", Type: "local_agent"}}}, now.Add(3*time.Minute))
	if !s.Job("a1").Running() {
		t.Fatal("listed as running in the background, yet not running")
	}
}

// A background task finishing while nothing runs wakes the agent with no
// message: the turn it starts is put down to the task.
func TestWokenByABackgroundTask(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(host.Sent{Text: "run the tests in the background"}, now)
	for _, l := range []string{
		`{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_09","description":"go test ./...","is_backgrounded":true,"task_type":"local_bash"}`,
	} {
		ev, _ := headless.Decode([]byte(l))
		s.Apply(ev, now)
	}
	s.Apply(headless.Result{Subtype: "success"}, now)
	ev, _ := headless.Decode([]byte(`{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"toolu_09","status":"completed","summary":"go test ./..."}`))
	s.Apply(ev, now.Add(time.Minute))
	s.Apply(headless.Delta{Text: "The tests pass."}, now.Add(time.Minute+time.Second))
	tn := s.Turns[len(s.Turns)-1]
	if len(s.Turns) != 2 || tn.From != "background shell · completed" || tn.Cause != "go test ./..." {
		t.Fatalf("turn %d: from %q, cause %q", len(s.Turns), tn.From, tn.Cause)
	}
}

// A woken turn's heredoc command is drawn as a shell step's is: indented
// as written and highlighted, not flattened into a message.
func TestWokenByAHeredoc(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(host.Sent{Text: "go"}, now)
	cmd := "python3 - <<'EOF'\nif x:\n    print(1)\nEOF"
	desc, _ := jsonx.Marshal(cmd)
	ev, _ := headless.Decode([]byte(`{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_09","description":` + string(desc) + `,"is_backgrounded":true,"task_type":"local_bash"}`))
	s.Apply(ev, now)
	s.Apply(headless.Result{Subtype: "success"}, now)
	ev, _ = headless.Decode([]byte(`{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"toolu_09","status":"completed"}`))
	s.Apply(ev, now.Add(time.Minute))
	s.Apply(headless.Delta{Text: "done"}, now.Add(time.Minute+time.Second))
	tn := s.Turns[len(s.Turns)-1]
	if tn.Command != cmd || tn.Cause != "python3 - <<'EOF'" {
		t.Fatalf("command %q, cause %q", tn.Command, tn.Cause)
	}
	out := plain(s.Render(Options{Width: 100, Now: now.Add(2 * time.Minute)}))
	if !strings.Contains(out, "    print(1)") {
		t.Fatalf("heredoc body lost its indent:\n%s", out)
	}
}

// A task that finishes while a turn runs is held: Claude Code wakes the
// agent for it once the turn ends, and that turn is put down to it.
func TestWokenByATaskThatEndedMidTurn(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(host.Sent{Text: "run the tests in the background"}, now)
	for _, l := range []string{
		`{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_09","description":"go test ./...","is_backgrounded":true,"task_type":"local_bash"}`,
		`{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"toolu_09","status":"completed","summary":"go test ./..."}`,
	} {
		ev, _ := headless.Decode([]byte(l))
		s.Apply(ev, now)
	}
	s.Apply(headless.Result{Subtype: "success"}, now.Add(5*time.Minute))
	s.Apply(headless.Delta{Text: "The tests pass."}, now.Add(5*time.Minute+time.Second))
	tn := s.Turns[len(s.Turns)-1]
	if len(s.Turns) != 2 || tn.Cause != "go test ./..." {
		t.Fatalf("turn %d: from %q, cause %q", len(s.Turns), tn.From, tn.Cause)
	}

	// Your next message starts afresh: nothing held is put down to it.
	s.Apply(headless.Result{Subtype: "success"}, now.Add(6*time.Minute))
	s.Apply(host.Sent{Text: "thanks"}, now.Add(6*time.Minute))
	if tn := s.Turns[len(s.Turns)-1]; tn.Cause != "" {
		t.Fatalf("your turn put down to %q", tn.Cause)
	}
}

// A background subagent working on after the turn that started it ended
// opens no turn: its steps stay with the turn it runs under.
func TestSubagentAfterItsTurnOpensNoTurn(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(host.Sent{Text: "look into it in the background"}, now)
	s.Apply(headless.Message{Role: "assistant", ID: "m1", Blocks: []headless.Block{{Type: "tool_use", ID: "toolu_A", Name: "Agent", Input: []byte(`{"description":"dig","run_in_background":true}`)}}}, now)
	s.Apply(headless.Result{Subtype: "success"}, now)
	s.Apply(headless.Message{Role: "assistant", ID: "m2", ParentToolUseID: "toolu_A", Blocks: []headless.Block{{Type: "tool_use", ID: "toolu_B", Name: "Bash", Input: []byte(`{"command":"ls"}`)}}}, now.Add(time.Second))
	s.Apply(headless.Message{Role: "assistant", ID: "m3", ParentToolUseID: "toolu_unknown", Blocks: []headless.Block{{Type: "tool_use", ID: "toolu_C", Name: "Bash", Input: []byte(`{"command":"pwd"}`)}}}, now.Add(time.Second))
	if len(s.Turns) != 1 || s.Live() != nil {
		t.Fatalf("%d turns, live %v", len(s.Turns), s.Live() != nil)
	}
	if a := s.byID["toolu_A"]; len(a.Children) != 1 || s.Turns[0].Steps() != 2 {
		t.Fatalf("children %d, steps %d", len(a.Children), s.Turns[0].Steps())
	}
}

// A background subagent's call waiting on you stays waiting when a later
// turn of the main agent, which it lands in, ends.
func TestSubagentApprovalOutlivesTurn(t *testing.T) {
	s := New()
	now := time.Now()
	s.Apply(host.Sent{Text: "dig in the background"}, now)
	s.Apply(headless.Message{Role: "assistant", ID: "m1", Blocks: []headless.Block{{Type: "tool_use", ID: "toolu_A", Name: "Agent", Input: []byte(`{"description":"dig","run_in_background":true}`)}}}, now)
	s.Apply(headless.Result{Subtype: "success"}, now)
	s.Apply(host.Sent{Text: "and meanwhile?"}, now.Add(time.Minute))
	s.Apply(headless.Message{Role: "assistant", ID: "m2", ParentToolUseID: "toolu_A", Blocks: []headless.Block{{Type: "tool_use", ID: "toolu_B", Name: "Bash", Input: []byte(`{"command":"sed -n 1p x"}`)}}}, now.Add(time.Minute))
	s.Apply(headless.PermissionRequest{ID: "r1", Tool: "Bash", ToolUseID: "toolu_B", Input: []byte(`{"command":"sed -n 1p x"}`)}, now.Add(time.Minute))
	s.Apply(headless.Result{Subtype: "success"}, now.Add(2*time.Minute))
	if p := s.Pending(); len(p) != 1 || p[0].ID != "toolu_B" {
		t.Fatalf("the subagent's ask was dropped with the turn: %d pending", len(p))
	}
	// One whose run we never saw start, with no turn to hold it, still asks.
	s.Apply(headless.Message{Role: "assistant", ID: "m3", ParentToolUseID: "toolu_unknown", Blocks: []headless.Block{{Type: "tool_use", ID: "toolu_C", Name: "Bash", Input: []byte(`{"command":"pwd"}`)}}}, now.Add(3*time.Minute))
	s.Apply(headless.PermissionRequest{ID: "r2", Tool: "Bash", ToolUseID: "toolu_C", Input: []byte(`{"command":"pwd"}`)}, now.Add(3*time.Minute))
	if p := s.Pending(); len(p) != 2 {
		t.Fatalf("an orphan subagent's ask should show: %d pending", len(p))
	}
}
