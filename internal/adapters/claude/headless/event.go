// Package headless runs Claude Code as a bare agent loop: `claude -p` speaking
// stream-json on stdin and stdout. Claude Code keeps the model, tools, hooks,
// MCP and compaction; rush draws the session and answers its permission
// prompts, so no terminal UI, PTY host or daemon runs for it.
package headless

import (
	"encoding/base64"
	"encoding/json/jsontext"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// Event is one decoded line of Claude Code's output.
type Event interface{ event() }

// Init arrives once, when the session is ready.
type Init struct {
	SessionID      string
	Model          string
	Cwd            string
	PermissionMode string
	Version        string
	Tools          []string
	SlashCommands  []string
	MCPServers     []MCPServer
	// APIKeySource is where its API key comes from: "none" when it runs on
	// a Claude sign-in instead.
	APIKeySource string
}

// MCPServer is one MCP server as Claude Code last saw it: connected,
// failed, needs-auth, pending.
type MCPServer struct {
	Name   string `json:"name"`
	Status string `json:"status"`
}

// MessageStart opens an assistant message that Deltas then fill in.
type MessageStart struct {
	MessageID string
	Model     string
}

// Delta is a piece of text or thinking streamed while a message is written.
// Index is the content block it belongs to.
type Delta struct {
	Index    int
	Thinking bool
	Input    bool // a tool call's input, streamed as JSON
	Text     string
}

// BlockStart opens a content block: "thinking", "text" or "tool_use".
// Thinking often streams no text at all (only a signature), so this is
// how to know it's happening.
type BlockStart struct {
	Index int
	Type  string
}

// Message is a whole assistant or user turn. Tool results arrive as user
// messages. ParentToolUseID is set when a subagent wrote it.
type Message struct {
	Role            string
	ID              string
	Model           string // assistant messages: the model that wrote it
	UUID            string
	ParentToolUseID string
	Blocks          []Block
	Usage           *Usage
	// ToolResult is Claude Code's structured account of a tool run, sent with
	// the tool_result message: an Edit's structuredPatch, a Read's file, a
	// Bash run's stdout and stderr.
	ToolResult jsontext.Value
}

// Block is one content block of a message.
type Block struct {
	Type      string // text, thinking, tool_use, tool_result
	Text      string // text, thinking, or a tool result flattened to text
	ID        string // tool_use
	Name      string // tool_use
	Input     jsontext.Value
	ToolUseID string            // tool_result
	IsError   bool              // tool_result
	Images    []event.ImageData // tool_result: the images it gave back
}

type Usage struct {
	InputTokens              int `json:"input_tokens"`
	OutputTokens             int `json:"output_tokens"`
	CacheReadInputTokens     int `json:"cache_read_input_tokens"`
	CacheCreationInputTokens int `json:"cache_creation_input_tokens"`
	CacheCreation            *struct {
		M5 int `json:"ephemeral_5m_input_tokens"`
		H1 int `json:"ephemeral_1h_input_tokens"`
	} `json:"cache_creation,omitempty"`
}

// PermissionRequest asks the host whether a tool may run. Answer it with
// Session.Allow or Session.Deny.
type PermissionRequest struct {
	ID          string
	Tool        string
	Input       jsontext.Value
	Description string
	Reason      string
	ReasonType  string
	ToolUseID   string
	BlockedPath string
	Suggestions jsontext.Value
}

// PermissionCancelled withdraws a PermissionRequest, e.g. after an interrupt.
type PermissionCancelled struct{ ID string }

// PermissionDenied reports a tool call refused without asking the host.
type PermissionDenied struct {
	Tool      string
	ToolUseID string
	Reason    string
}

// Status is the session's own busy marker, e.g. "requesting".
type Status struct{ Status string }

// Compact marks where Claude Code compacted the conversation: what set it
// off (manual or auto) and the context before and after.
type Compact struct {
	Trigger    string
	PreTokens  int
	PostTokens int
}

// Result ends a turn.
type Result struct {
	Subtype    string // success, error_max_turns, error_during_execution, ...
	IsError    bool
	Text       string
	StopReason string
	SessionID  string
	CostUSD    float64
	DurationMS int
	NumTurns   int
	Usage      Usage
}

// RateLimit carries the account's usage windows.
type RateLimit struct {
	Status string
	// Overage is whether the request was paid for as extra usage: the
	// plan's allowance is spent.
	Overage bool
	Raw     jsontext.Value
}

// ControlReply answers a control request the host sent.
type ControlReply struct {
	ID    string
	Error string
	Body  jsontext.Value
}

// MCPRequest carries one MCP message from Claude Code to a server the host
// runs in-process (one named in Initialize). Answer it with ReplyMCP.
type MCPRequest struct {
	ID      string
	Server  string
	Message jsontext.Value
}

// TaskStarted says Claude Code started a task: a Bash command, a subagent,
// a monitor or a workflow. Backgrounded is false for one the turn waits on
// (it can still be moved to the background, or stopped, by its ID).
type TaskStarted struct {
	ID           string
	ToolUseID    string
	Type         string // local_bash, local_agent, monitor_mcp, local_workflow, ...
	Description  string
	SubagentType string
	Workflow     string
	Backgrounded bool
}

// TaskUpdated is what changed about a task. Status is set when it ends
// (completed, failed, killed); Backgrounded when it's moved to the
// background.
type TaskUpdated struct {
	ID           string
	Status       string
	Description  string
	Backgrounded *bool
	Error        string
}

// TaskProgress is a subagent or workflow task's running numbers.
type TaskProgress struct {
	ID          string
	Description string
	Summary     string
	LastTool    string
	Tokens      int
	ToolUses    int
}

// TaskDone says a task finished (completed, failed, stopped), and where
// its output is.
type TaskDone struct {
	ID         string
	ToolUseID  string
	Status     string
	OutputFile string
	Summary    string
}

// BackgroundTasks lists every task now running in the background, each
// time the list changes.
type BackgroundTasks struct{ Tasks []BackgroundTask }

type BackgroundTask struct {
	ID          string `json:"task_id"`
	Type        string `json:"task_type"`
	Description string `json:"description"`
}

// Other is anything not decoded above, kept whole so nothing is lost.
type Other struct {
	Type, Subtype string
	Raw           jsontext.Value
}

func (Init) event()                {}
func (MessageStart) event()        {}
func (Delta) event()               {}
func (BlockStart) event()          {}
func (Message) event()             {}
func (PermissionRequest) event()   {}
func (PermissionCancelled) event() {}
func (PermissionDenied) event()    {}
func (Status) event()              {}
func (Result) event()              {}
func (Compact) event()             {}
func (RateLimit) event()           {}
func (ControlReply) event()        {}
func (MCPRequest) event()          {}
func (TaskStarted) event()         {}
func (TaskUpdated) event()         {}
func (TaskProgress) event()        {}
func (TaskDone) event()            {}
func (BackgroundTasks) event()     {}
func (Other) event()               {}

type envelope struct {
	Type            string         `json:"type"`
	Subtype         string         `json:"subtype"`
	UUID            string         `json:"uuid"`
	ParentToolUseID string         `json:"parent_tool_use_id"`
	RequestID       string         `json:"request_id"`
	Request         jsontext.Value `json:"request"`
	Response        jsontext.Value `json:"response"`
	Message         jsontext.Value `json:"message"`
	Event           jsontext.Value `json:"event"`
	ToolUseResult   jsontext.Value `json:"tool_use_result"`
}

// Decode turns one output line into an Event.
func Decode(line []byte) (Event, error) {
	ev, err := decodeLine(line)
	if o, ok := ev.(Other); ok && o.Raw == nil {
		// Only what isn't decoded keeps a copy of its line; copying every
		// line, deltas and all, was most of what decoding allocated.
		o.Raw = append(jsontext.Value(nil), line...)
		return o, err
	}
	return ev, err
}

func decodeLine(line []byte) (Event, error) {
	var e envelope
	if err := jsonx.Unmarshal(line, &e); err != nil {
		return nil, err
	}
	other := Other{Type: e.Type, Subtype: e.Subtype}
	switch e.Type {
	case "system":
		return decodeSystem(e.Subtype, line, other)
	case "stream_event":
		return decodeStream(e.Event, other)
	case "assistant", "user":
		return decodeMessage(e)
	case "control_request":
		return decodeControl(e, other)
	case "control_cancel_request":
		return PermissionCancelled{ID: e.RequestID}, nil
	case "control_response":
		var r struct {
			RequestID string         `json:"request_id"`
			Error     string         `json:"error"`
			Response  jsontext.Value `json:"response"`
		}
		if err := jsonx.Unmarshal(e.Response, &r); err != nil {
			return nil, err
		}
		return ControlReply{ID: r.RequestID, Error: r.Error, Body: r.Response}, nil
	case "result":
		var r struct {
			Subtype    string  `json:"subtype"`
			IsError    bool    `json:"is_error"`
			Result     string  `json:"result"`
			StopReason string  `json:"stop_reason"`
			SessionID  string  `json:"session_id"`
			CostUSD    float64 `json:"total_cost_usd"`
			DurationMS int     `json:"duration_ms"`
			NumTurns   int     `json:"num_turns"`
			Usage      Usage   `json:"usage"`
		}
		if err := jsonx.Unmarshal(line, &r); err != nil {
			return nil, err
		}
		return Result{Subtype: r.Subtype, IsError: r.IsError, Text: r.Result, StopReason: r.StopReason,
			SessionID: r.SessionID, CostUSD: r.CostUSD, DurationMS: r.DurationMS, NumTurns: r.NumTurns, Usage: r.Usage}, nil
	case "rate_limit_event":
		var r struct {
			Info jsontext.Value `json:"rate_limit_info"`
		}
		_ = jsonx.Unmarshal(line, &r)
		var s struct {
			Status  string `json:"status"`
			Overage bool   `json:"isUsingOverage"`
		}
		_ = jsonx.Unmarshal(r.Info, &s)
		return RateLimit{Status: s.Status, Overage: s.Overage, Raw: r.Info}, nil
	}
	return other, nil
}

func decodeSystem(subtype string, line []byte, other Other) (Event, error) {
	switch subtype {
	case "init":
		var r struct {
			SessionID      string      `json:"session_id"`
			Model          string      `json:"model"`
			Cwd            string      `json:"cwd"`
			PermissionMode string      `json:"permissionMode"`
			Version        string      `json:"claude_code_version"`
			Tools          []string    `json:"tools"`
			SlashCommands  []string    `json:"slash_commands"`
			MCPServers     []MCPServer `json:"mcp_servers"`
			APIKeySource   string      `json:"apiKeySource"`
		}
		if err := jsonx.Unmarshal(line, &r); err != nil {
			return nil, err
		}
		return Init(r), nil
	case "status":
		var r struct {
			Status string `json:"status"`
		}
		_ = jsonx.Unmarshal(line, &r)
		return Status(r), nil
	case "compact_boundary":
		var r struct {
			Meta struct {
				Trigger   string `json:"trigger"`
				PreTokens int    `json:"pre_tokens"`
			} `json:"compact_metadata"`
		}
		_ = jsonx.Unmarshal(line, &r)
		return Compact{Trigger: r.Meta.Trigger, PreTokens: r.Meta.PreTokens}, nil
	case "permission_denied":
		var r struct {
			Tool      string `json:"tool_name"`
			ToolUseID string `json:"tool_use_id"`
			Reason    string `json:"decision_reason"`
		}
		_ = jsonx.Unmarshal(line, &r)
		return PermissionDenied(r), nil
	case "task_started":
		var r struct {
			ID           string `json:"task_id"`
			ToolUseID    string `json:"tool_use_id"`
			Type         string `json:"task_type"`
			Description  string `json:"description"`
			SubagentType string `json:"subagent_type"`
			Workflow     string `json:"workflow_name"`
			Backgrounded bool   `json:"is_backgrounded"`
		}
		if err := jsonx.Unmarshal(line, &r); err != nil {
			return nil, err
		}
		return TaskStarted(r), nil
	case "task_updated":
		var r struct {
			ID    string `json:"task_id"`
			Patch struct {
				Status       string `json:"status"`
				Description  string `json:"description"`
				Backgrounded *bool  `json:"is_backgrounded"`
				Error        string `json:"error"`
			} `json:"patch"`
		}
		if err := jsonx.Unmarshal(line, &r); err != nil {
			return nil, err
		}
		p := r.Patch
		return TaskUpdated{ID: r.ID, Status: p.Status, Description: p.Description, Backgrounded: p.Backgrounded, Error: p.Error}, nil
	case "task_progress":
		var r struct {
			ID          string `json:"task_id"`
			Description string `json:"description"`
			Summary     string `json:"summary"`
			LastTool    string `json:"last_tool_name"`
			Usage       struct {
				Tokens   int `json:"total_tokens"`
				ToolUses int `json:"tool_uses"`
			} `json:"usage"`
		}
		if err := jsonx.Unmarshal(line, &r); err != nil {
			return nil, err
		}
		return TaskProgress{ID: r.ID, Description: r.Description, Summary: r.Summary, LastTool: r.LastTool, Tokens: r.Usage.Tokens, ToolUses: r.Usage.ToolUses}, nil
	case "task_notification":
		var r struct {
			ID         string `json:"task_id"`
			ToolUseID  string `json:"tool_use_id"`
			Status     string `json:"status"`
			OutputFile string `json:"output_file"`
			Summary    string `json:"summary"`
		}
		if err := jsonx.Unmarshal(line, &r); err != nil {
			return nil, err
		}
		return TaskDone(r), nil
	case "background_tasks_changed":
		var r struct {
			Tasks []BackgroundTask `json:"tasks"`
		}
		if err := jsonx.Unmarshal(line, &r); err != nil {
			return nil, err
		}
		return BackgroundTasks(r), nil
	}
	return other, nil
}

func decodeStream(raw jsontext.Value, other Other) (Event, error) {
	var ev struct {
		Type    string `json:"type"`
		Index   int    `json:"index"`
		Message struct {
			ID    string `json:"id"`
			Model string `json:"model"`
		} `json:"message"`
		Delta struct {
			Type        string `json:"type"`
			Text        string `json:"text"`
			Thinking    string `json:"thinking"`
			PartialJSON string `json:"partial_json"`
		} `json:"delta"`
		ContentBlock struct {
			Type string `json:"type"`
		} `json:"content_block"`
	}
	if err := jsonx.Unmarshal(raw, &ev); err != nil {
		return nil, err
	}
	switch ev.Type {
	case "message_start":
		return MessageStart{MessageID: ev.Message.ID, Model: ev.Message.Model}, nil
	case "content_block_start":
		return BlockStart{Index: ev.Index, Type: ev.ContentBlock.Type}, nil
	case "content_block_delta":
		switch ev.Delta.Type {
		case "text_delta":
			return Delta{Index: ev.Index, Text: ev.Delta.Text}, nil
		case "thinking_delta":
			return Delta{Index: ev.Index, Thinking: true, Text: ev.Delta.Thinking}, nil
		case "input_json_delta":
			// A tool call being written: counted, not shown.
			return Delta{Index: ev.Index, Input: true, Text: ev.Delta.PartialJSON}, nil
		}
	}
	return other, nil
}

// DecodeMessage decodes an assistant or user message from its parts, for a
// reader (like a transcript's) that has already split the line up.
func DecodeMessage(typ string, message, toolUseResult jsontext.Value) (Event, error) {
	return decodeMessage(envelope{Type: typ, Message: message, ToolUseResult: toolUseResult})
}

func decodeMessage(e envelope) (Event, error) {
	var m struct {
		ID      string         `json:"id"`
		Role    string         `json:"role"`
		Model   string         `json:"model"`
		Content jsontext.Value `json:"content"`
		Usage   *Usage         `json:"usage"`
	}
	if err := jsonx.Unmarshal(e.Message, &m); err != nil {
		return nil, err
	}
	out := Message{Role: m.Role, ID: m.ID, Model: m.Model, UUID: e.UUID, ParentToolUseID: e.ParentToolUseID, Usage: m.Usage, ToolResult: e.ToolUseResult}
	if out.Role == "" {
		out.Role = e.Type
	}
	var text string
	if jsonx.Unmarshal(m.Content, &text) == nil {
		out.Blocks = []Block{{Type: "text", Text: text}}
		return out, nil
	}
	var blocks []struct {
		Type      string         `json:"type"`
		Text      string         `json:"text"`
		Thinking  string         `json:"thinking"`
		ID        string         `json:"id"`
		Name      string         `json:"name"`
		Input     jsontext.Value `json:"input"`
		ToolUseID string         `json:"tool_use_id"`
		Content   jsontext.Value `json:"content"`
		IsError   bool           `json:"is_error"`
	}
	if err := jsonx.Unmarshal(m.Content, &blocks); err != nil {
		return nil, err
	}
	for _, b := range blocks {
		blk := Block{Type: b.Type, Text: b.Text, ID: b.ID, Name: b.Name, Input: b.Input, ToolUseID: b.ToolUseID, IsError: b.IsError}
		switch b.Type {
		case "thinking":
			blk.Text = b.Thinking
		case "tool_result":
			blk.Text, blk.Images = flatten(b.Content)
		}
		out.Blocks = append(out.Blocks, blk)
	}
	return out, nil
}

// flatten reads a tool result's content, a string or a list of blocks, as
// text and the images it holds.
func flatten(raw jsontext.Value) (string, []event.ImageData) {
	var s string
	if jsonx.Unmarshal(raw, &s) == nil {
		return s, nil
	}
	var parts []struct {
		Type   string `json:"type"`
		Text   string `json:"text"`
		Source struct {
			MediaType string `json:"media_type"`
			Data      string `json:"data"`
		} `json:"source"`
	}
	if jsonx.Unmarshal(raw, &parts) != nil {
		return "", nil
	}
	var out []string
	var images []event.ImageData
	for _, p := range parts {
		switch p.Type {
		case "text":
			out = append(out, p.Text)
		case "image":
			data, _ := base64.StdEncoding.DecodeString(p.Source.Data)
			images = append(images, event.ImageData{MediaType: p.Source.MediaType, Data: data})
		}
	}
	return strings.Join(out, "\n"), images
}

func decodeControl(e envelope, other Other) (Event, error) {
	var r struct {
		Subtype     string         `json:"subtype"`
		Tool        string         `json:"tool_name"`
		Input       jsontext.Value `json:"input"`
		Description string         `json:"description"`
		Reason      string         `json:"decision_reason"`
		ReasonType  string         `json:"decision_reason_type"`
		ToolUseID   string         `json:"tool_use_id"`
		BlockedPath string         `json:"blocked_path"`
		Suggestions jsontext.Value `json:"permission_suggestions"`
	}
	if err := jsonx.Unmarshal(e.Request, &r); err != nil {
		return nil, err
	}
	if r.Subtype == "mcp_message" {
		var m struct {
			Server  string         `json:"server_name"`
			Message jsontext.Value `json:"message"`
		}
		if err := jsonx.Unmarshal(e.Request, &m); err != nil {
			return nil, err
		}
		return MCPRequest{ID: e.RequestID, Server: m.Server, Message: m.Message}, nil
	}
	if r.Subtype != "can_use_tool" {
		return other, nil
	}
	return PermissionRequest{ID: e.RequestID, Tool: r.Tool, Input: r.Input, Description: r.Description,
		Reason: r.Reason, ReasonType: r.ReasonType, ToolUseID: r.ToolUseID, BlockedPath: r.BlockedPath,
		Suggestions: r.Suggestions}, nil
}

// Command is a slash command the session accepts.
type Command struct {
	Name         string   `json:"name"`
	Description  string   `json:"description"`
	ArgumentHint string   `json:"argumentHint"`
	Aliases      []string `json:"aliases"`
}

// Commands reads the slash commands out of the reply to Initialize.
func Commands(reply ControlReply) []Command {
	var r struct {
		Commands []Command `json:"commands"`
	}
	_ = jsonx.Unmarshal(reply.Body, &r)
	return r.Commands
}

// Patch is one hunk of an edit, as Claude Code reports it in an Edit or
// Write result's structuredPatch.
type Patch struct {
	OldStart int      `json:"oldStart"`
	OldLines int      `json:"oldLines"`
	NewStart int      `json:"newStart"`
	NewLines int      `json:"newLines"`
	Lines    []string `json:"lines"` // each prefixed ' ', '-' or '+'
}

// Patches reads the hunks from a tool result, if it has any.
func Patches(toolResult jsontext.Value) []Patch {
	var r struct {
		StructuredPatch []Patch `json:"structuredPatch"`
	}
	_ = jsonx.Unmarshal(toolResult, &r)
	return r.StructuredPatch
}
