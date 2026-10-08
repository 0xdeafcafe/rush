package pi

import (
	"bufio"
	"bytes"
	"io"
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

// Pi keeps a session in a file of its own, <dir>/sessions/--<cwd>--/
// <time>_<id>.jsonl, the cwd's slashes made dashes. The first line is the
// session's header; every other is an entry in a tree, each naming its
// parent, the last written the tip.

// sessionsDir is where p's sessions are: PI_CODING_AGENT_SESSION_DIR for
// the user's own Pi, else the profile's sessions folder.
func sessionsDir(p agent.Profile) string {
	if d := os.Getenv("PI_CODING_AGENT_SESSION_DIR"); d != "" && p.Dir == Home() {
		return d
	}
	return filepath.Join(p.Dir, "sessions")
}

// sessionFiles are every session file of p's.
func sessionFiles(p agent.Profile) []string {
	m, _ := filepath.Glob(filepath.Join(sessionsDir(p), "*", "*.jsonl"))
	return m
}

// sessionFile is where session id is kept in p, or "".
func sessionFile(p agent.Profile, id string) string {
	if m, _ := filepath.Glob(filepath.Join(sessionsDir(p), "*", "*_"+id+".jsonl")); len(m) > 0 {
		return m[len(m)-1]
	}
	return ""
}

// idOfFile is the session id in a session file's name, <time>_<id>.jsonl;
// "" for what isn't one.
func idOfFile(path string) string {
	name := filepath.Base(path)
	if !strings.HasSuffix(name, ".jsonl") {
		return ""
	}
	_, id, ok := strings.Cut(strings.TrimSuffix(name, ".jsonl"), "_")
	if !ok {
		return ""
	}
	return id
}

// liveFor is how recently a session must have been written to count as
// running; workingFor, to count as working.
const (
	liveFor    = 2 * time.Minute
	workingFor = 15 * time.Second
)

// Live are the sessions Pi wrote to lately: a pi in a terminal, most
// often. Pi keeps no record of which sessions run, so one written in the
// last two minutes is taken as running, and in the last fifteen seconds as
// working. Sessions rush runs itself are also here; the host's rows claim
// them.
func (Adapter) Live(p agent.Profile) []agent.Session {
	now := time.Now()
	var out []agent.Session
	for _, path := range sessionFiles(p) {
		fi, err := os.Stat(path)
		if err != nil || now.Sub(fi.ModTime()) > liveFor {
			continue
		}
		s, ok := pastSession(path)
		if !ok {
			continue
		}
		s.Kind, s.Profile, s.State = p.Kind, p, "idle"
		if now.Sub(fi.ModTime()) < workingFor {
			s.State = "working"
		}
		out = append(out, s)
	}
	return out
}

// Past is every session of p's, newest first.
func (Adapter) Past(p agent.Profile) []agent.Session {
	var out []agent.Session
	for _, path := range sessionFiles(p) {
		s, ok := pastSession(path)
		if !ok {
			continue
		}
		s.Kind, s.Profile, s.State = p.Kind, p, "done"
		out = append(out, s)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].UpdatedAt.After(out[j].UpdatedAt) })
	return out
}

// heads are session files' heads as last read, by path: a file is read
// again only once it has changed.
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

// pastSession reads what a list needs from a session file.
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

// entry is one line of a session file, with the fields of every type
// rush reads.
type entry struct {
	Type      string `json:"type"` // session, message, model_change, compaction, session_info, ...
	ID        string `json:"id"`
	ParentID  string `json:"parentId"`
	Timestamp string `json:"timestamp"`

	Cwd     string   `json:"cwd"`     // session
	Message *message `json:"message"` // message
	ModelID string   `json:"modelId"` // model_change
	Name    string   `json:"name"`    // session_info

	Summary      string `json:"summary"` // compaction, branch_summary
	TokensBefore int    `json:"tokensBefore"`
}

func (e *entry) time() time.Time {
	t, _ := time.Parse(time.RFC3339Nano, e.Timestamp)
	return t
}

// headLines is how many lines of a session Past reads looking for its
// first prompt; a name set later is looked for in the whole file.
const headLines = 200

var (
	nameMark = []byte(`"type":"session_info"`)
	userMark = []byte(`"role":"user"`)
)

// readHead reads a session's header, model and name: the one it was given,
// else its first prompt.
func readHead(path string, mod time.Time) (agent.Session, bool) {
	f, err := os.Open(path)
	if err != nil {
		return agent.Session{}, false
	}
	defer f.Close()
	s := agent.Session{ID: idOfFile(path), Transcript: path, UpdatedAt: mod}
	n, header, named := 0, false, false
	_ = readLines(f, func(b []byte) bool {
		n++
		// Only lines a head needs are decoded: most are messages, some big.
		wanted := n == 1 || bytes.Contains(b, nameMark) ||
			(n < headLines && (s.Model == "" && bytes.Contains(b, []byte(`"model_change"`)) ||
				s.Name == "" && bytes.Contains(b, userMark)))
		if !wanted {
			return true
		}
		var e entry
		if jsonx.Unmarshal(b, &e) != nil {
			return true
		}
		switch e.Type {
		case "session":
			header = true
			if e.ID != "" {
				s.ID = e.ID
			}
			s.Cwd, s.CreatedAt = e.Cwd, e.time()
		case "model_change":
			if s.Model == "" {
				s.Model = e.ModelID
			}
		case "session_info":
			if t := oneLine(e.Name); t != "" {
				s.Name, named = t, true
			}
		case "message":
			if m := e.Message; m != nil && m.Role == "user" && s.Name == "" && !named {
				s.Name = oneLine(m.text())
			}
		}
		return true
	})
	if !header {
		return agent.Session{}, false
	}
	if s.UpdatedAt.IsZero() {
		s.UpdatedAt = s.CreatedAt
	}
	return s, true
}

// maxFileLine is the longest session line read; a longer one ends the read.
const maxFileLine = 64 << 20

// readLines calls fn with each line of r until it says stop.
func readLines(r io.Reader, fn func([]byte) bool) error {
	br := bufio.NewReaderSize(r, 64<<10)
	for {
		b, err := br.ReadBytes('\n')
		if len(b) > maxFileLine {
			return bufio.ErrTooLong
		}
		if b = bytes.TrimSpace(b); len(b) > 0 && !fn(b) {
			return nil
		}
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
	}
}

// nameLen is how many characters of a prompt name a session.
const nameLen = 80

// oneLine is text on one line, cut to nameLen characters.
func oneLine(text string) string {
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

var _ agent.Discoverer = Adapter{}
