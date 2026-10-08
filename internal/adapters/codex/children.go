package codex

import (
	"os"
	"path"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

var _ agent.ChildFinder = Adapter{}

// childMeta is what a spawned thread's rollout says of where it came from.
type childMeta struct {
	ID     string `json:"id"`
	Parent string `json:"parent_thread_id"`
	Source struct {
		Subagent struct {
			Spawn struct {
				Parent   string `json:"parent_thread_id"`
				Path     string `json:"agent_path"`
				Nickname string `json:"agent_nickname"`
			} `json:"thread_spawn"`
		} `json:"subagent"`
	} `json:"source"`
}

// FindChild is the thread parent spawned as child: its thread id, or the
// task it was given (spawn_agent's task_name). Rollouts are filed by the
// day they began, so only those days' folders are looked in.
func (Adapter) FindChild(p agent.Profile, parent, child string, start time.Time) (agent.Session, bool) {
	if parent == "" || child == "" {
		return agent.Session{}, false
	}
	from := start.Add(-3 * time.Second)
	// Filed by the local day, begun a day early for a clock read elsewhere.
	for day := from.Local().AddDate(0, 0, -1); !day.After(time.Now().Add(24 * time.Hour)); day = day.AddDate(0, 0, 1) {
		dir := filepath.Join(p.Dir, "sessions", day.Format("2006"), day.Format("01"), day.Format("02"))
		ents, _ := os.ReadDir(dir)
		for _, e := range ents {
			name := e.Name()
			if !strings.HasPrefix(name, "rollout-") || !strings.HasSuffix(name, ".jsonl") {
				continue
			}
			if fi, err := e.Info(); err != nil || fi.ModTime().Before(from) {
				continue
			}
			if s, ok := childOf(filepath.Join(dir, name), parent, child); ok {
				s.Kind, s.Profile = Kind, p
				return s, true
			}
		}
	}
	return agent.Session{}, false
}

// childOf reads the rollout at file's meta: the session, when it's the
// thread parent spawned as child.
func childOf(file, parent, child string) (agent.Session, bool) {
	f, err := os.Open(file)
	if err != nil {
		return agent.Session{}, false
	}
	defer f.Close()
	var l rolloutLine
	_ = readHeadLines(f, func(b []byte) bool { _ = jsonx.Unmarshal(b, &l); return false })
	var m childMeta
	if l.Type != "session_meta" || jsonx.Unmarshal(l.Payload, &m) != nil {
		return agent.Session{}, false
	}
	sp := m.Source.Subagent.Spawn
	if m.Parent != parent && sp.Parent != parent {
		return agent.Session{}, false
	}
	if m.ID != child && path.Base(sp.Path) != path.Base(child) {
		return agent.Session{}, false
	}
	at := parseTime(l.Timestamp)
	return agent.Session{ID: m.ID, Name: sp.Nickname, Transcript: file, CreatedAt: at, UpdatedAt: at}, true
}
