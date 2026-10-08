// Package agent is rush's own idea of a coding agent and its work, apart
// from any one agent. Claude Code, Codex, Copilot and the rest are adapters
// that fill these types in; nothing here knows which one it came from.
// docs/multi-agent.md has the whole plan.
package agent

import (
	"time"

	"github.com/0xdeafcafe/photon/uithread"
)

// Todo is one item of an agent's plan or todo list.
type Todo struct {
	Label         string
	Done, Started bool
}

// Task is something an agent has running beside its turn: a subagent, a
// shell or a monitor.
type Task struct {
	Kind      string // agent, shell, monitor
	Label     string
	StartedAt time.Time
}

// PR is a pull request a session opened or pushed to.
type PR struct {
	URL    string `json:"-"`
	Title  string `json:"title"`
	Number int    `json:"number"`
	State  string `json:"state"`
	Review string `json:"review"`
	Checks struct {
		Passed  int `json:"passed"`
		Failed  int `json:"failed"`
		Pending int `json:"pending"`
	} `json:"checks"`
}

// SubagentStats counts the subagents a session started: Direct ones it
// started itself, Nested ones its subagents started.
type SubagentStats struct {
	Spawned, Direct, Nested int
}

// Preview is what a session is doing and has lately said, read cheaply
// from the end of its transcript.
type Preview struct {
	Text     string
	Tool     string
	ToolArg  string
	Doing    string // the tool call in words: "running pnpm test"
	At       time.Time
	Model    string
	Effort   string // how hard it was thinking, when the transcript says
	LastUser string
	Context  int64 // tokens the last request sent: how full the context window is
	Recent   []Line
	First    Line // the message that started the conversation
}

// Previewer is an agent that can read a Preview from the last window
// bytes of a session's transcript at path.
type Previewer interface {
	Preview(path string, window int64) Preview
}

// ReadPreview is agent k's Preview of the transcript at path; empty when
// k can't read one. It reads the disk, so never on the UI.
func ReadPreview(k Kind, path string, window int64) Preview {
	uithread.Forbid("agent.ReadPreview")
	if pv, ok := As[Previewer](k); ok {
		return pv.Preview(path, window)
	}
	return Preview{}
}

// Line is one message of a Preview.
type Line struct {
	Role string // user, assistant, tool
	Text string
	At   time.Time
}

// Convo is a past conversation, known from its transcript.
type Convo struct {
	SessionID string
	Cwd       string
	// Title is its rename, or else the title the agent gave it, or else
	// its first prompt.
	Title   string
	Started time.Time
}

// Command is a slash command or skill a session can run.
type Command struct {
	Name         string
	Description  string
	ArgumentHint string
	Skill        bool
	Path         string // its SKILL.md or command file
	Source       string // yours, project, the agent's cloud, or the plugin's name
	Size         int64  // of the file: roughly what using it adds
}

// Codenamer is an agent whose sessions have names for each other, the
// ones their messages to one another use ("rush-8a"), by session id.
type Codenamer interface {
	Codenames() map[string]string
}
