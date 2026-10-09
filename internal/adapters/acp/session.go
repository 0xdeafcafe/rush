// Package acp drives any agent that speaks the Agent Client Protocol
// (https://agentclientprotocol.com): JSON-RPC over the agent's stdin and
// stdout. It starts or resumes one session and turns what the agent says
// into rush's own events. Copilot, Kimi, Mistral Vibe, Gemini and
// OpenCode adapters are thin layers over it.
package acp

import (
	"context"
	"encoding/base64"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"mime"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/pgguard"
)

// Options is how to run an ACP agent and which session to open.
type Options struct {
	Command string   // the agent's program: "kimi", "copilot"
	Args    []string // what makes it speak ACP: "acp", "--acp"
	Env     []string // added to rush's own
	Dir     string   // the session's working directory; rush's when empty
	// Resume is a past session to open instead of a new one.
	Resume string
	// MCPServers are passed to the agent as ACP McpServer objects.
	MCPServers []jsontext.Value
	// Adapter names the agent in the events nothing else fits: "kimi".
	Adapter string
	// NoFS keeps file reads and writes with the agent rather than rush.
	NoFS bool
	// Trusted are the tools allowed without asking, as the agent names
	// them: mcp__rush__spawn_agent.
	Trusted []string
}

// ErrUnsupported is what the agent can't do.
var ErrUnsupported = errors.New("acp: the agent doesn't support this")

// Session is one ACP session, and the agent process running it when
// Start started one.
type Session struct {
	o    Options
	rpc  *rpc
	info Info
	id   string
	cwd  string

	cmd    *exec.Cmd
	stdin  io.Closer
	stderr tail
	exited chan struct{}

	events chan event.Event
	qmu    sync.Mutex
	qcond  *sync.Cond
	queue  []event.Event
	qend   bool
	closed chan struct{}
	once   sync.Once
	turns  sync.WaitGroup

	amu       sync.Mutex
	approvals map[string]jsontext.Value // approval ID → the agent's request ID
	questions map[string]*elicitation

	mu       sync.Mutex // what follows, and the order events go out in
	ready    bool
	gone     bool // the agent has gone: no more turns
	open     *event.Message
	calls    map[string]*call
	modes    []Mode
	mode     string
	config   []configOption
	legacy   string // the model, from the unstable models field
	commands []string
	cost     float64
}

var _ agent.Conn = (*Session)(nil)
var _ agent.Answerer = (*Session)(nil)

// Start runs the agent, introduces rush and opens the session.
func Start(ctx context.Context, o Options) (*Session, error) {
	cmd := exec.Command(o.Command, o.Args...)
	cmd.Dir = o.Dir
	if len(o.Env) > 0 {
		cmd.Env = append(os.Environ(), o.Env...)
	}
	// Its own process group, so stopping the session takes its shells too.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	s := newSession(o, stdin)
	s.cmd, s.stdin = cmd, stdin
	cmd.Stderr = &s.stderr
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	guard := pgguard.Watch(cmd.Process.Pid)
	go func() {
		s.rpc.read(stdout)
		_ = cmd.Wait()
		guard.Release()
		s.ended()
	}()
	if err := s.begin(ctx); err != nil {
		_ = s.Close()
		// The agent's own answer says why; else what it last said on stderr.
		var rpcErr *Error
		if t := s.stderr.String(); t != "" && !errors.As(err, &rpcErr) {
			err = fmt.Errorf("%w: %s", err, lastLine(t))
		}
		return nil, err
	}
	return s, nil
}

// Dial opens a session with an agent already running at the other end of
// r and w.
func Dial(ctx context.Context, r io.Reader, w io.Writer, o Options) (*Session, error) {
	s := newSession(o, w)
	if c, ok := w.(io.Closer); ok {
		s.stdin = c
	}
	go func() {
		s.rpc.read(r)
		s.ended()
	}()
	if err := s.begin(ctx); err != nil {
		_ = s.Close()
		return nil, err
	}
	return s, nil
}

func newSession(o Options, w io.Writer) *Session {
	if o.Adapter == "" {
		o.Adapter = "acp"
	}
	s := &Session{
		o: o, rpc: newRPC(w), exited: make(chan struct{}),
		events: make(chan event.Event, 64), closed: make(chan struct{}),
		approvals: map[string]jsontext.Value{}, questions: map[string]*elicitation{},
		calls: map[string]*call{},
	}
	s.qcond = sync.NewCond(&s.qmu)
	s.rpc.notify, s.rpc.serve = s.notified, s.served
	go s.pump()
	return s
}

