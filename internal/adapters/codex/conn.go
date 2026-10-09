package codex

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"os"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/agtools"
)

// Server requests rush answers.
const (
	reqCommand     = "item/commandExecution/requestApproval"
	reqFileChange  = "item/fileChange/requestApproval"
	reqPermissions = "item/permissions/requestApproval"
	reqUserInput   = "item/tool/requestUserInput"
	reqElicitation = "mcpServer/elicitation/request"
)

// mode is one of Codex's approval presets, as a thread starts with it
// and as a turn overrides it.
type mode struct {
	approval    string
	sandbox     string         // thread/start's SandboxMode
	sandboxTurn map[string]any // turn/start's SandboxPolicy
}

// modes are the presets Codex's own /approvals offers.
var modes = map[string]mode{
	"read-only": {"on-request", "read-only", map[string]any{"type": "readOnly", "networkAccess": false}},
	"auto": {"on-request", "workspace-write", map[string]any{"type": "workspaceWrite", "writableRoots": []string{},
		"networkAccess": false, "excludeTmpdirEnvVar": false, "excludeSlashTmp": false}},
	"full-access": {"never", "danger-full-access", map[string]any{"type": "dangerFullAccess"}},
}

// modeOf is the preset a thread's approval policy and sandbox type are,
// or "" when they're none of them.
func modeOf(approval, sandbox string) string {
	for id, m := range modes {
		if m.approval == approval && m.sandboxTurn["type"] == sandbox {
			return id
		}
	}
	return ""
}

// ask is a server request waiting on the user.
type ask struct {
	id     jsontext.Value
	method string
	perms  map[string]jsontext.Value // permissions asked for
	qs     map[string]string         // a question's text → its id
}

// Conn is a Codex thread run through `codex app-server`.
type Conn struct {
	rpc     *client
	ctx     context.Context
	cancel  context.CancelFunc
	version string
	models  []cachedModel // what its account offers, to name a model by

	mu      sync.Mutex
	thread  string
	turn    string // the running turn's id
	ended   string // the last turn to end, so a late turn/start answer doesn't revive it
	model   string
	account string
	next    struct{ model, effort, mode string } // applied on the next turn/start
	asks    map[string]ask
	calls   map[string]tool.Call // tool calls by item id, for approvals
	open    map[string]bool      // items streaming as a message
	tokens  usage.TokenUsage     // this turn's
	kids    map[string]*kid      // the threads it spawned, by id
	shells  map[string]*shell    // commands not yet completed, by item id

	qmu    sync.Mutex
	queue  []event.Event
	wake   chan struct{}
	events chan event.Event
	quit   chan struct{}
	once   sync.Once
}

var _ agent.Answerer = (*Conn)(nil)

// Start starts or resumes a thread: CODEX_HOME is the profile's folder,
// and the model, effort and mode are Codex's own names.
func Start(ctx context.Context, o agent.StartOptions) (*Conn, error) {
	if _, ok := modes[o.Mode]; !ok {
		// A mode saved under another harness or an older name starts on
		// Codex's own, rather than not at all.
		o.Mode = ""
	}
	c := newConn(ctx)
	rpc, err := spawn(o.Binary, o.Profile.Dir, o.Env, o.Flags, c.handle)
	if err != nil {
		c.cancel()
		return nil, err
	}
	if err := c.begin(rpc, o); err != nil {
		_ = c.Close()
		return nil, err
	}
	return c, nil
}

func newConn(ctx context.Context) *Conn {
	c := &Conn{asks: map[string]ask{}, calls: map[string]tool.Call{}, open: map[string]bool{},
		wake: make(chan struct{}, 1), events: make(chan event.Event, 64), quit: make(chan struct{})}
	c.ctx, c.cancel = context.WithCancel(ctx)
	return c
}

