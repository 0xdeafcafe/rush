package host

import (
	"bytes"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"github.com/0xdeafcafe/photon/uithread"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// Spawn writes cfg and starts its host as a detached process, returning once
// the host is accepting connections. cfg.ID and cfg.SessionID are filled in
// for a new conversation when empty.
// fillIDs names a new session, and gives a fork an id of its own.
func (cfg *Config) fillIDs() {
	if cfg.Kind == "" {
		// migration: callers from before rush ran other agents name
		// none, meaning Claude Code; from here on the kind is written.
		cfg.Kind = string(agent.Migrated(""))
		cfg.Account.Kind = agent.Kind(cfg.Kind)
	}
	if cfg.SessionID == "" {
		cfg.SessionID, cfg.ID = NewSessionID()
	}
	if cfg.ID == "" && cfg.Fork {
		_, cfg.ID = NewSessionID() // the copy is a session of its own
	}
	if cfg.ID == "" {
		cfg.ID = shortOf(cfg.SessionID)
	}
}

func Spawn(cfg Config) (Config, error) {
	cfg.fillIDs()
	d := dir(cfg.ID)
	if err := os.MkdirAll(d, 0o700); err != nil {
		return cfg, err
	}
	if info, err := ReadInfo(cfg.ID); err == nil && alive(info.HostPID) {
		return cfg, fmt.Errorf("%s is already running", cfg.ID)
	}
	b, err := jsonx.MarshalIndent(cfg)
	if err != nil {
		return cfg, err
	}
	if err := os.WriteFile(filepath.Join(d, "config.json"), b, 0o600); err != nil {
		return cfg, err
	}
	exe, err := os.Executable()
	if err != nil {
		return cfg, err
	}
	log, err := os.OpenFile(filepath.Join(d, "host.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return cfg, err
	}
	defer log.Close()
	_ = os.Remove(SockPath(cfg.ID))
	cmd := exec.Command(exe, "host", "run", cfg.ID)
	cmd.Dir = cfg.Cwd
	cmd.Stdout, cmd.Stderr = log, log
	// Its own session, so closing rush or its terminal leaves it running.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return cfg, err
	}
	pid := cmd.Process.Pid
	// Reap it when it ends, or a stopped host lingers as a zombie that still
	// looks alive for as long as this process runs.
	exited := make(chan struct{})
	go func() { _ = cmd.Wait(); close(exited) }()
	for deadline := time.Now().Add(10 * time.Second); time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
		if c, err := net.Dial("unix", SockPath(cfg.ID)); err == nil {
			_ = c.Close()
			if info, err := ReadInfo(cfg.ID); err == nil && info.HostPID == pid {
				return cfg, nil
			}
		}
		select {
		case <-exited:
			tail, _ := os.ReadFile(filepath.Join(d, "host.log"))
			return cfg, fmt.Errorf("host for %s exited: %s", cfg.ID, lastLine(string(tail)))
		default:
		}
	}
	return cfg, fmt.Errorf("host for %s (pid %d) did not start listening", cfg.ID, pid)
}

// ShortID is the id a new session started on sessionID gets: its folder,
// socket and the name rush lists it by.
func ShortID(sessionID string) string { return shortOf(sessionID) }

func shortOf(sessionID string) string {
	var out []rune
	for _, r := range sessionID {
		if r == '-' {
			continue
		}
		out = append(out, r)
		if len(out) == 8 {
			break
		}
	}
	return string(out)
}

func lastLine(s string) string {
	for len(s) > 0 && (s[len(s)-1] == '\n' || s[len(s)-1] == ' ') {
		s = s[:len(s)-1]
	}
	for i := len(s) - 1; i >= 0; i-- {
		if s[i] == '\n' {
			return s[i+1:]
		}
	}
	return s
}

