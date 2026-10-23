package ts

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// A Server is one language server process for one checkout.
type Server struct {
	Root   string
	Engine Engine

	cmd  *exec.Cmd
	in   io.WriteCloser
	run  *gate.Run // its gate ticket; nil when the gate is off
	caps map[string]any

	wmu      sync.Mutex // one message written at a time
	mu       sync.Mutex
	nextID   int
	pending  map[int]chan reply
	opened   map[string]openFile // by URI
	diags    map[string]jsontext.Value
	diagWait map[string][]chan struct{}
	closed   bool
	why      error // why it stopped, for the calls it fails
	lastUsed time.Time
	done     chan struct{}
}

type openFile struct {
	mtime   time.Time
	version int
}

type reply struct {
	result jsontext.Value
	err    error
}

// rpcError is a response's error, as LSP sends it.
type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// message is any LSP message, in or out.
type message struct {
	JSONRPC string         `json:"jsonrpc"`
	ID      jsontext.Value `json:"id,omitzero"`
	Method  string         `json:"method,omitzero"`
	Params  jsontext.Value `json:"params,omitzero"`
	Result  jsontext.Value `json:"result,omitzero"`
	Error   *rpcError      `json:"error,omitzero"`
}

// Start runs e for the checkout at root and says hello to it. run, if
// set, is the gate ticket it holds while it lives.
func Start(ctx context.Context, root string, e Engine, run *gate.Run) (*Server, error) {
	cmd := exec.Command(e.Bin, "--lsp", "--stdio")
	cmd.Dir = root
	cmd.Env = os.Environ()
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
	s := &Server{Root: root, Engine: e, cmd: cmd, in: in, run: run,
		pending: map[int]chan reply{}, opened: map[string]openFile{}, diags: map[string]jsontext.Value{},
		diagWait: map[string][]chan struct{}{}, lastUsed: time.Now(), done: make(chan struct{})}
	go s.read(bufio.NewReaderSize(out, 1<<16))
	go func() {
		err := cmd.Wait()
		s.stop(fmt.Errorf("the %s language server for %s exited: %v", e.Name, root, err))
	}()
	rootURI := fileURI(root)
	var init struct {
		Capabilities map[string]any `json:"capabilities"`
	}
	err = s.Call(ctx, "initialize", map[string]any{
		"processId": os.Getpid(),
		"rootUri":   rootURI,
		"capabilities": map[string]any{
			"textDocument": map[string]any{
				"hover":              map[string]any{"contentFormat": []string{"markdown", "plaintext"}},
				"documentSymbol":     map[string]any{"hierarchicalDocumentSymbolSupport": true},
				"definition":         map[string]any{"linkSupport": false},
				"implementation":     map[string]any{"linkSupport": false},
				"references":         map[string]any{},
				"callHierarchy":      map[string]any{},
				"publishDiagnostics": map[string]any{},
				"diagnostic":         map[string]any{},
			},
			"workspace": map[string]any{"symbol": map[string]any{}, "workspaceFolders": true, "configuration": true},
		},
		"workspaceFolders": []map[string]any{{"uri": rootURI, "name": filepath.Base(root)}},
	}, &init)
	if err != nil {
		s.Kill(err)
		return nil, err
	}
	s.caps = init.Capabilities
	s.notify("initialized", map[string]any{})
	return s, nil
}

// Done is closed once the server has stopped.
func (s *Server) Done() <-chan struct{} { return s.done }

// Why is why the server stopped; nil while it runs.
func (s *Server) Why() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.why
}

// Idle is how long since the server was last asked anything.
func (s *Server) Idle() time.Duration {
	s.mu.Lock()
	defer s.mu.Unlock()
	return time.Since(s.lastUsed)
}

// PID is the server process's.
func (s *Server) PID() int { return s.cmd.Process.Pid }

// Kill stops the server at once, failing what's in flight with why.
func (s *Server) Kill(why error) {
	s.stop(why)
	_ = syscall.Kill(-s.cmd.Process.Pid, syscall.SIGKILL)
}