// begin shakes hands and opens the thread. The thread's own settings go
// in thread/start; effort only exists per turn.
func (c *Conn) begin(rpc *client, o agent.StartOptions) error {
	c.rpc = rpc
	go c.pump()
	go func() {
		select {
		case <-c.ctx.Done():
			_ = c.Close()
		case <-c.quit:
		}
	}()
	v, err := rpc.initialize(c.ctx)
	if err != nil {
		return err
	}
	c.version = v
	c.models = readModels(o.Profile.Dir)
	if o.Model, err = modelID(o.Model, c.models); err != nil {
		return err
	}
	params := map[string]any{}
	set := func(k string, v any) {
		if v != "" {
			params[k] = v
		}
	}
	set("model", o.Model)
	set("cwd", o.Dir)
	// rush's own prompt, which Claude Code gets in --append-system-prompt.
	set("developerInstructions", strings.TrimSpace(o.Prompt))
	if i := slices.IndexFunc(o.Tools, func(t agent.ToolServer) bool { return t.Name == agtools.Server && t.Args != nil }); i >= 0 {
		// rush's own tools, which Claude Code is served in process, as a
		// process; a call may wait on another agent's whole task.
		if exe, err := os.Executable(); err == nil {
			// Codex starts it with a bare environment; it needs the session's (RUSH_HOME).
			params["config"] = map[string]any{"mcp_servers." + agtools.Server: map[string]any{"command": exe, "args": o.Tools[i].Args,
				"tool_timeout_sec": 3600, "env_vars": envNames(append(os.Environ(), o.Env...))}}
		}
	}
	if len(o.SkillRoots) > 0 {
		// Claude Code's skills beside Codex's own; not reading them is no reason not to start.
		_ = rpc.call(c.ctx, "skills/extraRoots/set", map[string]any{"extraRoots": o.SkillRoots}, nil)
	}
	if m, ok := modes[o.Mode]; ok {
		params["approvalPolicy"], params["sandbox"] = m.approval, m.sandbox
	}
	c.next.effort = o.Effort
	method := "thread/start"
	switch {
	case o.Fork:
		method, params["threadId"] = "thread/fork", o.SessionID
		params["excludeTurns"] = true
	case o.Resume:
		method, params["threadId"] = "thread/resume", o.SessionID
		params["excludeTurns"] = true
	}
	var res struct {
		Thread struct {
			ID string `json:"id"`
		} `json:"thread"`
		Model          string         `json:"model"`
		Cwd            string         `json:"cwd"`
		ApprovalPolicy jsontext.Value `json:"approvalPolicy"`
		Sandbox        struct {
			Type string `json:"type"`
		} `json:"sandbox"`
	}
	if err := rpc.call(c.ctx, method, params, &res); err != nil {
		return err
	}
	c.mu.Lock()
	c.thread, c.model = res.Thread.ID, res.Model
	c.mu.Unlock()
	if method == "thread/start" && len(o.Carry) > 0 {
		// Another agent's conversation, as this thread's own history.
		if err := rpc.call(c.ctx, "thread/inject_items", map[string]any{"threadId": res.Thread.ID, "items": carried(o.Carry)}, nil); err != nil {
			return fmt.Errorf("codex: taking the conversation on: %w", err)
		}
	}
	init := event.Init{SessionID: res.Thread.ID, Model: res.Model, Cwd: res.Cwd, Mode: o.Mode, Version: c.version}
	if init.Mode == "" {
		// Its own policy, as the preset that sets it: a raw "on-request"
		// saved as the mode would fail the next start.
		var approval string
		_ = jsonx.Unmarshal(res.ApprovalPolicy, &approval)
		init.Mode = modeOf(approval, res.Sandbox.Type)
	}
	c.emit(init)
	go c.readQuota()
	return nil
}

