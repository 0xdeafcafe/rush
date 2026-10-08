package codex

import (
	"encoding/json/jsontext"
	"errors"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// A thread's spawn_agent children run in its app-server, which tells rush
// of their turns too: each is a task beside the turn, as Claude Code's
// background subagents are, that rush can stop. Codex takes no input for
// one but through its parent ("direct app-server input is not allowed").

var _ agent.TaskStopper = (*Conn)(nil)

// kid is a spawned thread: the call that spawned it, its task, and the
// turn it's running ("" when it's idle).
type kid struct {
	call, label, turn string
	busy              bool
	tokens            int64
	tools             map[string]bool
}

// adopt takes thread, spawned by call, on as a task.
func (c *Conn) adopt(thread, call, label string) []event.Event {
	if thread == "" {
		return nil
	}
	c.mu.Lock()
	k := c.kid(thread)
	k.call, k.label, k.busy, k.tokens, k.tools = call, label, true, 0, map[string]bool{}
	c.mu.Unlock()
	return []event.Event{event.TaskStarted{ID: thread, CallID: call, Kind: event.SubagentTask, Label: label, Background: true}, c.background()}
}

// kid is thread's, made when it's new. Called with mu held.
func (c *Conn) kid(thread string) *kid {
	if c.kids == nil {
		c.kids = map[string]*kid{}
	}
	k := c.kids[thread]
	if k == nil {
		k = &kid{}
		c.kids[thread] = k
	}
	return k
}

// kidTurn is what a turn of another thread starting or ending means: a
// task starting again, or ending. Threads it didn't spawn mean nothing.
func (c *Conn) kidTurn(method, thread, turn, status string) []event.Event {
	c.mu.Lock()
	k := c.kid(thread)
	was := k.busy
	switch method {
	case "turn/started":
		k.turn, k.busy = turn, true
		if !was {
			k.tokens, k.tools = 0, map[string]bool{}
		}
	case "turn/completed":
		k.turn, k.busy = "", false
	default:
		c.mu.Unlock()
		return nil
	}
	call, label, busy := k.call, k.label, k.busy
	c.mu.Unlock()
	switch {
	case call == "" || was == busy:
		return nil
	case busy:
		return []event.Event{event.TaskStarted{ID: thread, CallID: call, Kind: event.SubagentTask, Label: label, Background: true}, c.background()}
	}
	return []event.Event{event.TaskDone{ID: thread, CallID: call, Status: taskStatus(status)}, c.background()}
}

// kidTokens and kidTool turn Codex's child-thread traffic into the same
// progress figures Claude reports for its native subagents.
func (c *Conn) kidTokens(thread string, u usage.TokenUsage) []event.Event {
	c.mu.Lock()
	k := c.kid(thread)
	if k.call == "" || !k.busy {
		c.mu.Unlock()
		return nil
	}
	k.tokens += u.Input + u.CacheRead + u.CacheWrite5m + u.CacheWrite1h
	p := event.TaskProgress{ID: thread, Tokens: int(k.tokens), ToolUses: len(k.tools)}
	c.mu.Unlock()
	return []event.Event{p}
}

func (c *Conn) kidTool(thread string, raw jsontext.Value) []event.Event {
	var it threadItem
	if jsonx.Unmarshal(raw, &it) != nil {
		return nil
	}
	call, ok := callOf(it, raw)
	if !ok {
		return nil
	}
	c.mu.Lock()
	k := c.kid(thread)
	if k.call == "" || !k.busy {
		c.mu.Unlock()
		return nil
	}
	if k.tools == nil {
		k.tools = map[string]bool{}
	}
	k.tools[it.ID] = true
	p := event.TaskProgress{ID: thread, LastTool: call.Name, Tokens: int(k.tokens), ToolUses: len(k.tools)}
	c.mu.Unlock()
	return []event.Event{p}
}

// taskStatus is a turn's status as a task's.
func taskStatus(s string) string {
	switch s {
	case "interrupted":
		return "stopped"
	case "failed":
		return "failed"
	}
	return "completed"
}

// background lists the spawned threads and background shells running now.
func (c *Conn) background() event.Background {
	c.mu.Lock()
	defer c.mu.Unlock()
	var tasks []event.BackgroundTask
	for id, k := range c.kids {
		if k.busy && k.call != "" {
			tasks = append(tasks, event.BackgroundTask{ID: id, Kind: event.SubagentTask, Label: k.label})
		}
	}
	for id, s := range c.shells {
		if s.bg {
			tasks = append(tasks, event.BackgroundTask{ID: id, Kind: event.ShellTask, Label: c.calls[id].Input.Command})
		}
	}
	return event.Background{Tasks: tasks}
}

// child is the spawned thread id names (its thread, or the call that
// spawned it), and the turn it's running.
func (c *Conn) child(id string) (thread, turn string, ok bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for t, k := range c.kids {
		if k.call != "" && (t == id || k.call == id) {
			return t, k.turn, true
		}
	}
	return "", "", false
}

// StopTask interrupts the turn a spawned thread is running, or ends a
// background shell's terminal.
func (c *Conn) StopTask(id string) error {
	c.mu.Lock()
	thread, process, isShell := c.shellOf(id)
	c.mu.Unlock()
	if isShell {
		return c.rpc.call(c.ctx, "thread/backgroundTerminals/terminate", map[string]any{"threadId": thread, "processId": process}, nil)
	}
	thread, turn, ok := c.child(id)
	if !ok {
		return errors.New("codex: no subagent or shell " + id)
	}
	if turn == "" {
		return nil // done already
	}
	return c.rpc.call(c.ctx, "turn/interrupt", map[string]any{"threadId": thread, "turnId": turn}, nil)
}
