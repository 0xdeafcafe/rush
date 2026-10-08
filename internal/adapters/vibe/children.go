package vibe

import (
	"os"
	"path/filepath"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

var _ agent.ChildFinder = Adapter{}

// FindChild is the session parent's task call child started: Vibe saves
// it under the parent's folder (agents/<agent>_…) and lists it in the
// parent's meta.json by the call that started it.
func (a Adapter) FindChild(p agent.Profile, parent, child string, _ time.Time) (agent.Session, bool) {
	if parent == "" || child == "" {
		return agent.Session{}, false
	}
	for _, s := range a.Past(p) {
		if s.ID != parent {
			continue
		}
		dir := filepath.Dir(s.Transcript)
		var meta struct {
			Children []struct {
				ID    string `json:"session_id"`
				Call  string `json:"tool_call_id"`
				Agent string `json:"agent"`
				Path  string `json:"relative_path"`
			} `json:"child_sessions"`
		}
		b, err := os.ReadFile(filepath.Join(dir, "meta.json"))
		if err != nil || jsonx.Unmarshal(b, &meta) != nil {
			return agent.Session{}, false
		}
		for _, c := range meta.Children {
			if c.Call != child || c.Path == "" || !filepath.IsLocal(c.Path) {
				continue
			}
			file := filepath.Join(dir, c.Path, "messages.jsonl")
			st, err := os.Stat(file)
			if err != nil {
				return agent.Session{}, false
			}
			var cm struct {
				Start string `json:"start_time"`
			}
			at := st.ModTime()
			if b, err := os.ReadFile(filepath.Join(dir, c.Path, "meta.json")); err == nil && jsonx.Unmarshal(b, &cm) == nil {
				if t, err := time.Parse(time.RFC3339Nano, cm.Start); err == nil {
					at = t
				}
			}
			return agent.Session{Kind: Kind, Profile: p, ID: c.ID, Name: c.Agent, Cwd: s.Cwd, Transcript: file, State: "done", CreatedAt: at, UpdatedAt: st.ModTime()}, true
		}
		return agent.Session{}, false
	}
	return agent.Session{}, false
}
