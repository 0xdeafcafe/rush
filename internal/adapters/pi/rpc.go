package pi

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"maps"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/pgguard"
)

// response is Pi's answer to a command: its id, and data or an error.
type response struct {
	ID      string         `json:"id"`
	Type    string         `json:"type"`
	Command string         `json:"command"`
	Success bool           `json:"success"`
	Data    jsontext.Value `json:"data"`
	Error   string         `json:"error"`
}

// rpcError is a command Pi refused.
type rpcError struct{ Command, Message string }

func (e *rpcError) Error() string { return fmt.Sprintf("pi: %s: %s", e.Command, e.Message) }

// errClosed is what a call gets once pi has gone.
var errClosed = errors.New("pi: closed")

// client talks to one pi over JSON lines. Handle is called in order, on
// the reading goroutine, for every line that isn't an answer to a call; it
// must not wait on a call.
type client struct {
	w      io.WriteCloser
	wmu    sync.Mutex
	handle func(typ string, line []byte)

	mu      sync.Mutex
	next    int64
	pending map[string]chan response
	err     error

	done   chan struct{}
	cmd    *exec.Cmd
	guard  *pgguard.Guard
	stderr *tail
}

func newClient(r io.Reader, w io.WriteCloser, handle func(string, []byte)) *client {
	c := &client{w: w, handle: handle, pending: map[string]chan response{}, done: make(chan struct{})}
	go c.read(r)
	return c
}

// spawn starts cmd in its own process group and talks to it.
func spawn(cmd *exec.Cmd, handle func(string, []byte)) (*client, error) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	in, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	out, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	errs := &tail{}
	cmd.Stderr = errs
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	c := newClient(out, in, handle)
	c.cmd, c.guard, c.stderr = cmd, pgguard.Watch(cmd.Process.Pid), errs
	return c, nil
}

// maxLine is the longest line read: a tool result or a message with an
// image in it can be large.
const maxLine = 64 << 20

// read splits what pi writes on LF alone, as its protocol asks: a JSON
// string can hold U+2028, which other line readers split on too.
func (c *client) read(r io.Reader) {
	br := bufio.NewReaderSize(r, 64<<10)
	var err error
	for {
		var line []byte
		line, err = br.ReadBytes('\n')
		if len(line) > maxLine {
			err = bufio.ErrTooLong
			break
		}
		if b := bytes.TrimRight(line, "\r\n"); len(b) > 0 {
			c.line(b)
		}
		if err != nil {
			break
		}
	}
	if err == io.EOF || err == nil {
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

func (c *client) line(b []byte) {
	var head struct {
		Type string `json:"type"`
		ID   string `json:"id"`
	}
	if jsonx.Unmarshal(b, &head) != nil {
		return
	}
	if head.Type == "response" {
		var r response
		if jsonx.Unmarshal(b, &r) != nil || r.ID == "" {
			return // an answer to a command sent without an id
		}
		c.mu.Lock()
		ch := c.pending[r.ID]
		delete(c.pending, r.ID)
		c.mu.Unlock()
		if ch != nil {
			ch <- r
		}
		return
	}
	if c.handle != nil {
		c.handle(head.Type, b)
	}
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

// call sends a command, given as its fields less the id, and decodes its
// answer's data into out, if out isn't nil.
func (c *client) call(ctx context.Context, cmd map[string]any, out any) error {
	ch := make(chan response, 1)
	c.mu.Lock()
	if c.err != nil {
		c.mu.Unlock()
		return errClosed
	}
	c.next++
	id := "rush-" + strconv.FormatInt(c.next, 10)
	c.pending[id] = ch
	c.mu.Unlock()
	msg := make(map[string]any, len(cmd)+1)
	maps.Copy(msg, cmd)
	msg["id"] = id
	if err := c.write(msg); err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return err
	}
	select {
	case r, ok := <-ch:
		if !ok {
			return errClosed
		}
		if !r.Success {
			return &rpcError{Command: r.Command, Message: r.Error}
		}
		if out != nil && len(r.Data) > 0 && string(r.Data) != "null" {
			return jsonx.Unmarshal(r.Data, out)
		}
		return nil
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return ctx.Err()
	}
}

// post sends a command without waiting for its answer.
func (c *client) post(cmd map[string]any) error {
	c.mu.Lock()
	err := c.err
	c.mu.Unlock()
	if err != nil {
		return errClosed
	}
	return c.write(cmd)
}

// close ends pi: stdin first, then its process group.
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

// said is what pi last wrote to stderr, to say why it went.
func (c *client) said() string {
	if c.stderr == nil {
		return ""
	}
	return c.stderr.String()
}

// tail keeps the last few kilobytes written to it.
type tail struct {
	mu sync.Mutex
	b  []byte
}

const tailSize = 4 << 10

func (t *tail) Write(p []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.b = append(t.b, p...)
	if n := len(t.b); n > tailSize {
		t.b = append(t.b[:0:0], t.b[n-tailSize:]...)
	}
	return len(p), nil
}

func (t *tail) String() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return strings.TrimSpace(string(t.b))
}
