package host

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// A turn that leaves a task in the background ends with Claude Code still
// needed: it rests only once the task is done and it has reported it.
const fakeBackground = `#!/bin/sh
sid=SID; prev=; for a in "$@"; do case "$prev" in --session-id|--resume) sid=$a ;; esac; prev=$a; done
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'","model":"claude-haiku-4-5","permissionMode":"default","tools":["Bash"]}'
while read -r line; do
  case "$line" in
  *'"type":"user"'*)
    echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash","description":"npm test"}]}'
    echo '{"type":"result","subtype":"success","result":"Started the tests.","total_cost_usd":0.01}'
    ( sleep 1; echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
      echo '{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"The tests passed."}]}}'
      echo '{"type":"result","subtype":"success","result":"The tests passed.","total_cost_usd":0.02}' ) &
    ;;
  esac
done
`

func TestRestsOnceBackgroundEnds(t *testing.T) {
	bin := setup(t)
	if err := os.WriteFile(bin, []byte(fakeBackground), 0o755); err != nil {
		t.Fatal(err)
	}
	cfg, err := Spawn(Config{Cwd: filepath.Dir(bin), Prompt: "run the tests", Binary: bin, IdleStop: Duration(200 * time.Millisecond)})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	defer c.Stop()
	idle := next(t, c, func(ev any) bool {
		i, ok := ev.(InfoEvent)
		return ok && i.Info.State == "idle" && i.Info.Detail != "" && len(i.Info.Background) == 1
	}).(InfoEvent).Info
	if idle.ClaudePID == 0 {
		t.Fatal("stopped with a task still running")
	}
	at := time.Now()
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.ClaudePID == 0 })
	if waited := time.Since(at); waited < 700*time.Millisecond {
		t.Fatalf("rested %s after the turn, before its task ended", waited)
	}
}
