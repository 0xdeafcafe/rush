package plugin

import (
	"bufio"
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"strconv"
	"sync"
	"sync/atomic"

	"github.com/0xdeafcafe/photon/jsonx"
)

// MaxFrame bounds one message. A tool result bigger than this is a bug, and
// refusing it keeps a plugin from making rush allocate without limit.
const MaxFrame = 16 << 20

// Handler answers a request, or takes a notification (whose result is
// dropped). Returning an *Error sends it as is; any other error is sent as a
// generic server error.
type Handler func(ctx context.Context, method string, params jsontext.Value) (any, error)

// Error is a JSON-RPC error.
type Error struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func (e *Error) Error() string { return e.Message }

// JSON-RPC error codes.
const (
	CodeParse          = -32700
	CodeInvalidRequest = -32600
	CodeNoMethod       = -32601
	CodeInvalidParams  = -32602
	CodeServer         = -32000
	CodeDenied         = -32001 // the plugin lacks the capability
	CodeLimited        = -32002 // allowed, but not so often: try again later
)

// Limited is the error for a call made too often: it may go again later.
func Limited(what string) *Error {
	return &Error{Code: CodeLimited, Message: "too often: " + what}
}

// Denied is the error for a call the plugin was not approved to make.
func Denied(what string) *Error {
	return &Error{Code: CodeDenied, Message: "not permitted: " + what}
}

type message struct {
	JSONRPC string         `json:"jsonrpc"`
	ID      jsontext.Value `json:"id,omitzero"`
	Method  string         `json:"method,omitempty"`
	Params  jsontext.Value `json:"params,omitzero"`
	Result  jsontext.Value `json:"result,omitzero"`
	Error   *Error         `json:"error,omitempty"`
}

// codec reads and writes whole messages.
type codec interface {
	read() ([]byte, error)
	write([]byte) error
}

// framed is a 4-byte big-endian length, then that many bytes of JSON. It is
// what rush's own plugins speak on fd 3: a message is read in two reads
// with no scanning, whatever it holds.
type framed struct {
	r *bufio.Reader
	w io.Writer
}

func (f *framed) read() ([]byte, error) {
	var hdr [4]byte
	if _, err := io.ReadFull(f.r, hdr[:]); err != nil {
		return nil, err
	}
	n := binary.BigEndian.Uint32(hdr[:])
	if n > MaxFrame {
		return nil, fmt.Errorf("frame of %d bytes is over the %d limit", n, MaxFrame)
	}
	b := make([]byte, n)
	_, err := io.ReadFull(f.r, b)
	return b, err
}

func (f *framed) write(b []byte) error {
	if len(b) > MaxFrame {
		return fmt.Errorf("frame of %d bytes is over the %d limit", len(b), MaxFrame)
	}
	buf := make([]byte, 4+len(b))
	binary.BigEndian.PutUint32(buf, uint32(len(b)))
	copy(buf[4:], b)
	_, err := f.w.Write(buf)
	return err
}

// lines is one JSON message per line: MCP's stdio transport, so any MCP
// server can be a plugin as it is.
type lines struct {
	r *bufio.Reader
	w io.Writer
}

func (l *lines) read() ([]byte, error) {
	var line []byte
	for {
		b, err := l.r.ReadSlice('\n')
		if len(line)+len(b) > MaxFrame {
			return nil, fmt.Errorf("line over the %d byte limit", MaxFrame)
		}
		line = append(line, b...)
		if errors.Is(err, bufio.ErrBufferFull) {
			continue
		}
		if err != nil {
			return nil, err
		}
		if len(bytes.TrimSpace(line)) > 0 {
			return line, nil
		}
		line = line[:0]
	}
}

func (l *lines) write(b []byte) error {
	_, err := l.w.Write(append(b, '\n'))
	return err
}

// Conn is a JSON-RPC 2.0 connection. Either side may call the other; calls
// run concurrently, and replies are matched to calls by id.
type Conn struct {
	c       codec
	closer  io.Closer
	handler Handler

	wmu     sync.Mutex
	next    atomic.Int64
	mu      sync.Mutex
	pending map[string]chan *message
	closed  bool // under mu: no more replies will come
	done    chan struct{}
	err     error
	ctx     context.Context
	cancel  context.CancelFunc
}

// NewConn speaks framed JSON-RPC over rw. h may be nil: every request then
// gets "method not found".
func NewConn(rw io.ReadWriteCloser, h Handler) *Conn {
	return newConn(&framed{r: bufio.NewReaderSize(rw, 64<<10), w: rw}, rw, h)
}

// NewLineConn speaks newline-delimited JSON-RPC (MCP's stdio transport),
// reading r and writing w; closing it closes both.
func NewLineConn(r io.ReadCloser, w io.WriteCloser, h Handler) *Conn {
	return newConn(&lines{r: bufio.NewReaderSize(r, 64<<10), w: w}, closers{r, w}, h)
}

type closers []io.Closer