// readQuota reads the account's limits once, which also tells later
// live readings whose account they are, and how the account pays.
func (c *Conn) readQuota() {
	ctx, cancel := context.WithTimeout(c.ctx, 30*time.Second)
	defer cancel()
	var acct accountResponse
	if c.rpc.call(ctx, "account/read", map[string]any{}, &acct) == nil {
		if b, ok := accountBilling(acct); ok {
			c.emit(event.Billing{Billing: b})
		}
	}
	q, err := readQuota(ctx, c.rpc)
	if err != nil {
		return
	}
	c.mu.Lock()
	c.account = q.Account
	c.mu.Unlock()
	c.emit(event.Quota{Quota: q})
}

func (c *Conn) Events() <-chan event.Event { return c.events }

// Send starts a turn, or steers the running one. /compact compacts the
// thread, as a turn of its own.
func (c *Conn) Send(in agent.Input) error {
	if strings.TrimSpace(in.Text) == "/compact" {
		if len(in.Images) != 0 {
			return errors.New("codex: /compact does not accept attachments")
		}
		c.mu.Lock()
		thread, turn := c.thread, c.turn
		c.mu.Unlock()
		if turn != "" {
			return errors.New("codex: let the current turn finish before compacting")
		}
		return c.rpc.call(c.ctx, "thread/compact/start", map[string]any{"threadId": thread}, nil)
	}
	input := []map[string]any{{"type": "text", "text": in.Text, "text_elements": []any{}}}
	for _, p := range in.Images {
		input = append(input, map[string]any{"type": "localImage", "path": p})
	}
	c.mu.Lock()
	thread, turn, next := c.thread, c.turn, c.next
	c.mu.Unlock()
	if turn != "" {
		err := c.rpc.call(c.ctx, "turn/steer", map[string]any{"threadId": thread, "input": input, "expectedTurnId": turn}, nil)
		var re *rpcError
		if !errors.As(err, &re) {
			return err // steered, or the app-server is gone
		}
		// The turn ended meanwhile or can't be steered: start another.
	}
	params := map[string]any{"threadId": thread, "input": input}
	if next.model != "" {
		params["model"] = next.model
	}
	if next.effort != "" {
		params["effort"] = next.effort
	}
	if m, ok := modes[next.mode]; ok {
		params["approvalPolicy"], params["sandboxPolicy"] = m.approval, m.sandboxTurn
	}
	var res struct {
		Turn struct {
			ID string `json:"id"`
		} `json:"turn"`
	}
	if err := c.rpc.call(c.ctx, "turn/start", params, &res); err != nil {
		return err
	}
	c.mu.Lock()
	if c.turn == "" && c.ended != res.Turn.ID {
		c.turn = res.Turn.ID
	}
	// Codex keeps a turn's overrides for the turns after it.
	if c.next == next {
		c.next = struct{ model, effort, mode string }{}
	}
	if next.model != "" {
		c.model = next.model
	}
	c.mu.Unlock()
	return nil
}

// Interrupt stops the running turn, if there is one.
func (c *Conn) Interrupt() error {
	c.mu.Lock()
	thread, turn := c.thread, c.turn
	c.mu.Unlock()
	if turn == "" {
		return nil
	}
	return c.rpc.call(c.ctx, "turn/interrupt", map[string]any{"threadId": thread, "turnId": turn}, nil)
}

// SetModel changes the model from the next turn on.
func (c *Conn) SetModel(model string) error {
	model, err := modelID(model, c.models)
	if err != nil {
		return err
	}
	c.mu.Lock()
	c.next.model = model
	c.mu.Unlock()
	return nil
}

// SetEffort changes the reasoning effort from the next turn on.
func (c *Conn) SetEffort(effort string) error {
	c.mu.Lock()
	c.next.effort = effort
	c.mu.Unlock()
	return nil
}

// SetMode changes the approval preset from the next turn on: read-only,
// auto or full-access.
func (c *Conn) SetMode(m string) error {
	if _, ok := modes[m]; !ok {
		return fmt.Errorf("codex: unknown mode %q", m)
	}
	c.mu.Lock()
	c.next.mode = m
	c.mu.Unlock()
	return nil
}