// ended runs once the agent has gone: turns still out end in an error,
// then the events channel closes.
func (s *Session) ended() {
	s.mu.Lock()
	s.gone = true
	s.mu.Unlock()
	s.turns.Wait()
	close(s.exited)
	s.qmu.Lock()
	s.qend = true
	s.qmu.Unlock()
	s.qcond.Broadcast()
}

// begin introduces rush and opens the session.
func (s *Session) begin(ctx context.Context) error {
	var init initializeResult
	err := s.rpc.call(ctx, "initialize", map[string]any{
		"protocolVersion": ProtocolVersion,
		"clientCapabilities": map[string]any{
			"fs":          map[string]bool{"readTextFile": !s.o.NoFS, "writeTextFile": !s.o.NoFS},
			"terminal":    false,
			"elicitation": map[string]any{"form": map[string]any{}},
		},
		"clientInfo": map[string]string{"name": "rush", "title": "rush", "version": "dev"},
	}, &init)
	if err != nil {
		return fmt.Errorf("acp: initialize: %w", err)
	}
	s.info = Info{
		Protocol:    init.ProtocolVersion,
		LoadSession: init.AgentCapabilities.LoadSession,
		Resume:      len(init.AgentCapabilities.SessionCapabilities.Resume) > 0 && string(init.AgentCapabilities.SessionCapabilities.Resume) != "null",
		Images:      init.AgentCapabilities.PromptCapabilities.Image,
		AuthMethods: init.AuthMethods,
	}
	if a := init.AgentInfo; a != nil {
		s.info.Name, s.info.Title, s.info.Version = a.Name, a.Title, a.Version
	}
	if s.cwd, err = filepath.Abs(s.o.Dir); err != nil {
		return err
	}
	servers := s.o.MCPServers
	if servers == nil {
		servers = []jsontext.Value{}
	}
	params := map[string]any{"cwd": s.cwd, "mcpServers": servers}
	var res sessionResult
	switch {
	case s.o.Resume == "":
		err = s.rpc.call(ctx, "session/new", params, &res)
		s.id = res.SessionID
	case s.info.LoadSession, s.info.Resume:
		// session/load replays the conversation as updates first.
		s.id, params["sessionId"] = s.o.Resume, s.o.Resume
		method := "session/load"
		if !s.info.LoadSession {
			method = "session/resume"
		}
		err = s.rpc.call(ctx, method, params, &res)
	default:
		return fmt.Errorf("acp: resume: %w", ErrUnsupported)
	}
	if err != nil {
		return fmt.Errorf("acp: open session: %w", err)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.flush()
	if res.Modes != nil {
		s.modes, s.mode = res.Modes.AvailableModes, res.Modes.CurrentModeID
	}
	if res.Models != nil {
		s.legacy = res.Models.CurrentModelID
	}
	s.setConfig(res.ConfigOptions)
	s.ready = true
	s.emitInit()
	return nil
}

// ID is the session's ID, as the agent knows it.
func (s *Session) ID() string { return s.id }

// Info is what the agent said about itself.
func (s *Session) Info() Info { return s.info }

// PID is the agent's process ID, or 0 when Dial connected it.
func (s *Session) PID() int {
	if s.cmd == nil || s.cmd.Process == nil {
		return 0
	}
	return s.cmd.Process.Pid
}

// Modes are the modes the session can be put in.
func (s *Session) Modes() []Mode {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Mode(nil), s.modes...)
}

// Stderr is the last of what the agent wrote to stderr.
func (s *Session) Stderr() string { return s.stderr.String() }

// Events is everything the session says, until the agent goes.
func (s *Session) Events() <-chan event.Event { return s.events }

// Send prompts the agent. Its reply arrives as events, ending in
// TurnEnd.
func (s *Session) Send(in agent.Input) error {
	var blocks []contentBlock
	if in.Text != "" {
		blocks = append(blocks, contentBlock{Type: "text", Text: in.Text})
	}
	if len(in.Images) > 0 && !s.info.Images {
		return fmt.Errorf("acp: images: %w", ErrUnsupported)
	}
	for _, p := range in.Images {
		b, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		mt := mime.TypeByExtension(strings.ToLower(filepath.Ext(p)))
		if mt == "" {
			mt = http.DetectContentType(b)
		}
		blocks = append(blocks, contentBlock{Type: "image", MimeType: mt, Data: base64.StdEncoding.EncodeToString(b)})
	}
	if len(blocks) == 0 {
		return errors.New("acp: nothing to send")
	}
	s.mu.Lock()
	if s.gone {
		s.mu.Unlock()
		return ErrClosed
	}
	s.turns.Add(1)
	s.mu.Unlock()
	go s.prompt(blocks)
	return nil
}