func (cs closers) Close() error {
	var first error
	for _, c := range cs {
		if err := c.Close(); err != nil && first == nil {
			first = err
		}
	}
	return first
}

func newConn(c codec, closer io.Closer, h Handler) *Conn {
	ctx, cancel := context.WithCancel(context.Background())
	conn := &Conn{c: c, closer: closer, handler: h, pending: map[string]chan *message{},
		done: make(chan struct{}), ctx: ctx, cancel: cancel}
	go conn.readLoop()
	return conn
}

// Done closes when the connection has.
func (c *Conn) Done() <-chan struct{} { return c.done }

// Err says why the connection closed.
func (c *Conn) Err() error {
	<-c.done
	return c.err
}

// Close ends the connection; calls waiting on it fail.
func (c *Conn) Close() error { return c.closer.Close() }

func (c *Conn) readLoop() {
	defer func() {
		c.cancel()
		c.mu.Lock()
		c.closed = true
		for id, ch := range c.pending {
			close(ch)
			delete(c.pending, id)
		}
		c.mu.Unlock()
		close(c.done)
	}()
	for {
		b, err := c.c.read()
		if err != nil {
			c.err = err
			_ = c.closer.Close()
			return
		}
		var m message
		if err := jsonx.Unmarshal(b, &m); err != nil {
			_ = c.send(&message{Error: &Error{Code: CodeParse, Message: "parse error"}})
			continue
		}
		switch {
		case m.Method != "" && len(m.ID) > 0:
			go c.serve(&m)
		case m.Method != "":
			// Notifications are taken in order, so a stream of events
			// arrives as it was sent.
			if c.handler != nil {
				_, _ = c.handler(c.ctx, m.Method, m.Params)
			}
		case len(m.ID) > 0:
			c.mu.Lock()
			ch := c.pending[string(m.ID)]
			delete(c.pending, string(m.ID))
			c.mu.Unlock()
			if ch != nil {
				ch <- &m
			}
		}
	}
}

func (c *Conn) serve(m *message) {
	out := &message{ID: m.ID}
	if c.handler == nil {
		out.Error = &Error{Code: CodeNoMethod, Message: "method not found: " + m.Method}
	} else if res, err := c.handler(c.ctx, m.Method, m.Params); err != nil {
		var e *Error
		if errors.As(err, &e) {
			out.Error = e
		} else {
			out.Error = &Error{Code: CodeServer, Message: err.Error()}
		}
	} else if raw, ok := res.(jsontext.Value); ok {
		out.Result = raw
	} else if b, err := jsonx.Marshal(res); err != nil {
		out.Error = &Error{Code: CodeServer, Message: err.Error()}
	} else {
		out.Result = b
	}
	if out.Error == nil && out.Result == nil {
		out.Result = jsontext.Value("null")
	}
	_ = c.send(out)
}

func (c *Conn) send(m *message) error {
	m.JSONRPC = "2.0"
	b, err := jsonx.Marshal(m)
	if err != nil {
		return err
	}
	c.wmu.Lock()
	defer c.wmu.Unlock()
	return c.c.write(b)
}

// ErrClosed is returned by a call whose connection closed before it was
// answered.
var ErrClosed = errors.New("connection closed")

// CallRaw calls method with params already encoded, and returns the raw
// result.
func (c *Conn) CallRaw(ctx context.Context, method string, params jsontext.Value) (jsontext.Value, error) {
	id := jsontext.Value(strconv.FormatInt(c.next.Add(1), 10))
	ch := make(chan *message, 1)
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil, ErrClosed
	}
	c.pending[string(id)] = ch
	c.mu.Unlock()
	forget := func() {
		c.mu.Lock()
		delete(c.pending, string(id))
		c.mu.Unlock()
	}
	if err := c.send(&message{ID: id, Method: method, Params: params}); err != nil {
		forget()
		return nil, err
	}
	select {
	case m, ok := <-ch:
		if !ok {
			return nil, ErrClosed
		}
		if m.Error != nil {
			return nil, m.Error
		}
		return m.Result, nil
	case <-ctx.Done():
		forget()
		return nil, ctx.Err()
	}
}

// Call calls method with params and decodes the result into out, which may
// be nil to ignore it.
func (c *Conn) Call(ctx context.Context, method string, params, out any) error {
	p, err := marshalParams(params)
	if err != nil {
		return err
	}
	res, err := c.CallRaw(ctx, method, p)
	if err != nil || out == nil {
		return err
	}
	return jsonx.Unmarshal(res, out)
}

// Notify sends a notification, which has no reply.
func (c *Conn) Notify(method string, params any) error {
	p, err := marshalParams(params)
	if err != nil {
		return err
	}
	return c.send(&message{Method: method, Params: p})
}

func marshalParams(params any) (jsontext.Value, error) {
	switch p := params.(type) {
	case nil:
		return nil, nil
	case jsontext.Value:
		return p, nil
	}
	return jsonx.Marshal(params)
}
