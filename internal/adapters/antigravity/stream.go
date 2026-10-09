package antigravity

import (
	"crypto/rand"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

type tokens struct {
	Input    int64 `json:"input_tokens"`
	Output   int64 `json:"output_tokens"`
	Cache    int64 `json:"cache_read_tokens"`
	Thinking int64 `json:"thinking_tokens"`
}

func (t tokens) since(p tokens) usage.TokenUsage {
	return usage.TokenUsage{Input: max(0, t.Input-p.Input), Output: max(0, t.Output-p.Output), CacheRead: max(0, t.Cache-p.Cache), Reasoning: max(0, t.Thinking-p.Thinking)}
}

type wire struct {
	Event        string `json:"event"`
	Conversation string `json:"conversation_id"`
	Init         struct {
		Model, Cwd string
		Mode       string `json:"permission_mode"`
		Tools      []string
	} `json:"init"`
	Step struct {
		Index    int `json:"step_index"`
		State    string
		Type     string `json:"step_type"`
		Text     string `json:"text_delta"`
		ToolName string `json:"tool_name"`
		Tool     *struct {
			Name       string
			Parameters json.RawMessage
			Output     json.RawMessage
			Error      *struct{ Type, Message string }
		} `json:"tool_info"`
	} `json:"step_update"`
	Result struct {
		Conversation            string `json:"conversation_id"`
		Status, Response, Error string
		Usage                   tokens
		Duration                float64 `json:"duration_seconds"`
		Turns                   int     `json:"num_turns"`
		Denied                  []struct {
			Name string `json:"display_name"`
		} `json:"denied_actions"`
	} `json:"result"`
}
type parser struct {
	run      string
	emit     func(event.Event) bool
	texts    map[int]*strings.Builder
	tools    map[int]bool
	finished map[int]bool
	hasText  bool
	previous tokens
	seconds  float64
	end      event.TurnEnd
}

func (p *parser) line(b []byte) (bool, error) {
	if p.run == "" {
		p.run = rand.Text()
	}
	var w wire
	if err := json.Unmarshal(b, &w); err != nil {
		return false, fmt.Errorf("invalid Antigravity stream: %w", err)
	}
	switch w.Event {
	case "init":
		p.emit(event.Init{SessionID: w.Conversation, Model: w.Init.Model, Cwd: w.Init.Cwd, Mode: w.Init.Mode, Tools: w.Init.Tools})
	case "step_update":
		s := w.Step
		id := fmt.Sprintf("agy-%s-step-%d", p.run, s.Index)
		if p.finished[s.Index] {
			return false, nil
		}
		if s.Type == "agent_response" {
			if p.texts == nil {
				p.texts = map[int]*strings.Builder{}
			}
			if _, ok := p.texts[s.Index]; !ok {
				p.emit(event.MessageStart{ID: id})
				p.texts[s.Index] = &strings.Builder{}
			}
			p.texts[s.Index].WriteString(s.Text)
			if s.Text != "" {
				p.emit(event.Delta{Kind: event.Text, Text: s.Text})
			}
			if s.State == "DONE" {
				if text := p.texts[s.Index].String(); text != "" {
					p.emit(event.Message{Role: "assistant", ID: id, Parts: []event.Part{{Kind: event.Text, Text: text}}})
					p.hasText = true
				}
				delete(p.texts, s.Index)
				p.mark(s.Index)
			}
		} else if s.Type == "tool" && s.Tool != nil {
			ti := s.Tool
			name := ti.Name
			if name == "" {
				name = s.ToolName
			}
			call := tool.Call{ID: id, Name: name, Title: name}
			call.Raw = append(call.Raw, ti.Parameters...)
			var args struct{ CommandLine, Cwd, TargetFile, AbsolutePath, FileContent string }
			_ = json.Unmarshal(ti.Parameters, &args)
			switch name {
			case "run_command":
				call.Kind = tool.Shell
				call.Input.Command, call.Input.Cwd = args.CommandLine, args.Cwd
			case "view_file":
				call.Kind = tool.Read
				call.Input.Path = args.AbsolutePath
			case "write_to_file":
				call.Kind = tool.Write
				call.Input.Path, call.Input.Content = args.TargetFile, args.FileContent
			}
			if !p.tools[s.Index] {
				if p.tools == nil {
					p.tools = map[int]bool{}
				}
				p.tools[s.Index] = true
				p.emit(event.Message{Role: "assistant", ID: id, Parts: []event.Part{{Kind: event.ToolCall, Call: &call}}})
			}
			// A denied or failed call ends ERROR, not DONE.
			if s.State == "DONE" || s.State == "ERROR" {
				p.emit(event.CallUpdated{Call: call})
				text := string(ti.Output)
				var plain string
				if json.Unmarshal(ti.Output, &plain) == nil {
					text = plain
				}
				if ti.Error != nil {
					text = strings.TrimSpace(text + "\n" + ti.Error.Message)
				}
				failed := ti.Error != nil || s.State == "ERROR"
				p.emit(event.Message{Role: "user", ID: id + "-result", Parts: []event.Part{{Kind: event.ToolResult, Output: &tool.Output{CallID: id, Text: text, IsError: failed}}}})
				p.mark(s.Index)
			}
		}
	case "result":
		r := w.Result
		// Complete any partial response if the CLI omits its final step transition.
		for i, text := range p.texts {
			if text.Len() == 0 {
				continue
			}
			p.emit(event.Message{Role: "assistant", ID: fmt.Sprintf("agy-%s-step-%d", p.run, i), Parts: []event.Part{{Kind: event.Text, Text: text.String()}}})
			p.hasText = true
		}
		if !p.hasText && r.Response != "" {
			p.emit(event.Message{Role: "assistant", ID: fmt.Sprintf("agy-%s-result-%d", p.run, r.Turns), Parts: []event.Part{{Kind: event.Text, Text: r.Response}}})
		}
		reason := "done"
		err := r.Error
		switch r.Status {
		case "SUCCESS":
		case "CANCELED", "INTERRUPTED":
			reason = "interrupted"
		default:
			reason = "error"
			if err == "" {
				err = "Antigravity ended with status " + r.Status
			}
		}
		// Headless, it can't ask: it denies the call and ends "SUCCESS" saying nothing.
		if reason == "done" && !p.hasText && r.Response == "" && len(r.Denied) > 0 {
			names := make([]string, len(r.Denied))
			for i, d := range r.Denied {
				names[i] = d.Name
			}
			reason, err = "error", "Antigravity denied "+strings.Join(names, ", ")+" without asking, as it can't headless: allow it under permissions.allow in its settings.json, or run it in always-proceed mode"
		}
		p.end = event.TurnEnd{Reason: reason, Err: err, Text: r.Response, Tokens: r.Usage.since(p.previous), Duration: time.Duration(max(0, r.Duration-p.seconds) * float64(time.Second)), Turns: 1}
		p.previous, p.seconds = r.Usage, r.Duration
		p.texts = nil
		p.tools = nil
		p.finished = nil
		p.hasText = false
		return true, nil
	}
	return false, nil
}
func (p *parser) mark(i int) {
	if p.finished == nil {
		p.finished = map[int]bool{}
	}
	p.finished[i] = true
}