// ReadInfo reads a session's last published info. A session whose host has
// gone is reported stopped, unless it deliberately went to sleep.
func ReadInfo(id string) (Info, error) {
	info, err := readInfoFile(id)
	if err == nil && !alive(info.HostPID) {
		info.ClaudePID = 0
		info.Lost = midTurn(info)
		if !info.Sleeping {
			info.State = "stopped"
		}
	}
	return info, err
}

func readInfoFile(id string) (Info, error) {
	var info Info
	b, err := os.ReadFile(filepath.Join(dir(id), "info.json"))
	if err != nil {
		return info, err
	}
	err = jsonx.Unmarshal(b, &info)
	info.migrate()
	return info, err
}

// List returns every rush-mode session, newest first.
func List() []Info { return new(Lister).List() }

// Lister lists sessions again and again, parsing only the info files that
// changed; every session ever started stays listed, so most are unchanged.
type Lister struct {
	infos map[string]listed
}

type listed struct {
	mod  time.Time
	size int64
	info Info
	err  error
}

// List is List.
func (l *Lister) List() []Info {
	if l.infos == nil {
		l.infos = map[string]listed{}
	}
	ents, _ := os.ReadDir(Root())
	out := make([]Info, 0, len(ents)) // an Info is big: growing copies them
	for _, e := range ents {
		id := e.Name()
		st, err := os.Stat(filepath.Join(dir(id), "info.json"))
		if err != nil {
			delete(l.infos, id)
			continue
		}
		c, ok := l.infos[id]
		if !ok || !c.mod.Equal(st.ModTime()) || c.size != st.Size() {
			c = listed{mod: st.ModTime(), size: st.Size()}
			c.info, c.err = readInfoFile(id)
			l.infos[id] = c
		}
		if c.err != nil {
			continue
		}
		info := c.info
		if info.State != "stopped" && !alive(info.HostPID) {
			info.ClaudePID = 0
			info.Lost = midTurn(info)
			if !info.Sleeping {
				info.State = "stopped"
			}
		}
		out = append(out, info)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].UpdatedAt.After(out[j].UpdatedAt) })
	return out
}

// ReadConfig reads the config a session was started with, to restart it.
func ReadConfig(id string) (Config, error) {
	uithread.Forbid("host.ReadConfig")
	var cfg Config
	b, err := os.ReadFile(filepath.Join(dir(id), "config.json"))
	if err != nil {
		return cfg, err
	}
	return cfg, jsonx.Unmarshal(b, &cfg)
}

// migrate brings info an older host wrote up to date.
func (i *Info) migrate() {
	i.Kind = string(agent.Migrated(i.Kind)) // migration: older hosts wrote no kind for Claude Code
}

// InfoEvent is a session's info, sent on connect and whenever it changes.
type InfoEvent struct{ Info Info }

// Answered says a permission request was settled, here or by another client.
type Answered struct{ ID string }

// ErrorEvent reports a command the host could not carry out.
type ErrorEvent struct{ Error string }

// Sent is a message a client sent, echoed so every client shows it.
type Sent struct {
	Exchange *event.Exchange // nil for user input; carries agent origin for echo reconciliation
	Text     string
	Images   []string // names of attached images
}

// Stamp is when the output that follows it happened.
type Stamp struct{ At time.Time }

// Commands lists the session's slash commands.
type Commands struct{ Commands []event.Command }

// Reply answers a control request a client passed through with Ask: ID is
// the client's own.
type Reply struct {
	ID    string
	Body  jsontext.Value
	Error string
}

// Context is what fills the context window, as last counted.
type Context struct{ Usage usage.Context }

