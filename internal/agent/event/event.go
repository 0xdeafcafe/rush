// Package event is what a running session says, whichever agent runs it:
// the one stream the host keeps, replays and sends to everything that
// draws a session. Adapters turn their agent's protocol into these; a
// transcript read back from disk turns into the same ones.
package event

import (
	"encoding/json/jsontext"
	"slices"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// Event is one thing a session said or did.
type Event interface{ event() }

// Init arrives once the session is ready.
type PermissionMode struct {
	ID          string `json:"id"`
	Name        string `json:"name,omitempty"`
	Description string `json:"description,omitempty"`
}

type Init struct {
	ModeID    string           // stable permission mode ID, when distinct from its display name
	Modes     []PermissionMode // choices reported by the running harness
	SessionID string
	Model     string
	Cwd       string
	Mode      string // its permission mode, in the agent's words
	Effort    string // how hard it thinks, when the agent says
	Version   string // the agent's
	Tools     []string
	Commands  []string // slash commands it takes, by name: Commands has more
	MCP       []MCPServer
}

// MCPServer is one MCP server as the agent last saw it: connected,
// failed, needs-auth, pending.
type MCPServer struct {
	Name   string `json:"name"`
	Status string `json:"status"`
}

// PartKind is what a part of a message is.
type PartKind int

const (
	Text PartKind = iota
	Thinking
	ToolCall
	ToolResult
	Image
)

// MessageStart opens an assistant message that Deltas then fill in.
type MessageStart struct {
	ID    string
	Model string
}

// PartStart opens part Index of the message being written. Thinking
// often streams nothing readable, so this is how to know it's happening.
type PartStart struct {
	Index int
	Kind  PartKind
}

// Delta is a piece of part Index as it's written: text, thinking, or a
// tool call's input as JSON.
type Delta struct {
	Index int
	Kind  PartKind
	Text  string
}

// Message is a whole message. Tool results come back as user messages.
// Parent is the tool call of the subagent that wrote it, if one did. A
// user message of text is something you said, unless it's Injected: put
// in the conversation as yours by the agent itself (a task's report, the
// mark an interrupt leaves, a command's output).
type Message struct {
	Role     string // user, assistant
	ID       string
	Model    string
	Parent   string
	Parts    []Part
	Tokens   *usage.TokenUsage
	Injected bool `json:",omitzero"`
}

// Part is one part of a Message.
type Part struct {
	Kind   PartKind
	Text   string       // Text, Thinking
	Call   *tool.Call   // ToolCall
	Output *tool.Output // ToolResult
	Image  *ImageData   // Image
}

// ImageData is an image in a message.
type ImageData struct {
	MediaType string
	Data      []byte
	Path      string // where rush keeps a copy, when it does
}

// CallUpdated is a tool call already sent, as it now reads: agents that
// announce a call before its input is known fill it in later.
type CallUpdated struct{ Call tool.Call }

// Approval asks whether a tool call may run. Options are the answers the
// agent takes; the host answers with one of their IDs.
type Approval struct {
	ID      string
	Call    tool.Call
	Reason  string // why the agent asks, when it says
	Path    string // the file outside the workspace that made it ask, if one did
	Options []Option
}

// OptionKind is what an Option does.
type OptionKind int

const (
	AllowOnce OptionKind = iota
	AllowAlways
	RejectOnce
	RejectAlways
)

// Option is one answer to an Approval.
type Option struct {
	ID    string
	Label string
	Kind  OptionKind
}

// ApprovalCancelled withdraws an Approval or a Question, e.g. after an
// interrupt.
type ApprovalCancelled struct{ ID string }

// Denied reports a tool call refused without asking.
type Denied struct {
	CallID string
	Tool   string
	Reason string
}

// Question asks the user to choose, as Claude's AskUserQuestion and
// Codex's request_user_input do.
type Question struct {
	ID     string
	CallID string
	Title  string
	Asks   []Ask
}

// Ask is one question of a Question.
type Ask struct {
	ID      string // the agent's key for the answer, when it isn't Text
	Header  string
	Text    string
	Multi   bool
	Options []Choice
}

// Choice is one option of an Ask.
type Choice struct {
	Label       string
	Description string
	Preview     string
	// Key and Picked are a decisions block's: the option's letter, and
	// whether the agent picked it.
	Key    string `json:",omitempty"`
	Picked bool   `json:",omitzero"`
}

// Status is the session's own busy marker.
type Status struct {
	Busy bool
	Text string // "requesting", "compacting"
}

// TurnEnd ends a turn.
type TurnEnd struct {
	Reason   string // done, interrupted, max_turns, error, ...
	Err      string // what went wrong, if something did
	Text     string // the turn's last words, as the agent sums it up
	Cost     float64
	Tokens   usage.TokenUsage
	Duration time.Duration
	Turns    int
}

// Compacted marks where the conversation was compacted: what set it off
// (manual, auto) and the context before and after.
type Compacted struct {
	Trigger       string
	Before, After int
}

// Quota is a fresh reading of the account's limits.
type Quota struct{ usage.Quota }

// Billing is how the session's requests are paid for, once the agent says
// or it changes: an allowance runs out into extra usage.
type Billing struct{ Billing usage.Billing }

// Limited says a limit stopped the session until ResetsAt.
type Limited struct {
	Window   string // the window's ID, if known
	ResetsAt time.Time
}

// Retry says a request to the model failed and is tried again after
// Delay: attempt Attempt of Max, for Status (529 overloaded, 429 rate
// limited, 0 when there was no answer) and Err.
type Retry struct {
	Attempt, Max int
	Delay        time.Duration
	Status       int
	Err          string
}

// Context is how much of the model's context the last request used.
type Context struct {
	Tokens, Window int
}

// StartNotice is context the harness gave the agent as its session began.
// It is shown compactly above the first turn, as Claude Code's start hooks are.
type StartNotice struct {
	Event string
	Text  string
}

// TaskKind is what a task beside the turn is.
type TaskKind int

const (
	ShellTask TaskKind = iota
	SubagentTask
	MonitorTask
	WorkflowTask
	OtherTask
)

// TaskStarted says the agent started work beside the turn: a shell, a
// subagent, a monitor. Background is false for one the turn waits on.
type TaskStarted struct {
	ID         string
	CallID     string
	Kind       TaskKind
	Label      string
	Agent      string // a subagent's type
	Background bool
}

// TaskUpdated is what changed about a task. Status is set when it ends
// (completed, failed, killed).
type TaskUpdated struct {
	ID         string
	Status     string
	Label      string
	Background *bool
	Err        string
}

// TaskProgress is a subagent's or a workflow's running numbers.
type TaskProgress struct {
	ID       string
	Summary  string
	LastTool string
	Tokens   int
	ToolUses int
	// Phases and Agents are a workflow's, as far as this progress tells:
	// each agent once, the latest word of it.
	Phases []string        `json:",omitempty"`
	Agents []WorkflowAgent `json:",omitempty"`
}

// WorkflowAgent is one agent a workflow runs.
type WorkflowAgent struct {
	Index int    // its place in the run, from 1
	Label string // what the workflow calls it: "port:draw"
	Phase string
	ID    string // its agent id, which names its transcript
	Model string
	// State is queued, running, done or failed.
	State     string
	LastTool  string `json:",omitempty"`
	Doing     string `json:",omitempty"` // what its last tool was given, in short
	Result    string `json:",omitempty"` // the start of what it gave back
	Error     string `json:",omitempty"`
	Tokens    int    `json:",omitempty"`
	ToolCalls int    `json:",omitempty"`
	Start     time.Time
	Took      time.Duration `json:",omitempty"`
}

// MergeAgents is agents with more's word of each added: an agent is known
// by its index, and a later word says more of it without unsaying what an
// earlier one said.
func MergeAgents(agents []WorkflowAgent, more ...WorkflowAgent) []WorkflowAgent {
	for _, b := range more {
		i := slices.IndexFunc(agents, func(a WorkflowAgent) bool { return a.Index == b.Index })
		if i < 0 {
			agents = append(agents, b)
			continue
		}
		a := &agents[i]
		a.Label, a.Phase, a.ID, a.Model = or(b.Label, a.Label), or(b.Phase, a.Phase), or(b.ID, a.ID), or(b.Model, a.Model)
		a.State, a.LastTool, a.Doing = or(b.State, a.State), or(b.LastTool, a.LastTool), or(b.Doing, a.Doing)
		a.Result, a.Error = or(b.Result, a.Result), or(b.Error, a.Error)
		a.Tokens, a.ToolCalls = max(a.Tokens, b.Tokens), max(a.ToolCalls, b.ToolCalls)
		if !b.Start.IsZero() {
			a.Start = b.Start
		}
		if b.Took > 0 {
			a.Took = b.Took
		}
	}
	return agents
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

// TaskDone says a task finished, and where its output is.
type TaskDone struct {
	ID         string
	CallID     string
	Status     string
	OutputFile string
	Summary    string
}

// Background is every task now running beside the turn, each time the
// list changes.
type Background struct{ Tasks []BackgroundTask }

// BackgroundTask is one task of a Background list.
type BackgroundTask struct {
	ID    string
	Kind  TaskKind
	Type  string // the agent's own word for it: local_bash, local_agent, ...
	Label string
	// Parent is the task of the subagent that started it, if one did.
	Parent string
}

// Commands are the session's slash commands and skills, once it says them.
type Commands struct{ List []Command }

// Command is one of a session's slash commands.
type Command struct {
	Name         string
	Description  string
	ArgumentHint string
	Aliases      []string
}

// Plan is the agent's whole plan or todo list, each time it changes.
type Plan struct{ Todos []tool.TodoItem }

// Other is anything an adapter doesn't turn into one of the above, kept
// whole so nothing is lost.
type Other struct {
	Adapter string
	Type    string
	Raw     jsontext.Value `json:",omitzero"`
}

func (Init) event()              {}
func (MessageStart) event()      {}
func (PartStart) event()         {}
func (Delta) event()             {}
func (Message) event()           {}
func (CallUpdated) event()       {}
func (Approval) event()          {}
func (ApprovalCancelled) event() {}
func (Denied) event()            {}
func (Question) event()          {}
func (Status) event()            {}
func (TurnEnd) event()           {}
func (Compacted) event()         {}
func (Quota) event()             {}
func (Limited) event()           {}
func (Billing) event()           {}
func (Context) event()           {}
func (StartNotice) event()       {}
func (Retry) event()             {}
func (TaskStarted) event()       {}
func (TaskUpdated) event()       {}
func (TaskProgress) event()      {}
func (TaskDone) event()          {}
func (Plan) event()              {}
func (Commands) event()          {}
func (Background) event()        {}
func (Other) event()             {}
