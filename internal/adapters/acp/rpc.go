package acp

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"
	"sync"

	"github.com/0xdeafcafe/photon/jsonx"
)

// JSON-RPC error codes ACP uses.
const (
	CodeMethodNotFound = -32601
	CodeInvalidParams  = -32602
	CodeInternal       = -32603
	CodeAuthRequired   = -32000
	CodeNotFound       = -32002
	CodeCancelled      = -32800
)

// Error is a JSON-RPC error, from the agent or for it.
type Error struct {
	Code    int            `json:"code"`
	Message string         `json:"message"`
	Data    jsontext.Value `json:"data,omitzero"`
}

func (e *Error) Error() string { return fmt.Sprintf("acp: %s (%d)", e.Message, e.Code) }

// ErrClosed is what calls return once the connection has ended.
var ErrClosed = errors.New("acp: connection closed")

// wire is one JSON-RPC message, whichever kind it is.
type wire struct {
	JSONRPC string         `json:"jsonrpc"`
	ID      jsontext.Value `json:"id,omitzero"`
	Method  string         `json:"method,omitempty"`
	Params  jsontext.Value `json:"params,omitzero"`
	Result  jsontext.Value `json:"result,omitzero"`
	Error   *Error         `json:"error,omitempty"`
}

// rpc is a JSON-RPC 2.0 connection of newline-delimited JSON, both ways:
// calls to the agent, and the agent's own calls and notifications, which
// it hands to notify and serve in the order they arrive.
type rpc struct {
	w   io.Writer
	wmu sync.Mutex

	mu      sync.Mutex
	next    int64
	pending map[int64]chan wire
	err     error
	done    chan struct{}

	notify func(method string, params jsontext.Value)
	serve  func(id jsontext.Value, method string, params jsontext.Value)
}

func newRPC(w io.Writer) *rpc {
	return &rpc{w: w, pending: map[int64]chan wire{}, done: make(chan struct{})}
}

// read reads r until it ends, then fails every call still waiting.
func (c *rpc) read(r io.Reader) {
	br := bufio.NewReaderSize(r, 64<<10)
	var err error
	for {
		var line []byte
		line, err = br.ReadBytes('\n')
		if line = bytes.TrimSpace(line); len(line) > 0 {
			c.dispatch(line)
		}
		if err != nil {
			break
		}
	}
	if err == io.EOF {
		err = ErrClosed
	}
	c.mu.Lock()
	c.err = err
	pending := c.pending
	c.pending = map[int64]chan wire{}
	c.mu.Unlock()
	for _, ch := range pending {
		close(ch)
	}
	close(c.done)
}

func (c *rpc) dispatch(line []byte) {
	var m wire
	if jsonx.Unmarshal(line, &m) != nil {
		return // not ours: some agents log to stdout
	}
	switch {
	case m.Method != "" && len(m.ID) > 0:
		if c.serve != nil {
			c.serve(m.ID, m.Method, m.Params)
		} else {
			_ = c.reply(m.ID, nil, &Error{Code: CodeMethodNotFound, Message: "method not found: " + m.Method})
		}
	case m.Method != "":
		if c.notify != nil {
			c.notify(m.Method, m.Params)
		}
	default:
		id, err := strconv.ParseInt(strings.Trim(string(m.ID), `"`), 10, 64)
		if err != nil {
			return
		}
		c.mu.Lock()
		ch := c.pending[id]
		delete(c.pending, id)
		c.mu.Unlock()
		if ch != nil {
			ch <- m
		}
	}
}

func (c *rpc) write(m wire) error {
	m.JSONRPC = "2.0"
	b, err := jsonx.Marshal(m)
	if err != nil {
		return err
	}
	c.wmu.Lock()
	defer c.wmu.Unlock()
	_, err = c.w.Write(append(b, '\n'))
	return err
}

func marshal(v any) (jsontext.Value, error) {
	if v == nil {
		return nil, nil
	}
	return jsonx.Marshal(v)
}

// call asks the agent method with params and decodes its result into
// out, which may be nil.
func (c *rpc) call(ctx context.Context, method string, params, out any) error {
	p, err := marshal(params)
	if err != nil {
		return err
	}
	ch := make(chan wire, 1)
	c.mu.Lock()
	if c.err != nil {
		c.mu.Unlock()
		return c.err
	}
	c.next++
	id := c.next
	c.pending[id] = ch
	c.mu.Unlock()
	if err := c.write(wire{ID: jsontext.Value(strconv.FormatInt(id, 10)), Method: method, Params: p}); err != nil {
		c.forget(id)
		return err
	}
	select {
	case m, ok := <-ch:
		if !ok {
			return c.closedErr()
		}
		if m.Error != nil {
			return m.Error
		}
		if out == nil || len(m.Result) == 0 {
			return nil
		}
		return jsonx.Unmarshal(m.Result, out)
	case <-ctx.Done():
		c.forget(id)
		_ = c.send("$/cancel_request", map[string]any{"requestId": id})
		return ctx.Err()
	}
}

func (c *rpc) forget(id int64) {
	c.mu.Lock()
	delete(c.pending, id)
	c.mu.Unlock()
}

func (c *rpc) closedErr() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.err != nil {
		return c.err
	}
	return ErrClosed
}

// send sends a notification.
func (c *rpc) send(method string, params any) error {
	p, err := marshal(params)
	if err != nil {
		return err
	}
	return c.write(wire{Method: method, Params: p})
}

// reply answers the agent's request id with result, or with e.
func (c *rpc) reply(id jsontext.Value, result any, e *Error) error {
	if e != nil {
		return c.write(wire{ID: id, Error: e})
	}
	if result == nil {
		result = struct{}{}
	}
	r, err := jsonx.Marshal(result)
	if err != nil {
		return err
	}
	return c.write(wire{ID: id, Result: r})
}
