package claude

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"errors"
	"maps"
	"mime"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Start runs claude -p for rush to draw.
func (a Adapter) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) { //nolint:gocritic // agent.Driver's signature
	acct, root := Account(o.Profile), false
	if acct.IsDefault() {
		// ~/.claude runs as the login in use, in its home.
		acct, root = a.runAs(), true
	}
	c := &conn{events: make(chan event.Event, 64), asks: map[string]headless.PermissionRequest{},
		waits: map[string]chan headless.ControlReply{}, acct: acct, root: root, tools: slices.Clone(o.Tools),
		done: make(chan struct{}), taps: o.Tap != nil}
	ho := headless.Options{Account: acct, Dir: o.Dir, Model: o.Model, Effort: o.Effort,
		PermissionMode: o.Mode, Binary: o.Binary}
	if allowed := c.trusted(); len(allowed) > 0 {
		// rush's own tools only draw, so they never ask.
		ho.Flags = append(ho.Flags, "--allowedTools", strings.Join(allowed, ","))
	}
	if len(o.Agents) > 0 {
		b, _ := jsonx.Marshal(o.Agents)
		ho.Flags = append(ho.Flags, "--agents", string(b))
	}
	if p := strings.TrimSpace(o.Prompt); p != "" {
		ho.Flags = append(ho.Flags, "--append-system-prompt", p)
	}
	if len(o.Without) > 0 {
		// Taken out of the model's context, not only refused.
		ho.Flags = append(ho.Flags, "--disallowedTools", strings.Join(o.Without, ","))
	}
	hooks := map[string]any{}
	if o.Inbox != "" {
		// After each tool call, a message waiting for the agent that made
		// it goes into that agent's own context: the one way into a
		// running subagent that isn't the main session's SendMessage.
		h := []map[string]any{{"matcher": "*", "hooks": []map[string]any{{"type": "command", "command": o.Inbox, "timeout": 10}}}}
		hooks["PostToolUse"], hooks["PostToolUseFailure"] = h, h
	}
	if o.BashHook != "" {
		hooks["PreToolUse"] = []map[string]any{{"matcher": "Bash", "hooks": []map[string]any{{"type": "command", "command": o.BashHook, "timeout": 10}}}}
	}
	if len(hooks) > 0 {
		b, _ := jsonx.Marshal(map[string]any{"hooks": hooks})
		ho.Flags = append(ho.Flags, "--settings", string(b))
	}
	ho.Flags = append(ho.Flags, o.Flags...)
	if len(o.Carry) > 0 && !o.Resume && !o.Fork && o.SessionID != "" {
		// Another agent's conversation, resumed as this one's own.
		if err := acct.WriteCarried(o.Dir, o.SessionID, o.Carry); err != nil {
			return nil, err
		}
		o.Resume = true
		c.carried = o.SessionID
	}
	if o.Resume {
		ho.Resume = o.SessionID
	} else {
		ho.SessionID = o.SessionID
	}
	if o.Fork {
		ho.Flags = append(ho.Flags, "--fork-session")
	}
	// Its scratch goes in the session's own folder, so what it leaves
	// behind can be seen and cleaned up.
	if o.TempDir != "" && os.MkdirAll(o.TempDir, 0o700) == nil {
		ho.Env = append(ho.Env, "TMPDIR="+o.TempDir, "TMP="+o.TempDir, "TEMP="+o.TempDir, "CLAUDE_CODE_TMPDIR="+o.TempDir)
	}
	if o.Lean {
		ho.Env = append(ho.Env, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1")
	}
	// A stream that goes silent is cut and tried again, not waited on for
	// good; one you've set yourself stands.
	if os.Getenv(StreamWatchdogEnv) == "" {
		ho.Env = append(ho.Env, StreamWatchdogEnv+"=1")
	}
	// Claude Code leaves the task list out for some models (Opus) when run
	// headless; rush shows it live, so it's always in.
	if os.Getenv("CLAUDE_CODE_ENABLE_TODO_TOOLS") == "" {
		ho.Env = append(ho.Env, "CLAUDE_CODE_ENABLE_TODO_TOOLS=1")
	}
	// Checkpoints, as Claude Code keeps them in a terminal, so a rewind can
	// put the files back too. The session's own environment goes last, to
	// have the last word.
	if o.APIKey != "" {
		// Ahead of the login signed in: paid per token.
		ho.Env = append(ho.Env, "ANTHROPIC_API_KEY="+o.APIKey)
	}
	if o.Subagents > 0 && os.Getenv(subagentCapEnv) == "" {
		ho.Env = append(ho.Env, subagentCapEnv+"="+strconv.Itoa(o.Subagents))
	}
	ho.Env = append(append(ho.Env, headless.CheckpointEnv), o.Env...)
	if o.Tap != nil {
		tap := o.Tap
		ho.Tap = func(line []byte) {
			if !c.ownTraffic(line) {
				tap(line)
			}
		}
	}
	if o.Lightly {
		ho.Skip = relayOnly
	}
	c.login = claude.SignedInAs(acct)
	s, err := headless.Start(ho)
	if err != nil {
		return nil, err
	}
	c.s = s
	names := make([]string, len(c.tools))
	for i, t := range c.tools {
		names[i] = t.Name
	}
	// Every process needs rush's tools registered before its first message.
	if c.initID, err = s.Initialize(names...); err != nil {
		_ = s.Stop(time.Second)
		return nil, err
	}
	go c.relay(ctx)
	return c, nil
}

// relayOnly is output a caller that has the lines passes on without
// reading: streamed deltas and tool results (user messages). They are most
// of what Claude Code writes, and the biggest lines.
func relayOnly(l []byte) bool {
	return bytes.HasPrefix(l, []byte(`{"type":"stream_event"`)) || bytes.HasPrefix(l, []byte(`{"type":"user"`))
}

// conn is a running claude -p as an agent.Conn.
type conn struct {
	s       *headless.Session
	events  chan event.Event
	tools   []agent.ToolServer
	initID  string
	carried string // imported history acknowledged by initialize
	acct    claude.Account
	// root is whether it was started for ~/.claude and runs in the
	// home of the login in use then.
	root bool
	// login is the account it started signed in as: its plan usage
	// readings are that login's.
	login string
	taps  bool          // its lines reach StartOptions.Tap
	done  chan struct{} // closed once it has ended

	mu    sync.Mutex
	asks  map[string]headless.PermissionRequest // approvals and questions not yet answered
	waits map[string]chan headless.ControlReply // control requests out, by id
	live  claude.Usage                          // the last usage reading passed on
}

// tool is the server a tool of Claude Code's name is one of rush's, and
// the tool's own name.
func (c *conn) tool(name string) (agent.ToolServer, string, bool) {
	for _, t := range c.tools {
		if rest, ok := strings.CutPrefix(name, "mcp__"+t.Name+"__"); ok {
			return t, rest, true
		}
	}
	return agent.ToolServer{}, "", false
}

// trusted are the tools that never ask, by Claude Code's names.
func (c *conn) trusted() []string {
	var out []string
	for _, t := range c.tools {
		for _, n := range t.Trusted {
			out = append(out, "mcp__"+t.Name+"__"+n)
		}
	}
	return out
}

func (c *conn) isTrusted(name string) bool {
	t, n, ok := c.tool(name)
	return ok && slices.Contains(t.Trusted, n)
}

// ownTraffic is a control request for rush's own tools: an MCP message, or
// a permission check for one that never asks. It stays between Claude Code
// and rush.
func (c *conn) ownTraffic(l []byte) bool {
	if !bytes.HasPrefix(l, []byte(`{"type":"control_request"`)) {
		return false
	}
	var e struct {
		Request struct {
			Subtype string `json:"subtype"`
			Server  string `json:"server_name"`
			Tool    string `json:"tool_name"`
		} `json:"request"`
	}
	if jsonx.Unmarshal(l, &e) != nil {
		return false
	}
	r := e.Request
	return r.Subtype == "mcp_message" && slices.ContainsFunc(c.tools, func(t agent.ToolServer) bool { return t.Name == r.Server }) ||
		r.Subtype == "can_use_tool" && c.isTrusted(r.Tool)
}

func (c *conn) relay(ctx context.Context) {
	defer close(c.events)
	defer close(c.done)
	var n headless.Neutral
	for {
		select {
		case <-ctx.Done():
			_ = c.s.Stop(3 * time.Second)
			return
		case ev, ok := <-c.s.Events:
			if !ok {
				return
			}
			if !c.own(ev) {
				for _, out := range n.Event(ev) {
					c.events <- out
				}
			}
		}
	}
}

// own takes what the conn answers itself rather than passing on, and
// notes what it needs of the rest: it reports whether ev was all its own.
func (c *conn) own(ev headless.Event) bool {
	switch e := ev.(type) {
	case headless.PermissionRequest:
		if c.isTrusted(e.Tool) {
			// Asked despite --allowedTools (a mode that asks for
			// everything): they only draw, so yes.
			_ = c.s.Allow(e, nil, false)
			return true
		}
		c.mu.Lock()
		c.asks[e.ID] = e
		c.mu.Unlock()
	case headless.PermissionCancelled:
		c.mu.Lock()
		delete(c.asks, e.ID)
		c.mu.Unlock()
	case headless.MCPRequest:
		for _, t := range c.tools {
			if t.Name == e.Server && t.Handle != nil {
				// A plugin's tool may take a while; the session carries on.
				go func() { _ = c.s.ReplyMCP(e.ID, t.Handle(e.Message)) }()
			}
		}
		return true
	case headless.ControlReply:
		c.mu.Lock()
		w, ok := c.waits[e.ID]
		delete(c.waits, e.ID)
		c.mu.Unlock()
		if ok {
			w <- e
			return true
		}
		if e.ID == c.initID && e.Error == "" {
			if c.carried != "" {
				c.events <- event.Init{SessionID: c.carried}
				c.carried = ""
			}
			var cmds event.Commands
			for _, k := range headless.Commands(e) {
				cmds.List = append(cmds.List, event.Command(k))
			}
			c.events <- cmds
		}
		return true
	case headless.RateLimit:
		c.shareUsage(e)
	}
	return false
}

// shareUsage passes the plan usage Claude Code reports with each request
// to every rush, as a reading of the login it runs on: the header and
// switching accounts then go by it, not by a fetch minutes old. A reading
// like the last goes only every half minute.
func (c *conn) shareUsage(ev headless.RateLimit) {
	u, ok := claude.LiveUsage(ev.Raw, time.Now())
	if !ok || c.login == "" {
		return
	}
	c.mu.Lock()
	last := c.live
	if u.FiveHour == last.FiveHour && u.SevenDay == last.SevenDay && u.FetchedAt.Sub(last.FetchedAt) < 30*time.Second {
		c.mu.Unlock()
		return
	}
	u.AccountID = c.login
	c.live = u
	c.mu.Unlock()
	acct := c.acct
	go func() {
		// Only while the folder is still signed in as it started: after a
		// switch, the reading may be the new login's.
		_ = claude.RecordLiveUsage(filepath.Join(state.Dir(), "usage.json"), acct, u.AccountID, u)
	}()
}

// KeepsQuota: its readings go where Claude's usage is kept (shareUsage).
func (c *conn) KeepsQuota() {}

// Stale is whether the folder is signed in as another login than the one
// it started on, or another login is in use since it started.
func (c *conn) Stale() bool {
	if c.root && (Adapter{}).runAs().ConfigDir != c.acct.ConfigDir {
		return true
	}
	return c.login != "" && claude.SignedInAs(c.acct) != c.login
}

// await sends a control request with send, and waits for its reply.
func (c *conn) await(ctx context.Context, send func() (string, error)) (headless.ControlReply, error) {
	w := make(chan headless.ControlReply, 1)
	// Held while sending, so the reply can't be read before it's awaited.
	c.mu.Lock()
	id, err := send()
	if err == nil {
		c.waits[id] = w
	}
	c.mu.Unlock()
	if err != nil {
		return headless.ControlReply{}, err
	}
	select {
	case r := <-w:
		if r.Error != "" {
			return r, errors.New(r.Error)
		}
		return r, nil
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.waits, id)
		c.mu.Unlock()
		return headless.ControlReply{}, ctx.Err()
	case <-c.done:
		return headless.ControlReply{}, errors.New("the session ended before it answered")
	}
}

