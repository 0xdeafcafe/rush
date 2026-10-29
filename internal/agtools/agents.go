package agtools

import (
	"encoding/json/jsontext"
	"os"
	"slices"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// The agent tools let a session hand work to any agent rush runs, of any
// harness: spawn_agent starts one as a rush session of its own, the
// caller's child; start_agent starts one beside the caller, a session in
// the list like one you made; agent_result and agent_send follow either
// up. See docs/subagents.md.

// The agent tools' own names.
const (
	SpawnAgent  = "spawn_agent"
	StartAgent  = "start_agent"
	AgentResult = "agent_result"
	AgentSend   = "agent_send"
)

// SpawnInput is what a session sends spawn_agent.
type SpawnInput struct {
	Agent      string `json:"agent"`
	Prompt     string `json:"prompt"`
	Model      string `json:"model,omitempty"`
	Effort     string `json:"effort,omitempty"`
	Background bool   `json:"background,omitempty"`
	// Step is the tool call that asked, when the harness says which
	// (Claude's _meta claudecode/toolUseId): the UI draws the child under it.
	Step string `json:"-"`
}

// StartInput is what a session sends start_agent.
type StartInput struct {
	Agent  string `json:"agent"`
	Prompt string `json:"prompt"`
	Model  string `json:"model,omitempty"`
	Effort string `json:"effort,omitempty"`
}

// ResultInput is what a session sends agent_result.
type ResultInput struct {
	ID   string `json:"id"`
	Wait int    `json:"wait_seconds,omitempty"`
}

// SendInput is what a session sends agent_send.
type SendInput struct {
	ID         string `json:"id"`
	Prompt     string `json:"prompt"`
	Background bool   `json:"background,omitempty"`
}

// Agents runs the agent tools for one session.
type Agents interface {
	// Catalogue is the agents spawn_agent can start, a line each.
	Catalogue() string
	Spawn(in SpawnInput) (string, error)
	Start(in StartInput) (string, error)
	Result(in ResultInput) (string, error)
	Send(in SendInput) (string, error)
}

// IsSpawn is whether a tool call named name is spawn_agent, by any
// harness's name for it: mcp__rush__spawn_agent, rush.spawn_agent.
func IsSpawn(name string) bool {
	return name == SpawnAgent || strings.HasSuffix(name, "_"+SpawnAgent) || strings.HasSuffix(name, "."+SpawnAgent) ||
		strings.HasSuffix(name, "/"+SpawnAgent)
}

// ReadSpawn reads a spawn_agent call's input, as the agent sent it.
func ReadSpawn(raw jsontext.Value) (SpawnInput, bool) {
	var in SpawnInput
	if len(raw) == 0 || jsonx.Unmarshal(raw, &in) != nil || in.Agent == "" {
		return SpawnInput{}, false
	}
	return in, true
}

var agentTools = []tool{{
	Name: SpawnAgent,
	Description: "Hand a task to another agent, of any provider and harness rush runs, as your subagent. It runs as a " +
		"session of its own, which the user sees under yours with its every step. By default the call waits and returns " +
		"the agent's final answer. With background, it returns the agent's id at once; you are sent its answer as a " +
		"message when it finishes, or can ask with agent_result. Give it the whole task with all the context it needs: " +
		"it can't see your conversation. Pick an agent by what the work needs: a cheaper or faster model for routine " +
		"work, another provider for a second opinion. This is how you reach other providers' models (OpenAI's in Codex, " +
		"Kimi, GLM, DeepSeek, Gemini, Ollama and the rest): when you're asked to \"use astra\" or \"have sol review it\", " +
		"call this tool rather than looking for another way. Name an agent as listed, or just its model, harness or both " +
		"(\"gpt-6.1-sol\", \"sol 6.1\", \"codex\", \"codex astra\"): rush finds it, and if nothing fits it says what does. " +
		"If one can't start, use another and tell the user; never debug its sign-in. For an agent that should be a " +
		"session of its own in the user's list, not your subagent, use start_agent.",
	InputSchema: map[string]any{
		"type": "object",
		"properties": map[string]any{
			"agent":      map[string]any{"type": "string", "description": "Which agent: a name listed, or a model, harness or provider by its name (\"sol 6.1\", \"codex\")."},
			"prompt":     map[string]any{"type": "string", "description": "The task, in full."},
			"model":      map[string]any{"type": "string", "description": "A model of that agent's other than the one its name picks. Rarely needed."},
			"effort":     map[string]any{"type": "string", "description": "Reasoning effort, for a model that takes one: low, medium, high."},
			"background": map[string]any{"type": "boolean", "description": "Return at once; its answer comes as a message when it's done."},
		},
		"required":             []string{"agent", "prompt"},
		"additionalProperties": false,
	},
	Meta: map[string]any{"anthropic/alwaysLoad": true},
}, {
	Name: StartAgent,
	Description: "Start another agent as a session of its own, beside yours in the user's list, as if the user had " +
		"started it: not your subagent. It isn't stopped with you and its answer isn't sent to you. Use it when the " +
		"user asks for a new session, or for work that should carry on apart from yours; for work you want back, use " +
		"spawn_agent. Returns its id at once: agent_send messages it, agent_result asks how it stands. Any agent " +
		"spawn_agent lists, named the same way, with model and effort as there. Give it the whole task: it can't see " +
		"your conversation.",
	InputSchema: map[string]any{
		"type": "object",
		"properties": map[string]any{
			"agent":  map[string]any{"type": "string", "description": "Which agent: a name spawn_agent lists, or a model, harness or provider by its name (\"sol 6.1\", \"codex\")."},
			"prompt": map[string]any{"type": "string", "description": "The task, in full."},
			"model":  map[string]any{"type": "string", "description": "A model of that agent's other than the one its name picks. Rarely needed."},
			"effort": map[string]any{"type": "string", "description": "Reasoning effort, for a model that takes one: low, medium, high."},
		},
		"required":             []string{"agent", "prompt"},
		"additionalProperties": false,
	},
}, {
	Name: AgentResult,
	Description: "How an agent you started with spawn_agent or start_agent stands, and its answer once it has finished. wait_seconds " +
		"waits up to that long for it to finish (at most 600); 0 answers at once.",
	InputSchema: map[string]any{
		"type": "object",
		"properties": map[string]any{
			"id":           map[string]any{"type": "string", "description": "The agent's id, as spawn_agent or start_agent returned it."},
			"wait_seconds": map[string]any{"type": "integer", "description": "How long to wait for it to finish."},
		},
		"required":             []string{"id"},
		"additionalProperties": false,
	},
}, {
	Name: AgentSend,
	Description: "Send a follow-up to an agent you started with spawn_agent or start_agent: it carries on with its own conversation. " +
		"Waits for its answer unless background.",
	InputSchema: map[string]any{
		"type": "object",
		"properties": map[string]any{
			"id":         map[string]any{"type": "string", "description": "The agent's id, as spawn_agent or start_agent returned it."},
			"prompt":     map[string]any{"type": "string", "description": "What to tell it."},
			"background": map[string]any{"type": "boolean", "description": "Return at once; its answer comes as a message."},
		},
		"required":             []string{"id", "prompt"},
		"additionalProperties": false,
	},
}}

// agentList is the agent tools, spawn_agent's description naming the
// agents ag can start.
func agentList(ag Agents) []tool {
	out := slices.Clone(agentTools)
	if c := strings.TrimSpace(ag.Catalogue()); c != "" {
		out[0].Description += "\n\nAgents:\n" + c
	}
	return out
}

// callAgent runs an agent tool; false when name is none of them.
func callAgent(ag Agents, name string, args jsontext.Value, step string) (map[string]any, bool) {
	if ag == nil {
		return nil, false
	}
	var text string
	var err error
	switch name {
	case SpawnAgent:
		var in SpawnInput
		if err = jsonx.Unmarshal(args, &in); err == nil {
			in.Step = step
			text, err = ag.Spawn(in)
		}
	case StartAgent:
		var in StartInput
		if err = jsonx.Unmarshal(args, &in); err == nil {
			text, err = ag.Start(in)
		}
	case AgentResult:
		var in ResultInput
		if err = jsonx.Unmarshal(args, &in); err == nil {
			text, err = ag.Result(in)
		}
	case AgentSend:
		var in SendInput
		if err = jsonx.Unmarshal(args, &in); err == nil {
			text, err = ag.Send(in)
		}
	default:
		return nil, false
	}
	if err != nil {
		return result(err.Error(), true), true
	}
	return result(text, false), true
}

// Env is what rush's tools run as a process need of rush's environment,
// as ACP's MCP server env: an agent may start them with almost none.
func Env() []map[string]string {
	env := []map[string]string{}
	for _, k := range []string{"RUSH_HOME", "RUSH_CACHE"} {
		if v := os.Getenv(k); v != "" {
			env = append(env, map[string]string{"name": k, "value": v})
		}
	}
	return env
}
