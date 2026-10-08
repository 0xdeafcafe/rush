package codex

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/pgguard"
)

// message is one line of the app-server protocol. It is JSON-RPC 2.0
// without the "jsonrpc" field: a request has ID and Method, a
// notification only Method, a response ID and Result or Error.
type message struct {
	ID     jsontext.Value `json:"id,omitzero"`
	Method string         `json:"method,omitempty"`
	Params jsontext.Value `json:"params,omitzero"`
	Result jsontext.Value `json:"result,omitzero"`
	Error  *rpcError      `json:"error,omitempty"`
}

type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func (e *rpcError) Error() string { return fmt.Sprintf("codex: %s (%d)", e.Message, e.Code) }

// errClosed is what a call gets once the app-server has gone.
var errClosed = errors.New("codex: app-server closed")

// client talks to one app-server over newline-delimited JSON. Handle is
// called in order, on the reading goroutine, for every notification and
// server request; it must not wait on a call.
type client struct {
	w      io.WriteCloser
	wmu    sync.Mutex
	handle func(*client, message)

	mu      sync.Mutex
	next    int64
	pending map[int64]chan message
	err     error

	done  chan struct{}
	cmd   *exec.Cmd
	guard *pgguard.Guard
}

func newClient(r io.Reader, w io.WriteCloser, handle func(*client, message)) *client {
	c := &client{w: w, handle: handle, pending: map[int64]chan message{}, done: make(chan struct{})}
	go c.read(r)
	return c
}

// spawn starts `codex app-server` in its own process group, with home as
// CODEX_HOME when it is set.
func spawn(binary, home string, env, flags []string, handle func(*client, message)) (*client, error) {
	if binary == "" {
		binary = "codex"
		if _, err := exec.LookPath(binary); err != nil {
			if p := agent.Path(Kind); p != "" {
				binary = p
			}
		}
	}
	cmd := exec.Command(binary, append([]string{"app-server"}, flags...)...)
	cmd.Env = append(os.Environ(), env...)
	if home != "" {
		cmd.Env = append(cmd.Env, "CODEX_HOME="+home)
	}
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	in, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	out, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	c := newClient(out, in, handle)
	c.cmd, c.guard = cmd, pgguard.Watch(cmd.Process.Pid)
	return c, nil
}

func (c *client) read(r io.Reader) {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 64*1024), 64*1024*1024)
	for sc.Scan() {
		var m message
		if err := jsonx.Unmarshal(sc.Bytes(), &m); err != nil {
			continue
		}
		if m.Method == "" && m.ID != nil {
			id, err := strconv.ParseInt(string(m.ID), 10, 64)
			c.mu.Lock()
			ch := c.pending[id]
			delete(c.pending, id)
			c.mu.Unlock()
			if err == nil && ch != nil {
				ch <- m
			}
			continue
		}
		if c.handle != nil {
			c.handle(c, m)
		}
	}
	err := sc.Err()
	if err == nil {
		err = errClosed
	}
	c.mu.Lock()
	c.err = err
	for id, ch := range c.pending {
		close(ch)
		delete(c.pending, id)
	}
	c.mu.Unlock()
	close(c.done)
}

func (c *client) write(m any) error {
	b, err := jsonx.Marshal(m)
	if err != nil {
		return err
	}
	c.wmu.Lock()
	defer c.wmu.Unlock()
	_, err = c.w.Write(append(b, '\n'))
	return err
}

// call sends a request and decodes its result into out, if out isn't nil.
func (c *client) call(ctx context.Context, method string, params, out any) error {
	ch := make(chan message, 1)
	c.mu.Lock()
	if c.err != nil {
		c.mu.Unlock()
		return errClosed
	}
	c.next++
	id := c.next
	c.pending[id] = ch
	c.mu.Unlock()
	req := struct {
		ID     int64  `json:"id"`
		Method string `json:"method"`
		Params any    `json:"params,omitempty"`
	}{id, method, params}
	if err := c.write(req); err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return err
	}
	select {
	case m, ok := <-ch:
		if !ok {
			return errClosed
		}
		if m.Error != nil {
			return m.Error
		}
		if out != nil && len(m.Result) > 0 {
			return jsonx.Unmarshal(m.Result, out)
		}
		return nil
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return ctx.Err()
	}
}

func (c *client) notify(method string) error {
	return c.write(struct {
		Method string `json:"method"`
	}{method})
}

// reply answers a server request.
func (c *client) reply(id jsontext.Value, result any) error {
	return c.write(struct {
		ID     jsontext.Value `json:"id"`
		Result any            `json:"result"`
	}{id, result})
}

// refuse answers a server request rush doesn't handle.
func (c *client) refuse(id jsontext.Value, msg string) error {
	return c.write(struct {
		ID    jsontext.Value `json:"id"`
		Error rpcError       `json:"error"`
	}{id, rpcError{Code: -32601, Message: msg}})
}

// initialize is the handshake every connection starts with. It returns
// the Codex version, read from the user agent it answers with.
func (c *client) initialize(ctx context.Context) (string, error) {
	var res struct {
		UserAgent string `json:"userAgent"`
	}
	// Experimental for thread/backgroundTerminals/terminate, how rush stops a background shell.
	params := map[string]any{
		"clientInfo":   map[string]any{"name": "rush", "title": "rush", "version": "0"},
		"capabilities": map[string]any{"experimentalApi": true, "requestAttestation": false},
	}
	if err := c.call(ctx, "initialize", params, &res); err != nil {
		return "", err
	}
	if err := c.notify("initialized"); err != nil {
		return "", err
	}
	return versionOf(res.UserAgent), nil
}

// versionOf is the version in a user agent such as
// "rush/0.155.1 (Mac OS 27.0.0; arm64) …": Codex puts its own version
// after the client's name.
func versionOf(ua string) string {
	f := strings.Fields(ua)
	if len(f) == 0 {
		return ""
	}
	_, v, _ := strings.Cut(f[0], "/")
	return v
}

// close ends the app-server: stdin first, then its process group.
func (c *client) close() error {
	_ = c.w.Close()
	if c.cmd == nil || c.cmd.Process == nil {
		return nil
	}
	pid := c.cmd.Process.Pid
	exited := make(chan struct{})
	go func() { _ = c.cmd.Wait(); c.guard.Release(); close(exited) }()
	select {
	case <-exited:
		return nil
	case <-time.After(time.Second):
	}
	_ = syscall.Kill(-pid, syscall.SIGTERM)
	select {
	case <-exited:
	case <-time.After(2 * time.Second):
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		<-exited
	}
	return nil
}

// idString is a request id as rush keys it.
func idString(id jsontext.Value) string {
	s := string(id)
	if u, err := strconv.Unquote(s); err == nil {
		return u
	}
	return s
}
