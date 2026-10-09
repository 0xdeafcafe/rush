package acp

import (
	"os"
	"path/filepath"
	"sync"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// A task Kimi's own record says completed ends, though no notice of it
// came over ACP; one still running stays.
func TestEndTasksFromKimiRecords(t *testing.T) {
	home := t.TempDir()
	dir := filepath.Join(home, "sessions", "wd_x", "session_abc", "agents", "main", "tasks")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(dir, "bash-done.json"), []byte(`{"status":"completed"}`), 0o600)
	os.WriteFile(filepath.Join(dir, "bash-live.json"), []byte(`{"status":"running"}`), 0o600)
	bg := tool.Call{ID: "c1", Input: tool.Input{Background: true}}
	live := tool.Call{ID: "c2", Input: tool.Input{Background: true}}
	s := &Session{o: Options{Adapter: "kimi", Env: []string{"KIMI_CODE_HOME=" + home}}, id: "session_abc",
		calls: map[string]*call{"c1": {c: bg, task: "bash-done"}, "c2": {c: live, task: "bash-live"}}}
	s.qcond = sync.NewCond(&s.qmu)
	got := s.EndTasks()
	if len(got) != 1 || got[0] != "bash-done" || !s.calls["c1"].ended || s.calls["c2"].ended {
		t.Fatalf("ended %v", got)
	}
	if again := s.EndTasks(); len(again) != 0 {
		t.Fatalf("ended twice: %v", again)
	}
}
