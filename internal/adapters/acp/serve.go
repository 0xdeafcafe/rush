package acp

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// served takes the agent's requests of rush. Those that wait on the user
// are answered later, through Answer and AnswerQuestion.
func (s *Session) served(id jsontext.Value, method string, params jsontext.Value) {
	switch {
	case method == "session/request_permission":
		s.permission(id, params)
	case method == "elicitation/create":
		s.elicit(id, params)
	case !s.o.NoFS && (method == "fs/read_text_file" || method == "fs/write_text_file"):
		go func() {
			res, err := fsCall(method, params)
			_ = s.rpc.reply(id, res, err)
		}()
	default:
		_ = s.rpc.reply(id, nil, &Error{Code: CodeMethodNotFound, Message: "method not found: " + method})
	}
}

// requestKey is a request ID as rush's approval and question IDs.
func requestKey(id jsontext.Value) string { return strings.Trim(string(id), `"`) }

func (s *Session) permission(id, params jsontext.Value) {
	var p struct {
		ToolCall toolCall           `json:"toolCall"`
		Options  []permissionOption `json:"options"`
	}
	if err := jsonx.Unmarshal(params, &p); err != nil {
		_ = s.rpc.reply(id, nil, &Error{Code: CodeInvalidParams, Message: err.Error()})
		return
	}
	key := requestKey(id)
	s.amu.Lock()
	s.approvals[key] = id
	s.amu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	// The call may not have been announced yet; the approval needs it to
	// hang off.
	c := s.track(p.ToolCall)
	a := event.Approval{ID: key, Call: c.c, Path: c.c.Input.Path}
	for _, o := range p.Options {
		a.Options = append(a.Options, event.Option{ID: o.OptionID, Label: o.Name, Kind: optionKind(o.Kind)})
	}
	s.emit(a)
}

func optionKind(k string) event.OptionKind {
	switch k {
	case "allow_always":
		return event.AllowAlways
	case "reject_once":
		return event.RejectOnce
	case "reject_always":
		return event.RejectAlways
	}
	return event.AllowOnce
}

// withdraw is the agent taking back a request of its own.
func (s *Session) withdraw(id jsontext.Value) {
	key := requestKey(id)
	s.amu.Lock()
	rid, approval := s.approvals[key]
	delete(s.approvals, key)
	q, question := s.questions[key]
	delete(s.questions, key)
	s.amu.Unlock()
	cancelled := &Error{Code: CodeCancelled, Message: "cancelled"}
	switch {
	case approval:
		_ = s.rpc.reply(rid, nil, cancelled)
		s.mu.Lock()
		s.emit(event.ApprovalCancelled{ID: key})
		s.mu.Unlock()
	case question:
		_ = s.rpc.reply(q.id, nil, cancelled)
		s.mu.Lock()
		s.emit(event.ApprovalCancelled{ID: key})
		s.mu.Unlock()
	}
}

// elicitation is a form the agent asked the user to fill in, as a
// Question.
type elicitation struct {
	id     jsontext.Value
	fields map[string]field
}

type field struct {
	typ    string            // string, number, integer, boolean, array
	values map[string]string // a choice's label → the value it sends
}

type propSchema struct {
	Type  string   `json:"type"`
	Title string   `json:"title"`
	Enum  []string `json:"enum"`
	OneOf []titled `json:"oneOf"`
	Items *struct {
		Enum  []string `json:"enum"`
		AnyOf []titled `json:"anyOf"`
	} `json:"items"`
}

type titled struct {
	Const string `json:"const"`
	Title string `json:"title"`
}

