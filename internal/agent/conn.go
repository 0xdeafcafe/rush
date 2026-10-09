package agent

import (
	"context"
	"encoding/json/jsontext"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// What a session rush hosts can do beyond Conn: each is an optional
// interface a Conn has only when its agent can, found with a type
// assertion. The host offers an op only to a session whose Conn has it.

// ToolServer is an MCP server rush serves in process for a session: its
// own drawing tools, or an approved plugin's.
type ToolServer struct {
	Name string
	// Trusted are its tools that never ask, by their own name: rush's
	// own only draw.
	Trusted []string
	// Handle answers one JSON-RPC message. It may take a while; the
	// session carries on meanwhile.
	Handle func(msg jsontext.Value) jsontext.Value
	// Args run it as a process instead, for an agent that only runs its
	// MCP servers so: rush's own executable with these.
	Args []string
}

// Responder is a Conn that takes an approval's answer with more than an
// option: the call's input as you changed it, in the agent's own words,
// and a refusal's message, and whether a refusal stops the turn.
type Responder interface {
	Allow(approvalID string, input jsontext.Value, always bool) error
	Deny(approvalID, message string, interrupt bool) error
}

// Asker is a Conn that passes a request in the agent's own control
// protocol through for a client, and waits for its reply.
type Asker interface {
	Ask(ctx context.Context, req jsontext.Value) (jsontext.Value, error)
}

// ContextReader is a Conn that breaks down what fills the context window,
// as the agent counts it.
type ContextReader interface {
	ContextUsage(ctx context.Context) (usage.Context, error)
}

// TaskStopper is a Conn that stops one task beside the turn, leaving the
// turn and the rest running.
type TaskStopper interface {
	StopTask(id string) error
}

// Backgrounder is a Conn that moves a call the turn is waiting on into the
// background, so the turn carries on; an empty id moves every one.
type Backgrounder interface {
	Background(callID string) error
}

// Staler is a Conn whose session holds a sign-in its profile has since
// left: it should rest once its turn ends, and pick up on the new one.
type Staler interface {
	Stale() bool
}

// Homed is a Conn that runs in the home of a login, which a switch can
// leave behind. Home is that home; Moved is whether new sessions run in
// another now. A moved session needn't follow while its prompt cache is
// warm: the new login's cache is empty, so its next turn would write the
// whole context again.
type Homed interface {
	Home() string
	Moved() bool
}

// QuotaKeeper is a Conn that keeps its own quota readings where its agent
// reads them: the host doesn't record its event.Quota again.
type QuotaKeeper interface {
	KeepsQuota()
}

// Ender is a Conn that says why its session ended, once Events closes.
type Ender interface {
	Err() error
}

// PIDer is a Conn whose agent is a process of its own.
type PIDer interface {
	PID() int
}

// Tapper is a Conn whose agent's lines reach StartOptions.Tap: the caller
// has them there, and needn't keep its events too.
type Tapper interface {
	Taps() bool
}

// Describer is an adapter with words of its own for what its tools do.
type Describer interface {
	Doing(c *tool.Call) string
}

// Doing is a call in a few words ("reading view.go"), as agent k says it
// where it has words of its own for its tools, else as tool.Doing does.
func Doing(k Kind, c *tool.Call) string {
	if a, ok := Get(k); ok {
		if d, ok := a.(Describer); ok {
			return d.Doing(c)
		}
	}
	return tool.Doing(*c)
}

// CwdReader is an agent whose session can move to another folder as it
// runs, into a worktree, and says where: the session its process pid, of
// profile p, runs, and the folder it works in now. It reads the disk.
type CwdReader interface {
	SessionCwd(p Profile, pid int) (sessionID, cwd string, ok bool)
}
