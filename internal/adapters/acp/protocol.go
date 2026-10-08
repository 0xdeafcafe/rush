package acp

import (
	"encoding/json/jsontext"

	"github.com/0xdeafcafe/photon/jsonx"
)

// ProtocolVersion is the ACP version this client speaks.
const ProtocolVersion = 1

// Info is what the agent said about itself when it started.
type Info struct {
	Name, Title, Version string
	Protocol             int
	LoadSession          bool // it replays a past session
	Resume               bool // it picks a past session up without replaying it
	Images               bool // prompts may carry images
	AuthMethods          []AuthMethod
}

// AuthMethod is one way the agent can be signed in.
type AuthMethod struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Description string   `json:"description"`
	Type        string   `json:"type"` // "terminal" runs the agent's own binary with Args
	Args        []string `json:"args"`
}

// Mode is one of the session's modes.
type Mode struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Description string `json:"description"`
}

type initializeResult struct {
	ProtocolVersion   int `json:"protocolVersion"`
	AgentCapabilities struct {
		LoadSession        bool `json:"loadSession"`
		PromptCapabilities struct {
			Image bool `json:"image"`
		} `json:"promptCapabilities"`
		SessionCapabilities struct {
			Resume jsontext.Value `json:"resume"`
		} `json:"sessionCapabilities"`
	} `json:"agentCapabilities"`
	AuthMethods []AuthMethod `json:"authMethods"`
	AgentInfo   *struct {
		Name    string `json:"name"`
		Title   string `json:"title"`
		Version string `json:"version"`
	} `json:"agentInfo"`
}

// sessionResult is session/new's result, and session/load's and
// session/resume's without the ID.
type sessionResult struct {
	SessionID string `json:"sessionId"`
	Modes     *struct {
		CurrentModeID  string `json:"currentModeId"`
		AvailableModes []Mode `json:"availableModes"`
	} `json:"modes"`
	ConfigOptions []configOption `json:"configOptions"`
	// Models is the unstable model picker some agents still send in
	// place of a "model" config option.
	Models *struct {
		CurrentModelID string `json:"currentModelId"`
	} `json:"models"`
}

// configOption is one of a session's settings: its model, its mode, how
// hard it thinks.
type configValue struct {
	Value       string        `json:"value"`
	Name        string        `json:"name"`
	Description string        `json:"description"`
	Options     []configValue `json:"options"`
}

type configOption struct {
	Options      []configValue  `json:"options"`
	ID           string         `json:"id"`
	Name         string         `json:"name"`
	Category     string         `json:"category"`
	Type         string         `json:"type"`
	CurrentValue jsontext.Value `json:"currentValue"`
}

func (o configOption) current() string {
	var s string
	_ = jsonx.Unmarshal(o.CurrentValue, &s)
	return s
}

type contentBlock struct {
	Type     string `json:"type"`
	Text     string `json:"text,omitempty"`
	Data     string `json:"data,omitempty"`
	MimeType string `json:"mimeType,omitempty"`
	URI      string `json:"uri,omitempty"`
	Name     string `json:"name,omitempty"`
}

type chunk struct {
	Content   contentBlock `json:"content"`
	MessageID string       `json:"messageId"`
}

// toolCall is a tool_call, a tool_call_update, or the call a permission
// request is about. Unset fields leave what an earlier one said.
type toolCall struct {
	ToolCallID string         `json:"toolCallId"`
	Title      *string        `json:"title"`
	Name       *string        `json:"name"`
	Kind       *string        `json:"kind"`
	Status     *string        `json:"status"`
	Content    []toolContent  `json:"content"`
	Locations  []location     `json:"locations"`
	RawInput   jsontext.Value `json:"rawInput"`
	RawOutput  jsontext.Value `json:"rawOutput"`
	// Meta is Vibe's word for what the tool does: "subagent" for its task.
	Meta *struct {
		EffectKind string `json:"effect_kind"`
	} `json:"_meta"`
}

type toolContent struct {
	Type       string       `json:"type"` // content, diff, terminal
	Content    contentBlock `json:"content"`
	Path       string       `json:"path"`
	OldText    *string      `json:"oldText"`
	NewText    string       `json:"newText"`
	TerminalID string       `json:"terminalId"`
}

type location struct {
	Path string `json:"path"`
	Line *int   `json:"line"`
}

type planEntry struct {
	Content string `json:"content"`
	Status  string `json:"status"`
}

type permissionOption struct {
	OptionID string `json:"optionId"`
	Name     string `json:"name"`
	Kind     string `json:"kind"`
}

type promptResult struct {
	StopReason string `json:"stopReason"`
	Usage      *struct {
		InputTokens       int64 `json:"inputTokens"`
		OutputTokens      int64 `json:"outputTokens"`
		ThoughtTokens     int64 `json:"thoughtTokens"`
		CachedReadTokens  int64 `json:"cachedReadTokens"`
		CachedWriteTokens int64 `json:"cachedWriteTokens"`
	} `json:"usage"`
}
