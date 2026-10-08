package claude

import (
	"bytes"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

type Convo = agent.Convo

// convoBufs are ReadConvo's read buffers: the list reads every transcript
// at start, and a fresh 192 KB each would be most of what it allocates.
var convoBufs = sync.Pool{New: func() any { b := make([]byte, 0, 128<<10); return &b }}

// ReadConvo reads a transcript's start, for where it ran and what was asked
// first, and its end, where Claude Code keeps appending its title. A
// transcript with nothing asked in it (a hook's, a probe's) is no
// conversation, and nor is a background job's (the jobs list has it, and
// each resume leaves an older transcript behind), one a program ran through
// the SDK (claude -p, evals, plugins' agents, rush's own sessions, which it
// lists itself) or a subagent's.
func ReadConvo(path string) (Convo, bool) {
	c := Convo{SessionID: strings.TrimSuffix(filepath.Base(path), ".jsonl")}
	f, err := os.Open(path)
	if err != nil {
		return c, false
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return c, false
	}
	bp := convoBufs.Get().(*[]byte)
	defer convoBufs.Put(bp)
	head := grown(bp, int(min(st.Size(), 64<<10)))
	if _, err := io.ReadFull(f, head); err != nil {
		return c, false
	}
	var first string
	sdk := false
	userLine := []byte(`"type":"user"`)
	for b := range lines(head) {
		// Once where and when are known, only a user's line, or one that
		// rules the transcript out, has more to say.
		if c.Cwd != "" && !c.Started.IsZero() && !bytes.Contains(b, userLine) && !rulesOut(b) {
			continue
		}
		var l struct {
			Type       string    `json:"type"`
			Cwd        string    `json:"cwd"`
			Timestamp  time.Time `json:"timestamp"`
			Entrypoint string    `json:"entrypoint"`
			Sidechain  bool      `json:"isSidechain"`
			Kind       string    `json:"sessionKind"`
		}
		if jsonx.Unmarshal(b, &l) != nil {
			continue
		}
		if l.Sidechain || l.Kind == "bg" || strings.HasPrefix(l.Entrypoint, "sdk") {
			sdk = true
			break
		}
		if c.Cwd == "" {
			c.Cwd = l.Cwd
		}
		if c.Started.IsZero() {
			c.Started = l.Timestamp
		}
		if first == "" && l.Type == "user" {
			for _, e := range recentEvents([][]byte{b}, 1) {
				if e.Role == "user" {
					first = e.Text
				}
			}
		}
		if c.Cwd != "" && !c.Started.IsZero() && first != "" {
			break
		}
	}
	if sdk || first == "" || c.Cwd == "" {
		return c, false
	}
	tail := head
	if off := st.Size() - 128<<10; off > 0 {
		tail = grown(bp, int(st.Size()-off))
		if _, err := f.ReadAt(tail, off); err != nil && err != io.EOF {
			return c, false
		}
	} else if int64(len(head)) < st.Size() {
		tail = grown(bp, int(st.Size()))
		if _, err := f.ReadAt(tail, 0); err != nil && err != io.EOF {
			return c, false
		}
	}
	var custom, ai string
	for b := range lines(tail) {
		if !bytes.HasPrefix(b, []byte(`{"type":"custom-title"`)) && !bytes.HasPrefix(b, []byte(`{"type":"ai-title"`)) {
			continue
		}
		var l struct {
			Custom string `json:"customTitle"`
			AI     string `json:"aiTitle"`
		}
		if jsonx.Unmarshal(b, &l) != nil {
			continue
		}
		if l.Custom != "" {
			custom = l.Custom
		}
		if l.AI != "" {
			ai = l.AI
		}
	}
	switch {
	case custom != "":
		c.Title = custom
	case ai != "":
		c.Title = ai
	default:
		c.Title = first
	}
	return c, true
}

// rulesOut is whether a line may mark a subagent's, a background job's or
// an SDK program's transcript, without decoding it.
func rulesOut(b []byte) bool {
	return bytes.Contains(b, []byte(`"isSidechain":true`)) || bytes.Contains(b, []byte(`"sessionKind":"bg"`)) ||
		bytes.Contains(b, []byte(`"entrypoint":"sdk`))
}

// grown is the pooled buffer at n bytes, grown when it's short.
func grown(bp *[]byte, n int) []byte {
	if cap(*bp) < n {
		*bp = make([]byte, n)
	}
	*bp = (*bp)[:n]
	return *bp
}

// lines yields b's lines without the newlines, as bytes.Split would, but
// without a slice of them all.
func lines(b []byte) func(func([]byte) bool) {
	return func(yield func([]byte) bool) {
		for {
			i := bytes.IndexByte(b, '\n')
			if i < 0 {
				yield(b)
				return
			}
			if !yield(b[:i]) {
				return
			}
			b = b[i+1:]
		}
	}
}
