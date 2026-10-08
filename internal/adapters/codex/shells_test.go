package codex

import (
	"context"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// A command still running when the model reads on is a background shell
// until its item completes, listed beside the spawned threads. Shapes as
// Codex 0.155's app-server sends them for exec_command with a short yield.
func TestBackgroundShell(t *testing.T) {
	c := newConn(context.Background())
	c.thread = "root"
	note := func(method string, params map[string]any) []event.Event {
		params["threadId"] = "root"
		b, _ := jsonx.Marshal(params)
		return c.notification(message{Method: method, Params: b})
	}
	cmd := func(status string, exit any) map[string]any {
		return map[string]any{"type": "commandExecution", "id": "exec-1", "command": "/bin/zsh -lc 'sleep 25; echo done'",
			"cwd": "/w", "processId": "78732", "source": "unifiedExecStartup", "status": status, "exitCode": exit, "commandActions": []any{}}
	}
	var log []string
	var lists [][]string
	take := func(evs []event.Event) {
		for _, ev := range evs {
			switch e := ev.(type) {
			case event.CallUpdated:
				if e.Call.Input.Background && e.Call.Input.Command == "sleep 25; echo done" {
					log = append(log, "backgrounded "+e.Call.ID)
				}
			case event.TaskStarted:
				if e.Kind == event.ShellTask && e.Background {
					log = append(log, "started "+e.ID+"/"+e.CallID)
				}
			case event.TaskDone:
				log = append(log, "done "+e.ID+"/"+e.Status)
			case event.Background:
				var ids []string
				for _, t := range e.Tasks {
					ids = append(ids, t.ID)
				}
				slices.Sort(ids)
				lists = append(lists, ids)
			}
		}
	}
	take(note("turn/started", map[string]any{"turn": map[string]any{"id": "t1", "status": "inProgress"}}))
	take(note("item/started", map[string]any{"item": map[string]any{"type": "subAgentActivity", "id": "call_k",
		"kind": "started", "agentThreadId": "kid", "agentPath": "/root/echoer"}}))
	take(note("item/started", map[string]any{"item": cmd("inProgress", nil)}))
	take(note("item/started", map[string]any{"item": map[string]any{"type": "agentMessage", "id": "m1"}}))
	take(note("turn/completed", map[string]any{"turn": map[string]any{"id": "t1", "status": "completed"}}))
	take(note("item/completed", map[string]any{"item": cmd("completed", 0)}))

	if want := []string{"backgrounded exec-1", "started exec-1/exec-1", "done exec-1/completed"}; !slices.Equal(log, want) {
		t.Errorf("events %v, want %v", log, want)
	}
	// The kid, then the kid and the shell, then the kid alone.
	if got := len(lists); got != 3 || strings.Join(lists[1], ",") != "exec-1,kid" || strings.Join(lists[2], ",") != "kid" {
		t.Errorf("background lists %v", lists)
	}
}

// From a rollout, a command whose item completed after its turn ended
// ran in the background.
func TestBackgroundShellFromRollout(t *testing.T) {
	r := (&rollout{t0: t0}).
		event("task_started", map[string]any{"turn_id": "u1"}).
		event("task_complete", map[string]any{"turn_id": "u1"}).
		event("task_started", map[string]any{"turn_id": "u2"}).
		event("item_completed", map[string]any{"turn_id": "u1", "item": map[string]any{"type": "CommandExecution", "id": "bg",
			"command": []string{"/bin/zsh", "-lc", "sleep 25"}, "status": "completed", "exit_code": 0}}).
		event("item_completed", map[string]any{"turn_id": "u2", "item": map[string]any{"type": "CommandExecution", "id": "fg",
			"command": []string{"/bin/zsh", "-lc", "ls"}, "status": "completed", "exit_code": 0}}).
		event("task_complete", map[string]any{"turn_id": "u2"})
	evs, err := readRollout(strings.NewReader(r.text()), time.Time{})
	if err != nil {
		t.Fatal(err)
	}
	bg := map[string]bool{}
	for _, ev := range evs {
		if m, ok := ev.(event.Message); ok {
			for _, p := range m.Parts {
				if p.Call != nil {
					bg[p.Call.ID] = p.Call.Input.Background
				}
			}
		}
	}
	if len(bg) != 2 || !bg["bg"] || bg["fg"] {
		t.Errorf("background %v", bg)
	}
}
