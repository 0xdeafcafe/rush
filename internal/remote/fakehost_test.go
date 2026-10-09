package remote

import (
	"net"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/host"
)

// shortHome makes this process's RUSH_HOME a folder of the test's own (under
// $TMPDIR, cleaned up with it), reached by a relative path: a session's unix
// socket path must fit in 104 bytes, which an absolute one in a deep
// scratch folder doesn't, and a relative one does. The process is in the
// folder for the test; a subprocess is run with it as its Dir and
// RUSH_HOME=".".
func shortHome(t *testing.T) string {
	t.Helper()
	d := t.TempDir()
	old, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(d); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chdir(old) })
	t.Setenv("RUSH_HOME", ".")
	return d
}

// listenIn listens on dir's host.sock by a relative path from inside dir.
func listenIn(t *testing.T, dir string) net.Listener {
	t.Helper()
	old, _ := os.Getwd()
	if err := os.Chdir(dir); err != nil {
		t.Fatal(err)
	}
	defer os.Chdir(old)
	ln, err := net.Listen("unix", "host.sock")
	if err != nil {
		t.Fatal(err)
	}
	return ln
}

// fakeHost stands in for a session's host: it is alive (this process),
// answers on the session's socket with a fixed replay then its info, and
// records every op line it is sent, so a test sees what reached it.
type fakeHost struct {
	id   string
	info host.Info
	ops  chan []byte
	ln   net.Listener
	mu   sync.Mutex
	conn []net.Conn
}

// startFakeHost makes session id's folder under root (a RUSH_HOME's
// sessions folder) and serves it.
func startFakeHost(t *testing.T, root, id, name, cwd string, replay ...[]byte) *fakeHost {
	t.Helper()
	dir := filepath.Join(root, id)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	f := &fakeHost{id: id, ops: make(chan []byte, 64)}
	f.info = host.Info{ID: id, SessionID: id + "-0000", Name: name, Cwd: cwd, HostPID: os.Getpid(), State: "idle",
		Proto: host.Proto, StartedAt: time.Now(), UpdatedAt: time.Now()}
	f.writeInfo(t, dir)
	ln := listenIn(t, dir)
	f.ln = ln
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			f.mu.Lock()
			f.conn = append(f.conn, c)
			f.mu.Unlock()
			for _, l := range replay {
				c.Write(append(append([]byte(nil), l...), '\n'))
			}
			f.sendInfoTo(c)
			go func() {
				r := jsonx.NewLineReader(c)
				for {
					l, ok := r.Next()
					if !ok {
						return
					}
					if host.OpName(l) != "hello" {
						f.ops <- append([]byte(nil), l...)
					}
				}
			}()
		}
	}()
	return f
}

func (f *fakeHost) writeInfo(t *testing.T, dir string) {
	b, _ := jsonx.Marshal(f.info)
	if err := os.WriteFile(filepath.Join(dir, "info.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
}

func (f *fakeHost) sendInfoTo(c net.Conn) {
	b, _ := jsonx.Marshal(f.info)
	c.Write(append(append([]byte(`{"info":`), b...), []byte(`,"type":"agtop_info"}`+"\n")...))
}

// push sends a line to every connected client.
func (f *fakeHost) push(line []byte) {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, c := range f.conn {
		c.Write(append(append([]byte(nil), line...), '\n'))
	}
}

// op waits for the next op line the host was sent.
func (f *fakeHost) op(t *testing.T) map[string]any {
	t.Helper()
	select {
	case l := <-f.ops:
		var m map[string]any
		if err := jsonx.Unmarshal(l, &m); err != nil {
			t.Fatal(err)
		}
		return m
	case <-time.After(5 * time.Second):
		t.Fatal("no op reached the host")
		return nil
	}
}

// noOp is that no op reaches the host for a moment.
func (f *fakeHost) noOp(t *testing.T) {
	t.Helper()
	select {
	case l := <-f.ops:
		t.Fatalf("an op reached the host: %s", l)
	case <-time.After(400 * time.Millisecond):
	}
}

func evLine(t *testing.T, ev event.Event) []byte {
	t.Helper()
	b, err := event.Marshal(ev)
	if err != nil {
		t.Fatal(err)
	}
	return append(append([]byte(`{"type":"agtop_ev","ev":`), b...), '}')
}

func approvalLine(t *testing.T, id string) []byte {
	return evLine(t, event.Approval{ID: id, Call: tool.Call{ID: "c1", Name: "Bash", Input: tool.Input{Command: "rm -rf build"}},
		Options: []event.Option{{ID: "allow", Label: "Allow", Kind: event.AllowOnce}}})
}

func questionLine(t *testing.T, id string) []byte {
	return evLine(t, event.Question{ID: id, Title: "Which?", Asks: []event.Ask{{Text: "Pick one", Options: []event.Choice{{Label: "A"}, {Label: "B"}}}}})
}