func (s *Session) prompt(blocks []contentBlock) {
	defer s.turns.Done()
	start := time.Now()
	var res promptResult
	err := s.rpc.call(context.Background(), "session/prompt", map[string]any{"sessionId": s.id, "prompt": blocks}, &res)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.flush()
	end := event.TurnEnd{Reason: stopReason(res.StopReason), Duration: time.Since(start), Turns: 1, Cost: s.cost}
	if err != nil {
		end.Reason, end.Err = "error", err.Error()
		var e *Error
		if errors.As(err, &e) {
			end.Err = e.Message
		}
	}
	if u := res.Usage; u != nil {
		end.Tokens.Input, end.Tokens.Output = u.InputTokens, u.OutputTokens
		end.Tokens.CacheRead, end.Tokens.CacheWrite5m = u.CachedReadTokens, u.CachedWriteTokens
		end.Tokens.Reasoning = u.ThoughtTokens
	}
	s.emit(end)
}

// stopReason is ACP's reason a turn stopped in rush's words.
func stopReason(r string) string {
	switch r {
	case "end_turn", "":
		return "done"
	case "cancelled":
		return "interrupted"
	case "max_turn_requests":
		return "max_turns"
	}
	return r // max_tokens, refusal
}

// Answer answers an Approval with one of its options. No option cancels
// it.
func (s *Session) Answer(approvalID, optionID string) error {
	s.amu.Lock()
	id, ok := s.approvals[approvalID]
	delete(s.approvals, approvalID)
	s.amu.Unlock()
	if !ok {
		return fmt.Errorf("acp: no approval %q waiting", approvalID)
	}
	outcome := map[string]string{"outcome": "cancelled"}
	if optionID != "" {
		outcome = map[string]string{"outcome": "selected", "optionId": optionID}
	}
	return s.rpc.reply(id, map[string]any{"outcome": outcome}, nil)
}

// Interrupt cancels the turn. Approvals still waiting are withdrawn, as
// ACP asks of a client that cancels.
func (s *Session) Interrupt() error {
	if err := s.rpc.send("session/cancel", map[string]string{"sessionId": s.id}); err != nil {
		return err
	}
	s.amu.Lock()
	waiting := s.approvals
	questions := s.questions
	s.approvals = map[string]jsontext.Value{}
	s.questions = map[string]*elicitation{}
	s.amu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	for aid, id := range waiting {
		_ = s.rpc.reply(id, map[string]any{"outcome": map[string]string{"outcome": "cancelled"}}, nil)
		s.emit(event.ApprovalCancelled{ID: aid})
	}
	for qid, q := range questions {
		_ = s.rpc.reply(q.id, map[string]string{"action": "cancel"}, nil)
		s.emit(event.ApprovalCancelled{ID: qid})
	}
	return nil
}

// SetModel switches the session's model, through its "model" setting or
// the older session/set_model.
func (s *Session) SetModel(model string) error {
	s.mu.Lock()
	opt, legacy := s.option("model"), s.legacy != ""
	s.mu.Unlock()
	switch {
	case opt != nil:
		return s.setOption(opt.ID, model)
	case legacy:
		if err := s.rpc.call(context.Background(), "session/set_model", map[string]string{"sessionId": s.id, "modelId": model}, nil); err != nil {
			return err
		}
		s.mu.Lock()
		s.legacy = model
		s.emitInit()
		s.mu.Unlock()
		return nil
	}
	return fmt.Errorf("acp: set model: %w", ErrUnsupported)
}

// SetEffort applies the agent's advertised thinking configuration.
func (s *Session) SetEffort(value string) error {
	s.mu.Lock()
	opt := s.option("thought_level")
	if opt == nil {
		opt = s.option("thinking")
	}
	id := ""
	if opt != nil {
		id = opt.ID
	}
	s.mu.Unlock()
	if id == "" {
		return fmt.Errorf("acp: thinking level: %w", ErrUnsupported)
	}
	return s.setOption(id, value)
}

// SetMode puts the session in one of its Modes.
func (s *Session) SetMode(mode string) error {
	s.mu.Lock()
	known := false
	for _, m := range s.modes {
		known = known || m.ID == mode
	}
	opt := s.option("mode")
	viaOption := opt != nil && len(s.modes) == 0
	s.mu.Unlock()
	switch {
	case known:
		if err := s.rpc.call(context.Background(), "session/set_mode", map[string]string{"sessionId": s.id, "modeId": mode}, nil); err != nil {
			return err
		}
		s.mu.Lock()
		s.mode = mode
		s.emitInit()
		s.mu.Unlock()
		return nil
	case viaOption:
		return s.setOption(opt.ID, mode)
	}
	return fmt.Errorf("acp: mode %q: %w", mode, ErrUnsupported)
}

