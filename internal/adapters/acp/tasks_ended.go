package acp

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// EndTasks ends those of its background tasks its agent's own records say
// have ended, and says which. Kimi tells of a task that ends between
// turns only in the next turn's context, never over ACP, so without this
// one finished in a minute reads as running for good.
func (s *Session) EndTasks() []string {
	if s.o.Adapter != "kimi" {
		return nil
	}
	home := kimiHome()
	for _, kv := range s.o.Env {
		if v, ok := strings.CutPrefix(kv, "KIMI_CODE_HOME="); ok {
			home = v
		}
	}
	s.mu.Lock()
	var open []*call
	for _, c := range s.calls {
		if c.task != "" && c.c.Input.Background && !c.ended {
			open = append(open, c)
		}
	}
	dir := s.id
	s.mu.Unlock()
	if !strings.HasPrefix(dir, "session_") {
		dir = "session_" + dir
	}
	var ended []string
	for _, c := range open {
		paths, _ := filepath.Glob(filepath.Join(home, "sessions", "wd_*", dir, "agents", "*", "tasks", c.task+".json"))
		for _, p := range paths {
			raw, err := os.ReadFile(p)
			var t struct{ Status string }
			if err != nil || jsonx.Unmarshal(raw, &t) != nil || t.Status == "" || t.Status == "running" {
				continue
			}
			s.mu.Lock()
			if !c.ended {
				c.ended = true
				s.emit(event.TaskDone{ID: c.task, CallID: c.c.ID, Status: t.Status})
				s.emitBackground()
				ended = append(ended, c.task)
			}
			s.mu.Unlock()
			break
		}
	}
	return ended
}