// Decode reads one line from a host: its own events, Claude Code's, or
// rush's own events from another agent's session (an event.Event).
func Decode(line []byte) (any, error) {
	// Claude Code's lines start with their type; the host's own (and its
	// echo of what you sent) are written with sorted keys and don't. So
	// most lines, deltas above all, are only taken apart once.
	if bytes.HasPrefix(line, []byte(`{"type":"`)) && !bytes.HasPrefix(line, []byte(`{"type":"agtop_`)) {
		return nativeLine(line)
	}
	var head struct {
		Type      string          `json:"type"`
		Info      Info            `json:"info"`
		RequestID string          `json:"request_id"`
		Error     string          `json:"error"`
		Sent      bool            `json:"agtop_sent"`
		Exchange  *event.Exchange `json:"agtop_exchange"`
		Images    []string        `json:"agtop_images"`
		Message   jsontext.Value  `json:"message"`
		Commands  []wireCommand   `json:"commands"`
		Context   *usage.Context  `json:"context"`
		ID        string          `json:"id"`
		Reply     jsontext.Value  `json:"reply"`
		T         int64           `json:"t"`
		Ev        jsontext.Value  `json:"ev"`
	}
	if err := jsonx.Unmarshal(line, &head); err != nil {
		return nil, err
	}
	switch head.Type {
	case typeInfo:
		head.Info.migrate()
		return InfoEvent{Info: head.Info}, nil
	case typeAnswered:
		return Answered{ID: head.RequestID}, nil
	case "agtop_error":
		return ErrorEvent{Error: head.Error}, nil
	case typeCommands:
		list := make([]event.Command, len(head.Commands))
		for i, c := range head.Commands {
			list[i] = event.Command(c)
		}
		return Commands{Commands: list}, nil
	case typeReply:
		return Reply{ID: head.ID, Body: head.Reply, Error: head.Error}, nil
	case typeContext:
		if head.Context == nil {
			return nil, errors.New("context line without a count")
		}
		return Context{Usage: *head.Context}, nil
	case typeTime:
		return Stamp{At: time.UnixMilli(head.T)}, nil
	case typeEvent:
		return event.Unmarshal(head.Ev)
	}
	if head.Sent {
		var m struct {
			Content string `json:"content"`
		}
		_ = jsonx.Unmarshal(head.Message, &m)
		return Sent{Text: m.Content, Images: head.Images, Exchange: head.Exchange}, nil
	}
	return nativeLine(line)
}

// Client is a connection to one session's host.
type Client struct {
	// Lines carries every line from the host, replay first; it closes when
	// the connection does.
	Lines <-chan []byte

	c   net.Conn
	wmu sync.Mutex
}

// Dial connects to a running session.
func Dial(id string) (*Client, error) { return dial(id, Proto) }

// DialRaw connects as a rush from before the hello: a Claude Code
// session's lines come as Claude Code wrote them.
func DialRaw(id string) (*Client, error) { return dial(id, 0) }

// dial connects saying hello as a client of protocol proto; 0 says none,
// as a rush from before the hello.
func dial(id string, proto int) (*Client, error) {
	c, err := net.Dial("unix", SockPath(id))
	if err != nil {
		return nil, err
	}
	lines := make(chan []byte, 1024)
	go func() {
		defer close(lines)
		r := jsonx.NewLineReader(c)
		for {
			l, ok := r.Next()
			if !ok {
				return
			}
			lines <- append([]byte(nil), l...)
		}
	}()
	cl := &Client{Lines: lines, c: c}
	if proto > 0 {
		if err := cl.do(op{Op: "hello", Proto: proto}); err != nil {
			_ = c.Close()
			return nil, err
		}
	}
	return cl, nil
}

func (c *Client) do(o op) error {
	b, err := jsonx.Marshal(o)
	if err != nil {
		return err
	}
	c.wmu.Lock()
	defer c.wmu.Unlock()
	_, err = c.c.Write(append(b, '\n'))
	return err
}

// Send delivers a message, or queues it if the agent is busy.
func (c *Client) Send(text string) error { return c.do(op{Op: "send", Text: text}) }

// SendNow delivers a message mid-turn; Claude reads it at its next step.
func (c *Client) SendNow(text string) error { return c.do(op{Op: "send", Text: text, Now: true}) }

// SendGuide hands a message to the turn under way without stopping it,
// where the agent can take one; else it's a Send.
func (c *Client) SendGuide(text string, paths []string) error {
	return c.do(op{Op: "send", Text: text, Images: paths, Guide: true})
}