// Close asks the server to go, then makes it.
func (s *Server) Close() {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_ = s.Call(ctx, "shutdown", nil, nil)
	s.notify("exit", nil)
	select {
	case <-s.done:
	case <-time.After(2 * time.Second):
	}
	s.Kill(errors.New("closed"))
}

func (s *Server) stop(why error) {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return
	}
	s.closed, s.why = true, why
	pending := s.pending
	s.pending = map[int]chan reply{}
	for _, ws := range s.diagWait {
		for _, w := range ws {
			close(w)
		}
	}
	s.diagWait = map[string][]chan struct{}{}
	s.mu.Unlock()
	for _, ch := range pending {
		ch <- reply{err: why}
	}
	if s.run != nil {
		s.run.Release()
	}
	close(s.done)
}

// Call sends a request and decodes its result into out, if set.
func (s *Server) Call(ctx context.Context, method string, params, out any) error {
	s.mu.Lock()
	if s.closed {
		why := s.why
		s.mu.Unlock()
		return why
	}
	s.nextID++
	id := s.nextID
	ch := make(chan reply, 1)
	s.pending[id] = ch
	s.lastUsed = time.Now()
	s.mu.Unlock()
	if err := s.send(message{ID: jsontext.Value(strconv.Itoa(id)), Method: method, Params: raw(params)}); err != nil {
		return err
	}
	select {
	case r := <-ch:
		if r.err != nil {
			return r.err
		}
		if out != nil && len(r.result) > 0 {
			return jsonx.Unmarshal(r.result, out)
		}
		return nil
	case <-ctx.Done():
		s.mu.Lock()
		delete(s.pending, id)
		s.mu.Unlock()
		s.notify("$/cancelRequest", map[string]any{"id": id})
		return fmt.Errorf("%s: %w", method, ctx.Err())
	}
}

func (s *Server) notify(method string, params any) {
	_ = s.send(message{Method: method, Params: raw(params)})
}

func raw(v any) jsontext.Value {
	if v == nil {
		return jsontext.Value("null")
	}
	b, err := jsonx.Marshal(v)
	if err != nil {
		return jsontext.Value("null")
	}
	return b
}

func (s *Server) send(m message) error {
	m.JSONRPC = "2.0"
	body, err := jsonx.Marshal(m)
	if err != nil {
		return err
	}
	s.wmu.Lock()
	defer s.wmu.Unlock()
	if _, err := fmt.Fprintf(s.in, "Content-Length: %d\r\n\r\n", len(body)); err != nil {
		return err
	}
	_, err = s.in.Write(body)
	return err
}

// read takes the server's messages until it closes: replies to calls,
// diagnostics it pushes, and requests it makes of us, which get the
// answer that asks for nothing.
func (s *Server) read(r *bufio.Reader) {
	for {
		n := -1
		for {
			line, err := r.ReadString('\n')
			if err != nil {
				return
			}
			line = strings.TrimSpace(line)
			if line == "" {
				break
			}
			if k, v, ok := strings.Cut(line, ":"); ok && strings.EqualFold(strings.TrimSpace(k), "Content-Length") {
				n, _ = strconv.Atoi(strings.TrimSpace(v))
			}
		}
		if n < 0 {
			continue
		}
		body := make([]byte, n)
		if _, err := io.ReadFull(r, body); err != nil {
			return
		}
		var m message
		if jsonx.Unmarshal(body, &m) != nil {
			continue
		}
		switch {
		case m.Method != "" && len(m.ID) > 0:
			s.answer(m)
		case m.Method == "textDocument/publishDiagnostics":
			var p struct {
				URI         string         `json:"uri"`
				Diagnostics jsontext.Value `json:"diagnostics"`
			}
			if jsonx.Unmarshal(m.Params, &p) == nil {
				s.mu.Lock()
				s.diags[p.URI] = p.Diagnostics
				for _, w := range s.diagWait[p.URI] {
					close(w)
				}
				delete(s.diagWait, p.URI)
				s.mu.Unlock()
			}
		case m.Method == "" && len(m.ID) > 0:
			id, _ := strconv.Atoi(string(m.ID))
			s.mu.Lock()
			ch := s.pending[id]
			delete(s.pending, id)
			s.mu.Unlock()
			if ch == nil {
				continue
			}
			if m.Error != nil {
				ch <- reply{err: fmt.Errorf("%s: %s", s.Engine.Name, m.Error.Message)}
			} else {
				ch <- reply{result: m.Result}
			}
		}
	}
}