// Ask passes a control request through and returns its reply's body.
func (c *conn) Ask(ctx context.Context, req jsontext.Value) (jsontext.Value, error) {
	r, err := c.await(ctx, func() (string, error) { return c.s.Ask(req) })
	return r.Body, err
}

// ContextUsage is what fills the context window, as /context counts it.
func (c *conn) ContextUsage(ctx context.Context) (usage.Context, error) {
	r, err := c.await(ctx, c.s.AskContextUsage)
	if err != nil {
		return usage.Context{}, err
	}
	u, err := headless.ParseContextUsage(r)
	if err != nil {
		return usage.Context{}, err
	}
	u.At = time.Now()
	return u, nil
}

func (c *conn) Events() <-chan event.Event { return c.events }

func (c *conn) Send(in agent.Input) error {
	var imgs []headless.Image
	for _, p := range in.Images {
		b, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		mt := mime.TypeByExtension(strings.ToLower(filepath.Ext(p)))
		if mt == "" {
			mt = "image/png"
		}
		imgs = append(imgs, headless.Image{MediaType: mt, Data: b})
	}
	return c.s.SendWith(in.Text, imgs)
}

// take is the request id asked, once: answering it forgets it.
func (c *conn) take(id string) (headless.PermissionRequest, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	r, ok := c.asks[id]
	delete(c.asks, id)
	return r, ok
}

