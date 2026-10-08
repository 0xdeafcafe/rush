package pi

import (
	"encoding/json/jsontext"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// line is one event pi writes, with the fields of every type rush reads.
// Its "message" is an AgentMessage in message events, and text in an
// extension's dialog.
type line struct {
	Type    string         `json:"type"`
	Message jsontext.Value `json:"message"`

	AssistantMessageEvent *struct {
		Type         string `json:"type"`
		ContentIndex int    `json:"contentIndex"`
		Delta        string `json:"delta"`
	} `json:"assistantMessageEvent"` // message_update

	Reason string `json:"reason"` // compaction_start, compaction_end
	Result *struct {
		TokensBefore int `json:"tokensBefore"`
	} `json:"result"`

	ID      string   `json:"id"` // extension_ui_request
	Method  string   `json:"method"`
	Title   string   `json:"title"`
	Options []string `json:"options"`

	ToolCallID       string         `json:"toolCallId"` // tool_execution_start, _update, _end
	ToolName         string         `json:"toolName"`
	ParentToolCallID string         `json:"parentToolCallId"`
	Args             jsontext.Value `json:"args"`
	PartialResult    *message       `json:"partialResult"` // its content and details
	IsError          bool           `json:"isError"`
}

// message is the line's AgentMessage, if it has one.
func (l *line) message() *message {
	if len(l.Message) == 0 || l.Message.Kind() != '{' {
		return nil
	}
	var m message
	if jsonx.Unmarshal(l.Message, &m) != nil {
		return nil
	}
	return &m
}

// text is the line's message when it's text.
func (l *line) text() string {
	var s string
	if len(l.Message) > 0 && l.Message.Kind() == '"' {
		_ = jsonx.Unmarshal(l.Message, &s)
	}
	return s
}

// handle turns what pi says into rush's events.
func (c *Conn) handle(typ string, raw []byte) {
	c.emit(c.events1(typ, raw)...)
}

func other(typ string, raw []byte) event.Event {
	return event.Other{Adapter: "pi", Type: typ, Raw: jsontext.Value(append([]byte(nil), raw...))}
}

// events1 is what one line means as events.
func (c *Conn) events1(typ string, raw []byte) []event.Event {
	switch typ {
	case "agent_settled", "turn_start", "turn_end", "queue_update", "auto_retry_end":
		// What these say comes whole in the message events, or is pi's
		// own bookkeeping.
		return nil
	}
	var l line
	if err := jsonx.Unmarshal(raw, &l); err != nil {
		return []event.Event{other(typ, raw)}
	}
	switch typ {
	case "agent_start":
		c.mu.Lock()
		c.running, c.began = true, time.Now()
		c.tokens, c.stop, c.failed, c.last = usage.TokenUsage{}, "", "", ""
		c.mu.Unlock()
		return []event.Event{event.Status{Busy: true}}
	case "agent_end":
		c.mu.Lock()
		end := event.TurnEnd{Reason: reason(c.stop), Err: c.failed, Text: c.last, Cost: c.cost, Tokens: c.tokens, Turns: 1}
		if !c.began.IsZero() {
			end.Duration = time.Since(c.began)
		}
		c.running, c.began = false, time.Time{}
		c.mu.Unlock()
		return []event.Event{end}
	case "message_start":
		if m := l.message(); m != nil && m.Role == "assistant" {
			c.mu.Lock()
			c.msg = m.id()
			model := firstNonEmpty(m.Model, c.model.ID)
			c.mu.Unlock()
			return []event.Event{event.MessageStart{ID: m.id(), Model: model}}
		}
		return nil
	case "message_update":
		return c.update(&l)
	case "message_end":
		if m := l.message(); m != nil {
			return c.messageEnd(m)
		}
	case "compaction_start":
		return []event.Event{event.Status{Busy: true, Text: "compacting"}}
	case "compaction_end":
		if l.Result == nil {
			return nil // aborted, or failed: the compact command says so
		}
		trigger := "auto"
		if l.Reason == "manual" {
			trigger = "manual"
		}
		return []event.Event{event.Compacted{Trigger: trigger, Before: l.Result.TokensBefore}}
	case "auto_retry_start":
		return []event.Event{event.Status{Busy: true, Text: "retrying"}}
	case "extension_ui_request":
		return c.dialog(&l, raw)
	case "tool_execution_start", "tool_execution_update", "tool_execution_end":
		return subagentTask(&l)
	}
	return []event.Event{other(typ, raw)}
}

// update is a piece of the assistant message being written.
func (c *Conn) update(l *line) []event.Event {
	e := l.AssistantMessageEvent
	if e == nil {
		return nil
	}
	i := e.ContentIndex
	switch e.Type {
	case "text_start":
		return []event.Event{event.PartStart{Index: i, Kind: event.Text}}
	case "text_delta":
		return []event.Event{event.Delta{Index: i, Kind: event.Text, Text: e.Delta}}
	case "thinking_start":
		return []event.Event{event.PartStart{Index: i, Kind: event.Thinking}}
	case "thinking_delta":
		return []event.Event{event.Delta{Index: i, Kind: event.Thinking, Text: e.Delta}}
	case "toolcall_start":
		return []event.Event{event.PartStart{Index: i, Kind: event.ToolCall}}
	case "toolcall_delta":
		return []event.Event{event.Delta{Index: i, Kind: event.ToolCall, Text: e.Delta}}
	}
	return nil
}

// messageEnd is a whole message. Messages rush sent come back from pi as
// it takes them in, and aren't told again: the host showed them as sent.
func (c *Conn) messageEnd(m *message) []event.Event {
	c.mu.Lock()
	defer c.mu.Unlock()
	switch m.Role {
	case "user":
		if len(c.sent) > 0 {
			c.sent = c.sent[1:]
			return nil
		}
		return m.events("pi-user-" + itoa(m.Timestamp))
	case "assistant":
		id := c.msg
		if id == "" {
			id = m.id()
		}
		c.msg = ""
		out := m.events(id)
		if m.StopReason != "" {
			c.stop = m.StopReason
		}
		if m.ErrorMessage != "" && m.StopReason != "aborted" {
			c.failed = m.ErrorMessage
		}
		if t := strings.TrimSpace(m.text()); t != "" {
			c.last = t
		}
		if u := m.Usage; u != nil {
			c.tokens.Add(u.usage())
			c.cost += u.Cost.Total
			if n := u.context(); n > 0 {
				out = append(out, event.Context{Tokens: n, Window: c.model.ContextWindow})
			}
		}
		if m.Model != "" && m.Model != c.model.ID && m.Provider == c.model.Provider {
			c.model.ID = m.Model
		}
		return out
	}
	return m.events("pi-" + m.Role + "-" + itoa(m.Timestamp))
}

// subagentTask is the subagent extension's run as a task the turn waits on,
// as Claude Code tells its subagents'. Other tools' runs show in their
// calls and results; a tool's own nested calls aren't told.
func subagentTask(l *line) []event.Event {
	if l.ToolName != "subagent" || l.ParentToolCallID != "" {
		return nil
	}
	switch l.Type {
	case "tool_execution_start":
		c := callOf(&block{ID: l.ToolCallID, Name: l.ToolName, Arguments: l.Args})
		return []event.Event{event.TaskStarted{ID: l.ToolCallID, CallID: l.ToolCallID, Kind: event.SubagentTask, Label: c.Input.Description, Agent: c.Input.Agent}}
	case "tool_execution_update":
		if l.PartialResult != nil {
			return []event.Event{subagentProgress(l.ToolCallID, l.PartialResult)}
		}
	case "tool_execution_end":
		status := "completed"
		if l.IsError {
			status = "failed"
		}
		return []event.Event{event.TaskDone{ID: l.ToolCallID, CallID: l.ToolCallID, Status: status}}
	}
	return nil
}

// subagentProgress is what a subagent run has done so far, from the
// details of its partial result: each agent's messages and usage.
func subagentProgress(id string, r *message) event.TaskProgress {
	p := event.TaskProgress{ID: id, Summary: oneLine(r.text())}
	var d struct {
		Results []struct {
			Messages []message `json:"messages"`
			Usage    struct {
				Input  int `json:"input"`
				Output int `json:"output"`
			} `json:"usage"`
		} `json:"results"`
	}
	_ = jsonx.Unmarshal(r.Details, &d)
	for _, res := range d.Results {
		p.Tokens += res.Usage.Input + res.Usage.Output
		for i := range res.Messages {
			for _, b := range res.Messages[i].blocks() {
				if b.Type == "toolCall" {
					p.ToolUses++
					p.LastTool = b.Name
				}
			}
		}
	}
	return p
}

// confirmYes and confirmNo are a confirm's choices.
const (
	confirmYes = "Yes"
	confirmNo  = "No"
)

// dialog is an extension asking something. Select and confirm are
// questions; a text box rush can't show is cancelled at once, and what
// needs no answer is passed on as it is.
func (c *Conn) dialog(l *line, raw []byte) []event.Event {
	switch l.Method {
	case "select", "confirm":
		ask := event.Ask{Text: firstNonEmpty(l.Title, l.text())}
		d := dialog{confirm: l.Method == "confirm"}
		if d.confirm {
			if l.Title != "" && l.text() != "" {
				ask.Header, ask.Text = l.Title, l.text()
			}
			ask.Options = []event.Choice{{Label: confirmYes}, {Label: confirmNo}}
		} else {
			for _, o := range l.Options {
				ask.Options = append(ask.Options, event.Choice{Label: o})
			}
		}
		c.mu.Lock()
		c.dialogs[l.ID] = d
		c.mu.Unlock()
		return []event.Event{event.Question{ID: l.ID, Title: l.Title, Asks: []event.Ask{ask}}}
	case "input", "editor":
		_ = c.rpc.post(map[string]any{"type": "extension_ui_response", "id": l.ID, "cancelled": true})
	}
	return []event.Event{other("extension_ui_request", raw)}
}

func itoa(n int64) string { return strconv.FormatInt(n, 10) }

func firstNonEmpty(ss ...string) string {
	for _, s := range ss {
		if s != "" {
			return s
		}
	}
	return ""
}