// answer replies to the server's own requests: a null for each setting it
// asks for, and nothing for the rest (progress, registrations).
func (s *Server) answer(m message) {
	result := jsontext.Value("null")
	if m.Method == "workspace/configuration" {
		var p struct {
			Items []jsontext.Value `json:"items"`
		}
		_ = jsonx.Unmarshal(m.Params, &p)
		nulls := make([]any, len(p.Items))
		result = raw(nulls)
	}
	_ = s.send(message{ID: m.ID, Result: result})
}

// Open makes sure the server has file as it is on disk now: opened, or
// sent again when it changed since. It is the URI to ask about.
func (s *Server) Open(file string) (string, error) {
	st, err := os.Stat(file)
	if err != nil {
		return "", err
	}
	uri := fileURI(file)
	s.mu.Lock()
	was, ok := s.opened[uri]
	s.mu.Unlock()
	if ok && was.mtime.Equal(st.ModTime()) {
		return uri, nil
	}
	text, err := os.ReadFile(file)
	if err != nil {
		return "", err
	}
	s.mu.Lock()
	version := was.version + 1
	s.opened[uri] = openFile{mtime: st.ModTime(), version: version}
	delete(s.diags, uri)
	s.mu.Unlock()
	if !ok {
		s.notify("textDocument/didOpen", map[string]any{"textDocument": map[string]any{
			"uri": uri, "languageId": languageOf(file), "version": version, "text": string(text)}})
	} else {
		s.notify("textDocument/didChange", map[string]any{"textDocument": map[string]any{"uri": uri, "version": version},
			"contentChanges": []map[string]any{{"text": string(text)}}})
	}
	return uri, nil
}

// Diagnostics is what the server says is wrong in uri: asked for where it
// answers that, else the last it pushed, waiting up to wait for one.
func (s *Server) Diagnostics(ctx context.Context, uri string, wait time.Duration) (jsontext.Value, error) {
	if _, ok := s.caps["diagnosticProvider"]; ok {
		var r struct {
			Items jsontext.Value `json:"items"`
		}
		if err := s.Call(ctx, "textDocument/diagnostic", map[string]any{"textDocument": map[string]any{"uri": uri}}, &r); err == nil {
			return r.Items, nil
		}
	}
	s.mu.Lock()
	if d, ok := s.diags[uri]; ok {
		s.mu.Unlock()
		return d, nil
	}
	w := make(chan struct{})
	s.diagWait[uri] = append(s.diagWait[uri], w)
	s.mu.Unlock()
	select {
	case <-w:
	case <-time.After(wait):
	case <-ctx.Done():
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.diags[uri], nil
}

// Footprint is what the server's processes hold now, as Activity
// Monitor counts it.
func (s *Server) Footprint() uint64 {
	tab := proc.Snapshot(nil)
	tab.Fill(nil, []int{s.PID()})
	var n uint64
	for _, pid := range tab.Descendants(s.PID()) {
		if p := tab.Procs[pid]; p != nil {
			n += p.Footprint
		}
	}
	return n
}

func fileURI(path string) string {
	return (&url.URL{Scheme: "file", Path: filepath.ToSlash(path)}).String()
}

// pathOf is the file a file: URI names.
func pathOf(uri string) string {
	u, err := url.Parse(uri)
	if err != nil || u.Scheme != "file" {
		return uri
	}
	return filepath.FromSlash(u.Path)
}

func languageOf(file string) string {
	switch strings.ToLower(filepath.Ext(file)) {
	case ".tsx":
		return "typescriptreact"
	case ".js", ".mjs", ".cjs":
		return "javascript"
	case ".jsx":
		return "javascriptreact"
	}
	return "typescript"
}
