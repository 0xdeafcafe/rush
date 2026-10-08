package codex

import (
	"bytes"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// liveFor is how recently a rollout must have been written to for its
// thread to count as running; workingFor, to count as working.
const (
	liveFor    = 2 * time.Minute
	workingFor = 15 * time.Second
)

// Live are the threads whose rollouts Codex wrote to lately: a codex in a
// terminal, most often. Codex keeps no record of which threads run that is
// cheap to read and can be trusted (its thread-writer-locks outlive their
// processes, and matching processes to rollouts would mean lsof on every
// refresh), so a rollout written in the last two minutes is taken as
// running, and one in the last fifteen seconds as working. Only the last
// week's folders are looked in: rollouts are filed by the day they began.
// Threads rush runs itself are also here; the host's rows claim them.
func (Adapter) Live(p agent.Profile) []agent.Session {
	now := time.Now()
	var names map[string]string
	var out []agent.Session
	for d := 0; d < 7; d++ {
		day := now.AddDate(0, 0, -d)
		dir := filepath.Join(p.Dir, "sessions", day.Format("2006"), day.Format("01"), day.Format("02"))
		ents, _ := os.ReadDir(dir)
		for _, e := range ents {
			if e.IsDir() || !strings.HasPrefix(e.Name(), "rollout-") || !strings.HasSuffix(e.Name(), ".jsonl") {
				continue
			}
			fi, err := e.Info()
			if err != nil || now.Sub(fi.ModTime()) > liveFor {
				continue
			}
			s, ok := pastSession(filepath.Join(dir, e.Name()))
			if !ok {
				continue
			}
			if names == nil {
				names = threadNames(filepath.Join(p.Dir, "session_index.jsonl"))
			}
			if n := names[s.ID]; n != "" {
				s.Name = oneLine(n)
			}
			s.Kind, s.Profile, s.State = Kind, p, "idle"
			if now.Sub(fi.ModTime()) < workingFor {
				s.State = "working"
			}
			out = append(out, s)
		}
	}
	return out
}

// headLines is how many lines of a rollout Past reads at most looking for
// its first prompt.
const headLines = 400

// Past is every rollout under the profile's sessions folder, newest
// first. Threads another thread started (spawned agents and the guardian
// reviewer) are left out: they belong to the thread that started them.
// A thread's name is the one Codex keeps in session_index.jsonl when it
// has one (only renamed or titled threads are there), else its first
// prompt.
func (Adapter) Past(p agent.Profile) []agent.Session {
	names := threadNames(filepath.Join(p.Dir, "session_index.jsonl"))
	var out []agent.Session
	_ = filepath.WalkDir(filepath.Join(p.Dir, "sessions"), func(path string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasPrefix(d.Name(), "rollout-") || !strings.HasSuffix(d.Name(), ".jsonl") {
			return nil
		}
		s, ok := pastSession(path)
		if !ok {
			return nil
		}
		s.Kind, s.Profile, s.State = Kind, p, "done"
		if n := names[s.ID]; n != "" {
			s.Name = oneLine(n)
		}
		out = append(out, s)
		return nil
	})
	sort.SliceStable(out, func(i, j int) bool { return out[i].UpdatedAt.After(out[j].UpdatedAt) })
	return out
}

// heads are rollouts' heads as last read, by path: a rollout is read again
// only once it has changed, and one finished never does.
var heads = struct {
	sync.Mutex
	m map[string]head
}{m: map[string]head{}}

type head struct {
	mod  time.Time
	size int64
	s    agent.Session
	ok   bool
}

// pastSession reads what a list needs from the head of a rollout. ok is
// false for a subagent's rollout or one that can't be read.
func pastSession(path string) (agent.Session, bool) {
	fi, err := os.Stat(path)
	if err != nil {
		return agent.Session{}, false
	}
	heads.Lock()
	h, ok := heads.m[path]
	heads.Unlock()
	if ok && h.mod.Equal(fi.ModTime()) && h.size == fi.Size() {
		return h.s, h.ok
	}
	s, ok := readHead(path, fi.ModTime())
	heads.Lock()
	heads.m[path] = head{mod: fi.ModTime(), size: fi.Size(), s: s, ok: ok}
	heads.Unlock()
	return s, ok
}

func readHead(path string, mod time.Time) (agent.Session, bool) {
	f, err := os.Open(path)
	if err != nil {
		return agent.Session{}, false
	}
	defer f.Close()
	s := agent.Session{ID: idFromName(filepath.Base(path)), Transcript: path, UpdatedAt: mod}
	meta, sub, n := false, false, 0
	_ = readHeadLines(f, func(b []byte) bool {
		n++
		// Most lines are ones a head doesn't need, some of them big: those
		// are passed over on their type alone, never decoded.
		if n > 1 && !headWants(b, meta, s) {
			return n < headLines && (s.Name == "" || s.Model == "")
		}
		var l rolloutLine
		if jsonx.Unmarshal(b, &l) != nil {
			return n < headLines
		}
		if s.CreatedAt.IsZero() {
			s.CreatedAt = parseTime(l.Timestamp)
		}
		switch l.Type {
		case "session_meta":
			var m sessionMeta
			if meta || jsonx.Unmarshal(l.Payload, &m) != nil {
				break
			}
			meta = true
			if m.subagent() {
				sub = true
				return false
			}
			if m.ID != "" {
				s.ID = m.ID
			}
			s.Cwd, s.Headless = m.Cwd, m.exec()
			if t := parseTime(m.Timestamp); !t.IsZero() {
				s.CreatedAt = t
			}
		case "turn_context":
			var c turnContext
			if s.Model == "" && jsonx.Unmarshal(l.Payload, &c) == nil {
				s.Model = c.Model
			}
		case "response_item":
			var ri responseItem
			if s.Name == "" && jsonx.Unmarshal(l.Payload, &ri) == nil && ri.Type == "message" && ri.Role == "user" {
				if m, ok := userPrompt(ri); ok {
					for _, p := range m.Parts {
						if s.Name == "" && strings.TrimSpace(p.Text) != "" {
							s.Name = oneLine(p.Text)
						}
					}
				}
			}
		}
		return n < headLines && (s.Name == "" || s.Model == "")
	})
	if sub {
		return agent.Session{}, false
	}
	if s.UpdatedAt.IsZero() {
		s.UpdatedAt = s.CreatedAt
	}
	return s, true
}

var (
	typeMark     = []byte(`"type":"`)
	payloadMark  = []byte(`"payload":`)
	userRoleMark = []byte(`"role":"user"`)
)

// headWants is whether readHead needs a rollout line, from its type: the
// session's meta once, a turn's context until the model is known, and a
// user's message until the session is named.
func headWants(b []byte, meta bool, s agent.Session) bool {
	// Codex writes the line's type before its payload; a line that isn't
	// written so is decoded to see.
	head := b[:min(len(b), 160)]
	i := bytes.Index(head, typeMark)
	if p := bytes.Index(head, payloadMark); i < 0 || p >= 0 && p < i {
		return true
	}
	t := b[i+len(typeMark):]
	switch {
	case bytes.HasPrefix(t, []byte(`session_meta"`)):
		return !meta
	case bytes.HasPrefix(t, []byte(`turn_context"`)):
		return s.Model == ""
	case bytes.HasPrefix(t, []byte(`response_item"`)):
		return s.Name == "" && bytes.Contains(b, userRoleMark)
	}
	return false
}

// idFromName is the thread id at the end of a rollout's file name,
// rollout-2026-09-22T17-01-09-<uuid>.jsonl.
func idFromName(name string) string {
	name = strings.TrimSuffix(name, ".jsonl")
	if len(name) >= 36 {
		return name[len(name)-36:]
	}
	return name
}

// threadNames is session_index.jsonl: each named thread's latest name.
func threadNames(path string) map[string]string {
	f, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer f.Close()
	names := map[string]string{}
	_ = readLines(f, func(b []byte) bool {
		var e struct {
			ID         string `json:"id"`
			ThreadName string `json:"thread_name"`
		}
		if jsonx.Unmarshal(b, &e) == nil && e.ID != "" && strings.TrimSpace(e.ThreadName) != "" {
			names[e.ID] = e.ThreadName
		}
		return true
	})
	return names
}

// nameLen is how many characters of a prompt name a thread.
const nameLen = 80

// oneLine is text on one line, cut to nameLen characters.
func oneLine(text string) string {
	// Only as much as could show is collapsed: a prompt can be a whole
	// pasted file.
	var b strings.Builder
	n, gap := 0, false
	for _, c := range text {
		if unicode.IsSpace(c) {
			gap = b.Len() > 0
			continue
		}
		if gap {
			b.WriteByte(' ')
			n, gap = n+1, false
		}
		b.WriteRune(c)
		if n++; n > nameLen {
			break
		}
	}
	s := b.String()
	if r := []rune(s); len(r) > nameLen {
		return strings.TrimSpace(string(r[:nameLen-1])) + "…"
	}
	return s
}

var (
	_ agent.Discoverer    = Adapter{}
	_ agent.HistoryReader = Adapter{}
)