var errNotAsked = errors.New("nothing is waiting on that answer")

// Answer answers an approval with one of the options headless.Neutral
// gave it: allow, always or deny.
func (c *conn) Answer(approvalID, optionID string) error {
	switch optionID {
	case "allow":
		return c.Allow(approvalID, nil, false)
	case "always":
		return c.Allow(approvalID, nil, true)
	}
	return c.Deny(approvalID, "", false)
}

// Allow lets a call run, with its input as changed when input is set.
func (c *conn) Allow(approvalID string, input jsontext.Value, always bool) error {
	r, ok := c.take(approvalID)
	if !ok {
		return errNotAsked
	}
	switch {
	case len(input) == 0:
		input = r.Input
	case r.Tool == "AskUserQuestion":
		input = over(r.Input, input)
	}
	return c.s.Allow(r, input, always)
}

// over is input with the keys of top written over it: answers go into
// the question's own input, so whatever else Claude put there stays.
func over(input, top jsontext.Value) jsontext.Value {
	var base, add map[string]jsontext.Value
	if jsonx.Unmarshal(input, &base) != nil || jsonx.Unmarshal(top, &add) != nil {
		return top
	}
	if base == nil {
		base = map[string]jsontext.Value{}
	}
	maps.Copy(base, add)
	b, err := jsonx.Marshal(base)
	if err != nil {
		return top
	}
	return b
}