// SendImages delivers a message with image files attached, or queues it
// with them if the agent is busy; now sends it mid-turn instead.
func (c *Client) SendImages(text string, paths []string, now bool) error {
	return c.do(op{Op: "send", Text: text, Images: paths, Now: now})
}

// Queue edits, by index into Info.Queue.
// The queue ops name a message by its place and its text as you saw it, so
// they still find it if the queue moved in between.

func (c *Client) EditQueued(i int, was, text string) error {
	return c.do(op{Op: "queue_edit", Index: i, Was: was, Text: text})
}
func (c *Client) RemoveQueued(i int, was string) error {
	return c.do(op{Op: "queue_remove", Index: i, Was: was})
}
func (c *Client) MoveQueued(i int, was string, to int) error {
	return c.do(op{Op: "queue_move", Index: i, Was: was, To: to})
}
func (c *Client) MergeQueued(i int, was string) error {
	return c.do(op{Op: "queue_merge", Index: i, Was: was})
}
func (c *Client) SendQueued(i int, was string) error {
	return c.do(op{Op: "queue_send", Index: i, Was: was})
}

// SteerQueued hands queued message i to the turn under way without
// stopping it, where the agent can take one (Proto 7).
func (c *Client) SteerQueued(i int, was string) error {
	return c.do(op{Op: "queue_send", Index: i, Was: was, Guide: true})
}

// HoldQueue pauses or resumes sending the queue.
func (c *Client) HoldQueue(on bool) error { return c.do(op{Op: "queue_hold", Now: on}) }

// QueueSeparately sends queued messages one per turn instead of together.
func (c *Client) QueueSeparately(on bool) error { return c.do(op{Op: "queue_separate", Now: on}) }

// Allow lets a pending tool call run; input nil keeps the requested input.
func (c *Client) Allow(id string, input jsontext.Value, always bool) error {
	return c.do(op{Op: "allow", ID: id, Input: input, Always: always})
}

func (c *Client) Deny(id, message string, interrupt bool) error {
	return c.do(op{Op: "deny", ID: id, Message: message, Interrupt: interrupt})
}

func (c *Client) Interrupt() error { return c.do(op{Op: "interrupt"}) }

// StopTask stops one subagent or background shell by its task id, and
// nothing else.
func (c *Client) StopTask(id string) error         { return c.do(op{Op: "stop_task", ID: id}) }
func (c *Client) SetPermissionMode(m string) error { return c.do(op{Op: "mode", Mode: m}) }

// Background moves a tool call the turn is waiting on (a Bash command or a
// subagent) into the background by its tool_use id; empty moves them all.
// Hosts before Proto 3 ignore it.
func (c *Client) Background(toolUseID string) error { return c.do(op{Op: "background", ID: toolUseID}) }
func (c *Client) SetModel(m string) error           { return c.do(op{Op: "model", Model: m}) }

// Relogin tells the session ~/.claude is now signed in as another account.
func (c *Client) Relogin() error { return c.do(op{Op: "relogin"}) }

// ContinueAtReset says whether a session stopped by a usage limit carries
// on when the limit resets.
func (c *Client) ContinueAtReset(yes bool) error { return c.do(op{Op: "limit", Now: yes}) }

// SetEffort changes effort; it applies from the next Claude Code start.
func (c *Client) SetEffort(e string) error { return c.do(op{Op: "effort", Effort: e}) }

// SetWithout says what the session goes without from its agent's next
// start: at once when it's idle.
func (c *Client) SetWithout(w []string) error { return c.do(op{Op: "without", Without: w}) }

// Rewind carries on from another conversation, sessionID: a copy cut
// before one of your messages (resume), a fresh one, or one of its
// Branches. The path it leaves is kept as a branch, as left describes it.
// The connection closes once it has; dial again to see it.
func (c *Client) Rewind(sessionID string, resume bool, left Branch) error {
	return c.do(op{Op: "rewind", Text: sessionID, Now: resume, Branch: &left})
}

