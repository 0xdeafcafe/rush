package convo

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Subagent is one run of a subagent, as Claude Code records it beside the
// session's transcript: <session>/subagents/agent-<id>.jsonl and its meta.
type Subagent struct {
	ID          string
	Type        string `json:"agentType"`
	Description string `json:"description"`
	Model       string `json:"model"`
	ToolUseID   string `json:"toolUseId"`
	Depth       int    `json:"spawnDepth"` // 1 for one the session started, more for one a run started
	Path        string // its own transcript
	Born        int64  // when it started: its meta file's time as first seen
	Mod         int64  // when it last wrote, for ordering
	Size        int64  // its transcript's size, to skip reading one that hasn't grown
}

// ListSubagents finds the subagents of the session whose transcript is at
// path, oldest first.
func ListSubagents(transcript string) []Subagent { return new(Subagents).List(transcript) }

// Subagents lists a session's subagents again and again, reading each
// run's meta file only when it's new or has changed, and the folder only
// when something was added to it.
type Subagents struct {
	metas   map[string]subMeta
	dir     string
	dirMod  time.Time
	names   []string // the meta files, as of dirMod
	scanned time.Time
}

type subMeta struct {
	mod  time.Time
	size int64
	sa   Subagent
	ok   bool
}

// List is ListSubagents.
func (l *Subagents) List(transcript string) []Subagent {
	dir := filepath.Join(strings.TrimSuffix(transcript, ".jsonl"), "subagents")
	if l.metas == nil || l.dir != dir {
		*l = Subagents{metas: map[string]subMeta{}, dir: dir}
	}
	// A new run adds files to the folder, which changes its time; the
	// metas are re-read at least every 10s in case one was rewritten.
	st, err := os.Stat(dir)
	if err != nil {
		l.names, l.dirMod = nil, time.Time{}
		return nil
	}
	fresh := time.Since(l.scanned) < 10*time.Second && st.ModTime().Equal(l.dirMod)
	if !fresh {
		l.names, _ = filepath.Glob(filepath.Join(dir, "agent-*.meta.json"))
		l.dirMod, l.scanned = st.ModTime(), time.Now()
	}
	out := make([]Subagent, 0, len(l.names))
	for _, meta := range l.names {
		m, seen := l.metas[meta]
		if !seen || !fresh || !m.ok {
			st, err := os.Stat(meta)
			if err != nil {
				continue
			}
			if !seen || !m.mod.Equal(st.ModTime()) || m.size != st.Size() {
				born := m.sa.Born // a rewrite doesn't make it any younger
				m = subMeta{mod: st.ModTime(), size: st.Size()}
				m.sa.Born = born
				if born == 0 {
					m.sa.Born = st.ModTime().UnixNano()
				}
				if b, err := os.ReadFile(meta); err == nil && jsonx.Unmarshal(b, &m.sa) == nil {
					m.ok = true
					m.sa.ID = strings.TrimSuffix(strings.TrimPrefix(filepath.Base(meta), "agent-"), ".meta.json")
					m.sa.Path = filepath.Join(dir, "agent-"+m.sa.ID+".jsonl")
				}
				l.metas[meta] = m // a half-written one changes size, and is read again
			}
		}
		if !m.ok {
			continue
		}
		sa := m.sa
		if st, err := os.Stat(sa.Path); err == nil {
			sa.Mod, sa.Size = st.ModTime().UnixNano(), st.Size()
		}
		out = append(out, sa)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Mod < out[j].Mod })
	return out
}

// Step finds a tool call by its id.
func (s *Session) Step(id string) *Step { return s.byID[id] }

// SubagentTail reads a subagent's transcript. Its lines are all marked as a
// sidechain, which a session's own Tail skips, so this one keeps them.
func SubagentTail(path string) *Tail { return &Tail{Path: path, Sess: New(), sidechain: true} }

// SubagentStats follows a subagent's transcript for its row alone: times,
// tool calls, tokens, model and its latest words. It keeps none of the
// inputs, outputs or earlier words, so a session with hundreds of runs
// costs kilobytes each rather than megabytes. Open one with SubagentTail
// to see its conversation.
func SubagentStats(path string) *Tail {
	t := SubagentTail(path)
	t.Sess.light = true
	return t
}