// Deny refuses a call, saying why when message is set; interrupt stops
// the turn too.
func (c *conn) Deny(approvalID, message string, interrupt bool) error {
	r, ok := c.take(approvalID)
	if !ok {
		return errNotAsked
	}
	return c.s.Deny(r, message, interrupt)
}

// AnswerQuestion answers an AskUserQuestion. Claude takes several choices
// as one answer, joined.
func (c *conn) AnswerQuestion(id string, answers map[string][]string) error {
	r, ok := c.take(id)
	if !ok {
		return errNotAsked
	}
	_, qs := r.Questions()
	joined := map[string]string{}
	for q, labels := range answers {
		joined[q] = strings.Join(labels, ", ")
	}
	return c.s.Allow(r, r.AnswerInput(qs, joined), false)
}

func (c *conn) Interrupt() error               { return c.s.Interrupt() }
func (c *conn) SetModel(model string) error    { return c.s.SetModel(model) }
func (c *conn) SetMode(mode string) error      { return c.s.SetPermissionMode(mode) }
func (c *conn) StopTask(id string) error       { return c.s.StopTask(id) }
func (c *conn) Background(callID string) error { return c.s.Background(callID) }
func (c *conn) Err() error                     { return c.s.Err() }
func (c *conn) Taps() bool                     { return c.taps }
func (c *conn) PID() int                       { return c.s.PID() }
func (c *conn) Close() error                   { return c.s.Stop(3 * time.Second) }

// RunsSession: its sessions are headless Claude Codes.
func (Adapter) RunsSession(args []string, sessionID string) bool {
	return headless.RunsSession(args, sessionID)
}

const subagentCapEnv = "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"

// StreamWatchdogEnv turns on Claude Code's own watchdog for a model stream
// that goes silent.
const StreamWatchdogEnv = "CLAUDE_ENABLE_STREAM_WATCHDOG"