// elicit turns a form elicitation into a Question: one Ask a field. A
// URL elicitation isn't advertised, so is declined.
func (s *Session) elicit(id, params jsontext.Value) {
	var p struct {
		Mode            string `json:"mode"`
		Message         string `json:"message"`
		ToolCallID      string `json:"toolCallId"`
		RequestedSchema struct {
			Properties map[string]propSchema `json:"properties"`
		} `json:"requestedSchema"`
	}
	if err := jsonx.Unmarshal(params, &p); err != nil || p.Mode != "form" {
		_ = s.rpc.reply(id, map[string]string{"action": "decline"}, nil)
		return
	}
	key := requestKey(id)
	e := &elicitation{id: id, fields: map[string]field{}}
	q := event.Question{ID: key, CallID: p.ToolCallID, Title: p.Message}
	s.mu.Lock()
	if c := s.calls[p.ToolCallID]; c != nil && c.c.Input.Background {
		// Asked in the background, its call returns before it's answered:
		// it stands on its own, not on a call whose result would close it.
		q.CallID = ""
	}
	s.mu.Unlock()
	names := make([]string, 0, len(p.RequestedSchema.Properties))
	for name := range p.RequestedSchema.Properties {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		ps := p.RequestedSchema.Properties[name]
		f := field{typ: ps.Type, values: map[string]string{}}
		ask := event.Ask{ID: name, Header: ps.Title, Text: ps.Title, Multi: ps.Type == "array"}
		if ask.Text == "" {
			ask.Text = name
		}
		choose := func(value, label string) {
			if label == "" {
				label = value
			}
			f.values[label] = value
			ask.Options = append(ask.Options, event.Choice{Label: label})
		}
		enum, one := ps.Enum, ps.OneOf
		if ps.Items != nil {
			enum, one = ps.Items.Enum, ps.Items.AnyOf
		}
		for _, v := range enum {
			choose(v, v)
		}
		for _, o := range one {
			choose(o.Const, o.Title)
		}
		if ps.Type == "boolean" {
			choose("true", "Yes")
			choose("false", "No")
		}
		e.fields[name] = f
		q.Asks = append(q.Asks, ask)
	}
	s.amu.Lock()
	s.questions[key] = e
	s.amu.Unlock()
	s.mu.Lock()
	s.emit(q)
	s.mu.Unlock()
}

// AnswerQuestion fills in the form a Question came from. No answers at
// all declines it.
func (s *Session) AnswerQuestion(id string, answers map[string][]string) error {
	s.amu.Lock()
	e, ok := s.questions[id]
	delete(s.questions, id)
	s.amu.Unlock()
	if !ok {
		return fmt.Errorf("acp: no question %q waiting", id)
	}
	if len(answers) == 0 {
		return s.rpc.reply(e.id, map[string]string{"action": "decline"}, nil)
	}
	content := map[string]any{}
	for name, got := range answers {
		f, ok := e.fields[name]
		if !ok || len(got) == 0 {
			continue
		}
		vals := make([]string, len(got))
		for i, g := range got {
			vals[i] = g
			if v, ok := f.values[g]; ok {
				vals[i] = v
			}
		}
		switch f.typ {
		case "array":
			content[name] = vals
		case "boolean":
			content[name] = vals[0] == "true"
		case "number", "integer":
			n, err := strconv.ParseFloat(vals[0], 64)
			if err != nil {
				return fmt.Errorf("acp: %s: %w", name, err)
			}
			content[name] = n
		default:
			content[name] = vals[0]
		}
	}
	return s.rpc.reply(e.id, map[string]any{"action": "accept", "content": content}, nil)
}

// fsCall reads or writes a file for the agent.
func fsCall(method string, params jsontext.Value) (any, *Error) {
	var p struct {
		Path    string `json:"path"`
		Line    int    `json:"line"`
		Limit   int    `json:"limit"`
		Content string `json:"content"`
	}
	if err := jsonx.Unmarshal(params, &p); err != nil || !filepath.IsAbs(p.Path) {
		return nil, &Error{Code: CodeInvalidParams, Message: "an absolute path is needed"}
	}
	if method == "fs/write_text_file" {
		mode := os.FileMode(0o644)
		if st, err := os.Stat(p.Path); err == nil {
			mode = st.Mode().Perm()
		}
		if err := os.MkdirAll(filepath.Dir(p.Path), 0o755); err != nil {
			return nil, &Error{Code: CodeInternal, Message: err.Error()}
		}
		if err := os.WriteFile(p.Path, []byte(p.Content), mode); err != nil {
			return nil, &Error{Code: CodeInternal, Message: err.Error()}
		}
		return nil, nil
	}
	b, err := os.ReadFile(p.Path)
	if os.IsNotExist(err) {
		return nil, &Error{Code: CodeNotFound, Message: err.Error()}
	}
	if err != nil {
		return nil, &Error{Code: CodeInternal, Message: err.Error()}
	}
	text := string(b)
	if p.Line > 1 || p.Limit > 0 {
		lines := strings.SplitAfter(text, "\n")
		from := min(max(p.Line, 1)-1, len(lines))
		to := len(lines)
		if p.Limit > 0 {
			to = min(from+p.Limit, to)
		}
		text = strings.Join(lines[from:to], "")
	}
	return map[string]string{"content": text}, nil
}