func (s *Session) setOption(id, value string) error {
	var res struct {
		ConfigOptions []configOption `json:"configOptions"`
	}
	err := s.rpc.call(context.Background(), "session/set_config_option", map[string]string{"sessionId": s.id, "configId": id, "value": value}, &res)
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.setConfig(res.ConfigOptions)
	s.emitInit()
	return nil
}

// option is the setting of category, if the session has one.
func (s *Session) option(category string) *configOption {
	for i, o := range s.config {
		if o.Category == category || (o.Category == "" && o.ID == category) {
			return &s.config[i]
		}
	}
	return nil
}

func (s *Session) setConfig(opts []configOption) {
	if opts == nil {
		return
	}
	s.config = opts
	if o := s.option("mode"); o != nil && len(s.modes) == 0 {
		s.mode = o.current()
	}
}

func (s *Session) model() string {
	if o := s.option("model"); o != nil {
		return o.current()
	}
	return s.legacy
}

// emitInit says what the session is. It goes again whenever that
// changes: its model, its mode, its commands.
func (s *Session) emitInit() {
	if !s.ready {
		return
	}
	s.emit(event.Init{SessionID: s.id, Model: s.model(), Cwd: s.cwd, Mode: s.modeName(), ModeID: s.mode, Modes: s.permissionModes(), Version: s.info.Version,
		Effort: s.effort(), Commands: append([]string(nil), s.commands...)})
}

// permissionModes preserves protocol IDs; display names cannot be sent as IDs.
func (s *Session) permissionModes() []event.PermissionMode {
	out := make([]event.PermissionMode, 0, len(s.modes))
	for _, m := range s.modes {
		out = append(out, event.PermissionMode{ID: m.ID, Name: m.Name, Description: m.Description})
	}
	if len(out) == 0 {
		if opt := s.option("mode"); opt != nil {
			var add func([]configValue)
			add = func(values []configValue) {
				for _, v := range values {
					if v.Value != "" {
						out = append(out, event.PermissionMode{ID: v.Value, Name: v.Name, Description: v.Description})
					}
					add(v.Options)
				}
			}
			add(opt.Options)
		}
	}
	return out
}

// modeName is the mode the session is in, by its name when it has one:
// some agents' mode ids are URLs.
func (s *Session) modeName() string {
	for _, m := range s.modes {
		if m.ID == s.mode && m.Name != "" {
			return strings.ToLower(m.Name)
		}
	}
	return s.mode
}

// Close ends the session: the agent is asked to go by closing its stdin,
// then made to.
func (s *Session) Close() error {
	s.once.Do(func() {
		s.qmu.Lock()
		close(s.closed)
		s.qmu.Unlock()
		s.qcond.Broadcast()
		if s.stdin != nil {
			_ = s.stdin.Close()
		}
		if s.cmd == nil || s.cmd.Process == nil {
			return
		}
		pid := s.cmd.Process.Pid
		for _, sig := range []syscall.Signal{0, syscall.SIGTERM, syscall.SIGKILL} {
			if sig != 0 {
				_ = syscall.Kill(-pid, sig)
			}
			select {
			case <-s.exited:
				return
			case <-time.After(2 * time.Second):
			}
		}
	})
	return nil
}

// emit queues ev. The queue never blocks the reader, whoever is slow to
// take events.
func (s *Session) emit(ev event.Event) {
	s.qmu.Lock()
	s.queue = append(s.queue, ev)
	s.qmu.Unlock()
	s.qcond.Signal()
}

func (s *Session) pump() {
	defer close(s.events)
	for {
		s.qmu.Lock()
		for len(s.queue) == 0 && !s.qend && !s.isClosed() {
			s.qcond.Wait()
		}
		q := s.queue
		s.queue = nil
		s.qmu.Unlock()
		if len(q) == 0 {
			return
		}
		for _, ev := range q {
			select {
			case s.events <- ev:
			case <-s.closed:
				return
			}
		}
	}
}

func (s *Session) isClosed() bool {
	select {
	case <-s.closed:
		return true
	default:
		return false
	}
}

// tail keeps the last few KB of stderr for error messages.
type tail struct {
	mu  sync.Mutex
	buf []byte
}

func (t *tail) Write(p []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.buf = append(t.buf, p...)
	if n := len(t.buf); n > 4<<10 {
		t.buf = t.buf[n-4<<10:]
	}
	return len(p), nil
}

func (t *tail) String() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return strings.TrimSpace(string(t.buf))
}

func lastLine(s string) string {
	if i := strings.LastIndexByte(s, '\n'); i >= 0 {
		return s[i+1:]
	}
	return s
}

func (s *Session) effort() string {
	for _, category := range []string{"thought_level", "thinking"} {
		if o := s.option(category); o != nil {
			return o.current()
		}
	}
	return ""
}