// Answer answers an approval with one of its options' IDs.
func (c *Conn) Answer(approvalID, optionID string) error {
	a, err := c.take(approvalID)
	if err != nil {
		return err
	}
	switch a.method {
	case reqCommand, reqFileChange:
		switch optionID {
		case "accept", "acceptForSession", "decline", "cancel":
			return c.rpc.reply(a.id, map[string]any{"decision": optionID})
		}
	case reqPermissions:
		switch optionID {
		case "turn", "session":
			granted := map[string]jsontext.Value{}
			for k, v := range a.perms {
				if string(v) != "null" {
					granted[k] = v
				}
			}
			return c.rpc.reply(a.id, map[string]any{"permissions": granted, "scope": optionID})
		case "decline":
			return c.rpc.reply(a.id, map[string]any{"permissions": map[string]any{}, "scope": "turn"})
		}
	case reqUserInput:
		c.put(approvalID, a)
		return fmt.Errorf("codex: %s is a question; answer it with AnswerQuestion", approvalID)
	}
	c.put(approvalID, a)
	return fmt.Errorf("codex: %q is no answer to %s", optionID, a.method)
}

// AnswerQuestion answers a request_user_input question, keyed by each
// Ask's ID or text.
func (c *Conn) AnswerQuestion(id string, answers map[string][]string) error {
	a, err := c.take(id)
	if err != nil {
		return err
	}
	if a.method != reqUserInput {
		c.put(id, a)
		return fmt.Errorf("codex: %s is not a question", id)
	}
	out := map[string]any{}
	for k, v := range answers {
		if qid, ok := a.qs[k]; ok {
			k = qid
		}
		out[k] = map[string]any{"answers": v}
	}
	return c.rpc.reply(a.id, map[string]any{"answers": out})
}

func (c *Conn) take(id string) (ask, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	a, ok := c.asks[id]
	if !ok {
		return ask{}, fmt.Errorf("codex: nothing is waiting on %s", id)
	}
	delete(c.asks, id)
	return a, nil
}

func (c *Conn) put(id string, a ask) {
	c.mu.Lock()
	c.asks[id] = a
	c.mu.Unlock()
}

// Close ends the app-server and the thread with it.
func (c *Conn) Close() error {
	c.once.Do(func() {
		close(c.quit)
		c.cancel()
		if c.rpc != nil {
			_ = c.rpc.close()
		}
	})
	return nil
}

// emit queues events for Events, so the reader never waits on whoever
// draws them.
func (c *Conn) emit(evs ...event.Event) {
	if len(evs) == 0 {
		return
	}
	c.qmu.Lock()
	c.queue = append(c.queue, evs...)
	c.qmu.Unlock()
	select {
	case c.wake <- struct{}{}:
	default:
	}
}

func (c *Conn) pump() {
	defer close(c.events)
	for {
		c.qmu.Lock()
		q := c.queue
		c.queue = nil
		c.qmu.Unlock()
		for _, e := range q {
			select {
			case c.events <- e:
			case <-c.quit:
				return
			}
		}
		if len(q) > 0 {
			continue
		}
		select {
		case <-c.wake:
		case <-c.quit:
			return
		case <-c.rpc.done:
			c.qmu.Lock()
			done := len(c.queue) == 0
			c.qmu.Unlock()
			if done {
				return
			}
		}
	}
}

// carried are lines as Responses API messages: yours as input, the other
// agent's answers as output.
func carried(lines []agent.Line) []map[string]any {
	out := make([]map[string]any, 0, len(lines))
	for _, l := range lines {
		part := "input_text"
		if l.Role == "assistant" {
			part = "output_text"
		}
		out = append(out, map[string]any{"type": "message", "role": l.Role, "content": []map[string]any{{"type": part, "text": l.Text}}})
	}
	return out
}

// envNames are the names of env's variables, each once.
func envNames(env []string) []string {
	var names []string
	for _, kv := range env {
		if k, _, ok := strings.Cut(kv, "="); ok && k != "" && !slices.Contains(names, k) {
			names = append(names, k)
		}
	}
	return names
}
