package plugind

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"net"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// fakeQueueHost is a session's host holding a queue, for its queue ops:
// it sends its info on connect and after each op, as a host does.
func fakeQueueHost(t *testing.T, info host.Info) (sent chan string) {
	t.Helper()
	writeInfo(t, info)
	ln, err := net.Listen("unix", host.SockPath(info.ID))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	sent = make(chan string, 4)
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				defer c.Close()
				say := func() {
					b, _ := jsonx.Marshal(map[string]any{"type": "agtop_info", "info": info})
					_, _ = c.Write(append(b, '\n'))
				}
				say()
				sc := bufio.NewScanner(c)
				for sc.Scan() {
					var o struct {
						Op    string `json:"op"`
						Index int    `json:"index"`
						Was   string `json:"was"`
					}
					_ = jsonx.Unmarshal(sc.Bytes(), &o)
					i := slices.Index(info.Queue, o.Was)
					switch {
					case o.Op == "hello":
						continue
					case i < 0:
						b, _ := jsonx.Marshal(map[string]any{"type": "agtop_error", "error": "that message has already been sent"})
						_, _ = c.Write(append(b, '\n'))
						continue
					case o.Op == "queue_send":
						sent <- info.Queue[i]
					}
					info.Queue = slices.Delete(slices.Clone(info.Queue), i, i+1)
					say()
				}
			}()
		}
	}()
	return sent
}

// With queued, a plugin sends or drops a queued message by its place, in
// its workspaces only, and never sees the text.
func TestQueuedSendAndRemove(t *testing.T) {
	// Short, because unix socket paths are capped near 104 bytes on macOS.
	home, err := os.MkdirTemp("/tmp", "agq-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(home) })
	home, _ = filepath.EvalSymlinks(home)
	t.Setenv("RUSH_HOME", home)
	ws := filepath.Join(home, "ws")
	_ = os.MkdirAll(ws, 0o700)
	m := plugin.Manifest{Name: "queue", Sessions: []string{plugin.CapQueued}, Workspaces: []string{ws}}
	r := newRunner(nil, m.Name, "")
	r.p = plugin.Plugin{Manifest: m}
	sent := fakeQueueHost(t, host.Info{ID: "q1", Cwd: ws, State: "working", Queue: []string{"first", "second", "third"}})
	writeInfo(t, host.Info{ID: "out1", Cwd: home, State: "working", Queue: []string{"x"}})

	call := func(method, params string) (any, error) {
		return r.fromPlugin(context.Background(), method, jsontext.Value(params))
	}
	res, err := call("sessions.queued.send", `{"id": "q1", "index": 1}`)
	if err != nil {
		t.Fatal(err)
	}
	if got := <-sent; got != "second" {
		t.Fatalf("sent %q", got)
	}
	if b, _ := jsonx.Marshal(res); string(b) != "{}" {
		t.Fatalf("the plugin was told %s", b)
	}
	// Naming one by its text would let a plugin test guesses at the queue:
	// only rush's own plugins may, and then was names the one meant even
	// if the queue moved.
	if _, err := call("sessions.queued.remove", `{"id": "q1", "index": 0, "was": "third"}`); !denied(err) {
		t.Fatalf("an installed plugin named a message by its text: %v", err)
	}
	r.p.Bundled = true
	if _, err := call("sessions.queued.remove", `{"id": "q1", "index": 0, "was": "third"}`); err != nil {
		t.Fatal(err)
	}
	r.p.Bundled = false
	if _, err := call("sessions.queued.remove", `{"id": "q1", "index": 5}`); err == nil {
		t.Fatal("no message 5, it should say so")
	}
	if _, err := call("sessions.queued.send", `{"id": "out1", "index": 0}`); !denied(err) {
		t.Fatalf("outside its workspaces: %v", err)
	}
	r.p.Sessions = nil
	if _, err := call("sessions.queued.send", `{"id": "q1", "index": 0}`); !denied(err) {
		t.Fatalf("without the capability: %v", err)
	}
}
