package codex

import (
	"encoding/json/jsontext"
	"path"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// handle turns what the app-server says into rush's events, and keeps
// the requests the user has to answer.
func (c *Conn) handle(rpc *client, m message) {
	if m.ID != nil {
		c.request(rpc, m)
		return
	}
	c.emit(c.notification(m)...)
}

func (c *Conn) other(m message) event.Event {
	return event.Other{Adapter: "codex", Type: m.Method, Raw: m.Params}
}

// notification is what one notification means as events.
func (c *Conn) notification(m message) []event.Event {
	var p struct {
		ThreadID string         `json:"threadId"`
		TurnID   string         `json:"turnId"`
		ItemID   string         `json:"itemId"`
		Delta    string         `json:"delta"`
		Item     jsontext.Value `json:"item"`
		Turn     *struct {
			ID     string `json:"id"`
			Status string `json:"status"`
			Error  *struct {
				Message        string         `json:"message"`
				CodexErrorInfo jsontext.Value `json:"codexErrorInfo"`
			} `json:"error"`
			DurationMs *int64 `json:"durationMs"`
		} `json:"turn"`
		Thread *struct {
			ID string `json:"id"`
		} `json:"thread"`
		Plan []struct {
			Step   string `json:"step"`
			Status string `json:"status"`
		} `json:"plan"`
		TokenUsage *struct {
			Last               tokenBreakdown `json:"last"`
			ModelContextWindow *int           `json:"modelContextWindow"`
		} `json:"tokenUsage"`
		RateLimits *rateLimitSnapshot `json:"rateLimits"`
		RequestID  jsontext.Value     `json:"requestId"`
		WillRetry  bool               `json:"willRetry"`
	}
	if err := jsonx.Unmarshal(m.Params, &p); err != nil {
		return []event.Event{c.other(m)}
	}
	c.mu.Lock()
	thread := c.thread
	c.mu.Unlock()
	if p.ThreadID != "" && thread != "" && p.ThreadID != thread {
		// A subagent's thread: its turns are its task's.
		var out []event.Event
		if p.Turn != nil {
			out = c.kidTurn(m.Method, p.ThreadID, p.Turn.ID, p.Turn.Status)
		}
		if m.Method == "thread/tokenUsage/updated" && p.TokenUsage != nil {
			out = append(out, c.kidTokens(p.ThreadID, p.TokenUsage.Last.usage())...)
		}
		if m.Method == "item/started" {
			out = append(out, c.kidTool(p.ThreadID, p.Item)...)
		}
		return append(out, c.other(m))
	}
	switch m.Method {
	case "thread/started":
		if p.Thread != nil && (thread == "" || p.Thread.ID == thread) {
			return nil // Start sent Init from thread/start's answer
		}
	case "turn/started":
		if p.Turn != nil {
			c.mu.Lock()
			c.turn, c.tokens = p.Turn.ID, usage.TokenUsage{}
			c.mu.Unlock()
		}
		return []event.Event{event.Status{Busy: true}}
	case "turn/completed":
		if p.Turn == nil {
			break
		}
		c.mu.Lock()
		end := event.TurnEnd{Reason: turnReason(p.Turn.Status), Tokens: c.tokens, Turns: 1}
		c.turn, c.ended, c.open = "", p.Turn.ID, map[string]bool{}
		c.mu.Unlock()
		if p.Turn.DurationMs != nil {
			end.Duration = time.Duration(*p.Turn.DurationMs) * time.Millisecond
		}
		out := c.detach() // what still runs outlives the turn
		if e := p.Turn.Error; e != nil {
			end.Err = e.Message
			if string(e.CodexErrorInfo) == `"usageLimitExceeded"` {
				out = append(out, event.Limited{})
			}
		}
		return append(out, end)
	case "item/started":
		return c.itemStarted(p.Item)
	case "item/completed":
		return c.itemCompleted(p.Item)
	case "item/agentMessage/delta":
		return c.delta(p.ItemID, event.Text, p.Delta)
	case "item/reasoning/textDelta", "item/reasoning/summaryTextDelta":
		return c.delta(p.ItemID, event.Thinking, p.Delta)
	case "turn/plan/updated":
		todos := make([]tool.TodoItem, len(p.Plan))
		for i, s := range p.Plan {
			todos[i] = tool.TodoItem{Label: s.Step, Status: todoStatus(s.Status)}
		}
		return []event.Event{event.Plan{Todos: todos}}
	case "thread/tokenUsage/updated":
		if p.TokenUsage == nil {
			break
		}
		c.mu.Lock()
		c.tokens.Add(p.TokenUsage.Last.usage())
		c.mu.Unlock()
		if w := p.TokenUsage.ModelContextWindow; w != nil {
			return []event.Event{event.Context{Tokens: int(p.TokenUsage.Last.TotalTokens), Window: *w}}
		}
		return nil
	case "account/rateLimits/updated":
		if p.RateLimits == nil {
			break
		}
		q := quotaFrom(*p.RateLimits)
		c.mu.Lock()
		q.Account = c.account
		c.mu.Unlock()
		out := []event.Event{event.Quota{Quota: q}}
		if b, ok := p.RateLimits.billing(); ok {
			out = append(out, event.Billing{Billing: b})
		}
		return out
	case "serverRequest/resolved":
		id := idString(p.RequestID)
		c.mu.Lock()
		_, waiting := c.asks[id]
		delete(c.asks, id)
		c.mu.Unlock()
		if waiting {
			return []event.Event{event.ApprovalCancelled{ID: id}}
		}
		return nil
	case "error":
		if p.WillRetry {
			return []event.Event{event.Status{Busy: true, Text: "retrying"}}
		}
	}
	return []event.Event{c.other(m)}
}

func turnReason(status string) string {
	switch status {
	case "completed":
		return "done"
	case "failed":
		return "error"
	}
	return status // interrupted
}

// tokenBreakdown is Codex's TokenUsageBreakdown.
type tokenBreakdown struct {
	TotalTokens           int64 `json:"totalTokens"`
	InputTokens           int64 `json:"inputTokens"`
	CachedInputTokens     int64 `json:"cachedInputTokens"`
	OutputTokens          int64 `json:"outputTokens"`
	ReasoningOutputTokens int64 `json:"reasoningOutputTokens"`
}

// usage is the breakdown as rush counts it. OpenAI's input includes the
// cached input and its output includes the reasoning, so input is what
// wasn't cached and output is taken as it is.
func (b tokenBreakdown) usage() usage.TokenUsage {
	return usage.TokenUsage{Input: max(b.InputTokens-b.CachedInputTokens, 0), Output: b.OutputTokens, CacheRead: b.CachedInputTokens,
		Reasoning: b.ReasoningOutputTokens}
}

// delta is a piece of a streaming item, opening its message first if it
// isn't open yet.
func (c *Conn) delta(item string, kind event.PartKind, text string) []event.Event {
	out := c.openItem(item, kind)
	return append(out, event.Delta{Index: 0, Kind: kind, Text: text})
}

func (c *Conn) openItem(item string, kind event.PartKind) []event.Event {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.open[item] {
		return nil
	}
	c.open[item] = true
	return []event.Event{event.MessageStart{ID: item, Model: c.model}, event.PartStart{Index: 0, Kind: kind}}
}

func (c *Conn) itemStarted(raw jsontext.Value) []event.Event {
	var it threadItem
	if jsonx.Unmarshal(raw, &it) != nil {
		return nil
	}
	switch it.Type {
	case "contextCompaction":
		return []event.Event{event.Status{Busy: true, Text: "compacting"}}
	case "agentMessage":
		return append(c.detach(), c.openItem(it.ID, event.Text)...)
	case "reasoning":
		return append(c.detach(), c.openItem(it.ID, event.Thinking)...)
	}
	var out []event.Event
	if call, ok := callOf(it, raw); ok {
		c.mu.Lock()
		c.calls[it.ID] = call
		c.mu.Unlock()
		c.started(it)
		out = append(out, c.callMessage(call))
	}
	if it.Type == "subAgentActivity" && it.Kind == "started" {
		out = append(out, c.adopt(it.AgentThreadID, it.ID, path.Base(it.AgentPath))...)
	}
	return out
}

func (c *Conn) callMessage(call tool.Call) event.Message {
	c.mu.Lock()
	model := c.model
	c.mu.Unlock()
	return event.Message{Role: "assistant", ID: call.ID, Model: model, Parts: []event.Part{{Kind: event.ToolCall, Call: &call}}}
}

func (c *Conn) itemCompleted(raw jsontext.Value) []event.Event {
	var it threadItem
	if jsonx.Unmarshal(raw, &it) != nil {
		return nil
	}
	c.mu.Lock()
	model := c.model
	delete(c.open, it.ID)
	c.mu.Unlock()
	switch it.Type {
	case "agentMessage":
		return []event.Event{event.Message{Role: "assistant", ID: it.ID, Model: model, Parts: []event.Part{{Kind: event.Text, Text: it.Text}}}}
	case "reasoning":
		if t := reasoningText(it); t != "" {
			return []event.Event{event.Message{Role: "assistant", ID: it.ID, Model: model, Parts: []event.Part{{Kind: event.Thinking, Text: t}}}}
		}
		return nil
	case "userMessage":
		return []event.Event{userMessage(it)}
	case "contextCompaction":
		return []event.Event{event.Compacted{}}
	}
	call, ok := callOf(it, raw)
	if !ok {
		return []event.Event{event.Other{Adapter: "codex", Type: "item/" + it.Type, Raw: raw}}
	}
	var out []event.Event
	c.mu.Lock()
	_, started := c.calls[it.ID]
	delete(c.calls, it.ID)
	c.mu.Unlock()
	if !started || it.Type == "webSearch" {
		out = append(out, c.callMessage(call)) // web searches say what they searched only at the end
	}
	o := outputOf(it, raw)
	out = append(out, event.Message{Role: "user", ID: it.ID + ":result", Parts: []event.Part{{Kind: event.ToolResult, Output: &o}}})
	return append(out, c.finished(it)...)
}

// request keeps a server request for the user to answer, or answers it
// at once when rush has nothing to ask.
func (c *Conn) request(rpc *client, m message) {
	id := idString(m.ID)
	var p struct {
		ItemID      string                    `json:"itemId"`
		Reason      string                    `json:"reason"`
		Command     string                    `json:"command"`
		Cwd         string                    `json:"cwd"`
		GrantRoot   string                    `json:"grantRoot"`
		Permissions map[string]jsontext.Value `json:"permissions"`
		Questions   []struct {
			ID       string `json:"id"`
			Header   string `json:"header"`
			Question string `json:"question"`
			Options  []struct {
				Label       string `json:"label"`
				Description string `json:"description"`
			} `json:"options"`
		} `json:"questions"`
	}
	_ = jsonx.Unmarshal(m.Params, &p)
	c.mu.Lock()
	call, known := c.calls[p.ItemID]
	c.mu.Unlock()
	a := ask{id: m.ID, method: m.Method}
	var ev event.Event
	switch m.Method {
	case reqCommand:
		if !known {
			call = tool.Call{ID: p.ItemID, Name: "shell", Kind: tool.Shell}
		}
		if p.Command != "" {
			call.Input.Command, call.Input.Cwd = script(p.Command), p.Cwd
		}
		ev = event.Approval{ID: id, Call: call, Reason: p.Reason, Options: decisions}
	case reqFileChange:
		if !known {
			call = tool.Call{ID: p.ItemID, Name: "apply_patch", Kind: tool.Edit}
		}
		ev = event.Approval{ID: id, Call: call, Reason: p.Reason, Path: p.GrantRoot, Options: decisions}
	case reqPermissions:
		a.perms = p.Permissions
		call = tool.Call{ID: p.ItemID, Name: "permissions", Title: p.Reason, Raw: m.Params}
		ev = event.Approval{ID: id, Call: call, Reason: p.Reason, Path: firstPath(p.Permissions["fileSystem"]), Options: grants}
	case reqUserInput:
		q := event.Question{ID: id, CallID: p.ItemID}
		a.qs = map[string]string{}
		for _, in := range p.Questions {
			ask := event.Ask{ID: in.ID, Header: in.Header, Text: in.Question}
			for _, o := range in.Options {
				ask.Options = append(ask.Options, event.Choice{Label: o.Label, Description: o.Description})
			}
			q.Asks = append(q.Asks, ask)
			a.qs[in.Question] = in.ID
		}
		ev = q
	case reqElicitation:
		// rush has no form to fill in yet.
		_ = rpc.reply(m.ID, map[string]any{"action": "decline", "content": nil, "_meta": nil})
		c.emit(c.other(m))
		return
	default:
		_ = rpc.refuse(m.ID, "rush does not handle "+m.Method)
		c.emit(c.other(m))
		return
	}
	c.mu.Lock()
	c.asks[id] = a
	c.mu.Unlock()
	c.emit(ev)
}

// decisions are how a command or file change can be answered.
var decisions = []event.Option{
	{ID: "accept", Label: "Yes", Kind: event.AllowOnce},
	{ID: "acceptForSession", Label: "Yes, for this session", Kind: event.AllowAlways},
	{ID: "decline", Label: "No", Kind: event.RejectOnce},
	{ID: "cancel", Label: "No, and stop", Kind: event.RejectAlways},
}

// grants are how a request for more permissions can be answered.
var grants = []event.Option{
	{ID: "turn", Label: "Yes, for this turn", Kind: event.AllowOnce},
	{ID: "session", Label: "Yes, for this session", Kind: event.AllowAlways},
	{ID: "decline", Label: "No", Kind: event.RejectOnce},
}

// firstPath is the first path a file-system permission asks for.
func firstPath(fs jsontext.Value) string {
	var p struct {
		Write []string `json:"write"`
		Read  []string `json:"read"`
	}
	_ = jsonx.Unmarshal(fs, &p)
	for _, l := range [][]string{p.Write, p.Read} {
		for _, s := range l {
			if s = strings.TrimSpace(s); s != "" {
				return s
			}
		}
	}
	return ""
}