// Tell leaves text for running subagent sub, which it's given at its next
// tool call, straight, not through the main session.
func (c *Client) Tell(sub, text string) error { return c.do(op{Op: "tell", ID: sub, Text: text}) }

// Compacted carries the session on in a fresh conversation, sessionID,
// that starts with prompt: a summary another model wrote of this one.
func (c *Client) Compacted(sessionID, prompt string, left Branch) error {
	return c.do(op{Op: "compacted", Text: sessionID, Message: prompt, Branch: &left})
}

// CompactedIfUnchanged replaces only the idle conversation used to make the
// summary. Protocol 9 checks the snapshot under the host's mutex.
func (c *Client) CompactedIfUnchanged(sessionID, prompt string, left Branch, expected Info) error {
	if expected.Proto < 9 {
		return errors.New("restart this session's host to use external compaction safely")
	}
	return c.do(op{Op: "compacted_checked", Text: sessionID, Message: prompt, Branch: &left,
		ExpectedSession: expected.SessionID, ExpectedUpdatedAt: expected.UpdatedAt})
}

// Restart ends a session's host and starts it again on this rush's
// binary, with change applied to its config first: how a host from an
// older rush gets what's new. The session must be idle.
func Restart(id string, change func(*Config)) error {
	cfg, err := ReadConfig(id)
	if err != nil {
		return err
	}
	info, _ := ReadInfo(id)
	if c, err := Dial(id); err == nil {
		_ = c.Stop()
		c.Close()
	}
	for deadline := time.Now().Add(15 * time.Second); alive(info.HostPID); time.Sleep(50 * time.Millisecond) {
		if time.Now().After(deadline) {
			return fmt.Errorf("the host for %s didn't stop", id)
		}
	}
	change(&cfg)
	cfg.Prompt, cfg.Images = "", nil
	_, err = Spawn(cfg)
	return err
}

// RewindByRestart is Rewind for a host from before it could (Proto 0):
// the host is restarted on this rush, already carrying on from
// sessionID, with the path it leaves kept as a branch.
func RewindByRestart(id, sessionID string, resume bool, left Branch) error {
	return Restart(id, func(cfg *Config) {
		kept := false
		if b, ok := agent.As[agent.Brancher](agent.Kind(cfg.Kind)); ok {
			_, err := os.Stat(b.TranscriptPath(cfg.Account, cfg.Cwd, cfg.SessionID))
			kept = err == nil
		}
		cfg.rewindTo(sessionID, resume, &left, kept)
	})
}

// Stop ends the session and its host; the conversation is kept.
func (c *Client) Stop() error { return c.do(op{Op: "stop"}) }

// Ask passes a control request to Claude Code (req carries its subtype:
// side_question, export_conversation, …), waking it if it's asleep. The
// answer arrives on Lines as a Reply with this id. Hosts before Proto 2
// ignore it.
func (c *Client) Ask(id string, req any) error {
	b, err := jsonx.Marshal(req)
	if err != nil {
		return err
	}
	return c.do(op{Op: "ask", ID: id, Request: b})
}

// AskContext asks for a fresh count of what fills the context window; it
// arrives on Lines as a Context. Hosts before Proto 2 ignore it.
func (c *Client) AskContext() error { return c.do(op{Op: "context"}) }

// Close ends the connection; it's safe on a nil client.
func (c *Client) Close() error {
	if c == nil || c.c == nil {
		return nil
	}
	return c.c.Close()
}

// SendExchange sends an agent-origin message, retaining origin through queues.
func (c *Client) SendExchange(e event.Exchange, now bool) error {
	return c.do(op{Op: "send", Text: e.Text, Images: e.Images, Now: now, Guide: !now, Exchange: &e})
}

// RecordExchange records the other side of an exchange without prompting it.
func (c *Client) RecordExchange(e event.Exchange) error {
	return c.do(op{Op: "exchange", Exchange: &e})
}
