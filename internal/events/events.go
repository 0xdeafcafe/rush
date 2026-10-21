// Package events is rush's log of what it does and why, to debug by: a
// session restarted, a message queued, a check-in sent, a command a hook
// rewrote, an account moved off at a usage limit. Each is a line of JSON in
// rush's events.jsonl, and a session's are in its own folder's too.
//
// Log never waits: lines go to a writer of their own, so the UI can log
// from its goroutine. A short-lived process (a hook, a command) calls Flush
// before it exits.
package events

import (
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/netwatch"
	"github.com/0xdeafcafe/rush/internal/state"
)

// maxSize is how big a log grows before it moves to its .1, the one
// before going.
const maxSize = 16 << 20

// Path is rush's own log, of every process and session.
func Path() string { return filepath.Join(state.Dir(), "events.jsonl") }

type line struct {
	paths []string
	b     []byte
	done  chan struct{} // closed once written, when it's waited for
}

var (
	once    sync.Once
	queue   chan line
	pending sync.WaitGroup
	proc    = "rush" // which of rush's processes this is: ui, host, hook, …
	procMu  sync.Mutex
)

func init() {
	netwatch.OnEvent = Log
	// Settings and other state rush saves: which file, from which process.
	state.OnSave = func(path string) {
		if rel, err := filepath.Rel(state.Dir(), path); err == nil {
			path = rel
		}
		Log("saved", map[string]any{"file": path})
	}
}

// SetProcess names this process in its lines: ui, host, hook, cli.
func SetProcess(name string) {
	procMu.Lock()
	proc = name
	procMu.Unlock()
}

// Log records an event of kind, with fields, in rush's log.
func Log(kind string, fields map[string]any) { write(kind, "", "", fields) }

// Session records an event of a session's, in its own log at path and in
// rush's, where it says which session.
func Session(id, path, kind string, fields map[string]any) { write(kind, id, path, fields) }

func write(kind, session, path string, fields map[string]any) {
	m := make(map[string]any, len(fields)+5)
	for k, v := range fields {
		m[k] = v
	}
	procMu.Lock()
	m["proc"] = proc
	procMu.Unlock()
	m["t"] = time.Now().UTC().Format(time.RFC3339Nano)
	m["kind"] = kind
	m["pid"] = os.Getpid()
	if session != "" {
		m["session"] = session
	}
	b, err := jsonx.Marshal(m)
	if err != nil {
		return
	}
	l := line{b: append(b, '\n')}
	if !testing.Testing() || os.Getenv("RUSH_HOME") != "" {
		// A test's go to its own rush folder, never the machine's.
		l.paths = append(l.paths, Path())
	}
	if path != "" {
		l.paths = append(l.paths, path)
	}
	if len(l.paths) == 0 {
		return
	}
	once.Do(start)
	if testing.Testing() {
		// Written before it returns: a test's folder is removed when it
		// ends, and a line written into it after would fail the test.
		l.done = make(chan struct{})
		pending.Add(1)
		queue <- l
		<-l.done
		return
	}
	pending.Add(1)
	select {
	case queue <- l:
	default:
		pending.Done() // the writer is that far behind: this one goes
	}
}

func start() {
	queue = make(chan line, 4096)
	go func() {
		for l := range queue {
			for _, p := range l.paths {
				appendLine(p, l.b)
			}
			pending.Done()
			if l.done != nil {
				close(l.done)
			}
		}
	}()
}

// appendLine adds b to the log at p. A line appended whole never
// interleaves with another process's.
func appendLine(p string, b []byte) {
	if st, err := os.Stat(p); err == nil && st.Size() > maxSize {
		_ = os.Rename(p, p[:len(p)-len(".jsonl")]+".1.jsonl")
	}
	f, err := os.OpenFile(p, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return // its folder is gone, or never was: nothing to keep it in
	}
	_, _ = f.Write(b)
	_ = f.Close()
}

// Flush waits, up to a second, for what's been logged to be written.
func Flush() {
	done := make(chan struct{})
	go func() { pending.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(time.Second):
	}
}
