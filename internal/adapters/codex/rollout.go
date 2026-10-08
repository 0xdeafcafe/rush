package codex

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json/jsontext"
	"io"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// A rollout is the JSONL file Codex writes each thread to, under
// $CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<uuid>.jsonl. Each line
// is {timestamp, ordinal, type, payload}.
//
// A rollout says most things twice: once as the response_items the model
// saw (messages, reasoning, the "exec" and "apply_patch" tool calls), and
// again as event_msg item_completed records, the same ThreadItems the
// app-server sends a live client. The items are the richer of the two
// (commands with their exit codes and output, file changes as diffs,
// user prompts without Codex's injected context), so a turn that has any
// item_completed record is read from its items alone, and a turn that has
// none, as older and guardian rollouts do, from its response_items. The
// event_msg user_message and agent_message records and the
// token_usage_record lines repeat what is read elsewhere and are skipped.
type rolloutLine struct {
	Timestamp string `json:"timestamp"`
	Type      string `json:"type"`
	Payload   view   `json:"payload"`
}

// view is a JSON value where the decoder found it, never copied: good for
// as long as the line it's in, which a rollout's reader never keeps.
type view []byte

func (v *view) UnmarshalJSONFrom(d *jsontext.Decoder) error {
	b, err := d.ReadValue()
	*v = view(b)
	return err
}

// maxLine is the longest line a rollout reader takes. Compactions and
// command outputs can run to several megabytes.
const maxLine = 256 << 20

// readers keeps readLines' 1MB buffers, and headReaders readHeadLines'
// 64KB ones: listing past threads reads the head of every rollout, and a
// fresh buffer each was most of what rush allocated. A head stops after a
// few hundred lines, so a smaller buffer reads less past them.
var readers, headReaders sync.Pool

// lineReader is a pooled reader and the buffer its lines longer than it
// are put together in, kept too: a rollout can have hundreds of them.
type lineReader struct {
	br   *bufio.Reader
	long []byte
}

// readLines calls fn with each line of r, however long, until fn says to
// stop. The line is only fn's until it returns.
func readLines(r io.Reader, fn func([]byte) bool) error {
	return readLinesWith(&readers, 1<<20, r, fn)
}

// readHeadLines is readLines for reading only the start of r.
func readHeadLines(r io.Reader, fn func([]byte) bool) error {
	return readLinesWith(&headReaders, 64<<10, r, fn)
}

func readLinesWith(pool *sync.Pool, size int, r io.Reader, fn func([]byte) bool) error {
	lr, _ := pool.Get().(*lineReader)
	if lr == nil {
		lr = &lineReader{br: bufio.NewReaderSize(r, size)}
	} else {
		lr.br.Reset(r)
	}
	defer func() {
		lr.br.Reset(nil)
		if cap(lr.long) > 32<<20 {
			lr.long = nil // one huge line needn't be held on to
		}
		pool.Put(lr)
	}()
	long := lr.long[:0]
	defer func() { lr.long = long }()
	for {
		chunk, err := lr.br.ReadSlice('\n')
		if err == bufio.ErrBufferFull {
			if len(long)+len(chunk) > maxLine {
				return bufio.ErrTooLong
			}
			long = append(long, chunk...)
			continue
		}
		line := chunk
		if len(long) > 0 {
			long = append(long, chunk...)
			line, long = long, long[:0]
		}
		if line = bytes.TrimSpace(line); len(line) > 0 && !fn(line) {
			return nil
		}
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
	}
}

func parseTime(s string) time.Time {
	t, _ := time.Parse(time.RFC3339Nano, s)
	return t
}

type sessionMeta struct {
	ID         string         `json:"id"`
	Timestamp  string         `json:"timestamp"`
	Cwd        string         `json:"cwd"`
	CLIVersion string         `json:"cli_version"`
	Source     jsontext.Value `json:"source"` // "cli", "exec", or {"subagent": ...}
	AgentPath  string         `json:"agent_path"`
}

// exec is whether the thread is a codex exec run.
func (m sessionMeta) exec() bool {
	var src string
	return jsonx.Unmarshal(m.Source, &src) == nil && src == "exec"
}

// subagent is whether the thread was started by another thread: a
// spawned agent or Codex's guardian reviewer.
func (m sessionMeta) subagent() bool {
	var src map[string]jsontext.Value
	if jsonx.Unmarshal(m.Source, &src) != nil {
		return false
	}
	_, ok := src["subagent"]
	return ok
}

type turnContext struct {
	Model          string `json:"model"`
	ApprovalPolicy string `json:"approval_policy"`
	Effort         string `json:"effort"`
}

// rolloutTokens is a rollout's token usage, in snake case.
type rolloutTokens struct {
	InputTokens           int64 `json:"input_tokens"`
	CachedInputTokens     int64 `json:"cached_input_tokens"`
	OutputTokens          int64 `json:"output_tokens"`
	ReasoningOutputTokens int64 `json:"reasoning_output_tokens"`
	TotalTokens           int64 `json:"total_tokens"`
}

func (t rolloutTokens) usage() usage.TokenUsage {
	return tokenBreakdown{TotalTokens: t.TotalTokens, InputTokens: t.InputTokens, CachedInputTokens: t.CachedInputTokens,
		OutputTokens: t.OutputTokens, ReasoningOutputTokens: t.ReasoningOutputTokens}.usage()
}

// rolloutLimits is a token_count's rate_limits: the app-server's
// RateLimitSnapshot in snake case.
type rolloutLimits struct {
	LimitID   string         `json:"limit_id"`
	LimitName string         `json:"limit_name"`
	Primary   *rolloutWindow `json:"primary"`
	Secondary *rolloutWindow `json:"secondary"`
	PlanType  string         `json:"plan_type"`
}

type rolloutWindow struct {
	UsedPercent   float64 `json:"used_percent"`
	WindowMinutes *int64  `json:"window_minutes"`
	ResetsAt      *int64  `json:"resets_at"`
}

func (l rolloutLimits) snapshot() rateLimitSnapshot {
	w := func(r *rolloutWindow) *rateLimitWindow {
		if r == nil {
			return nil
		}
		return &rateLimitWindow{UsedPercent: r.UsedPercent, WindowDurationMins: r.WindowMinutes, ResetsAt: r.ResetsAt}
	}
	return rateLimitSnapshot{LimitID: l.LimitID, LimitName: l.LimitName, Primary: w(l.Primary), Secondary: w(l.Secondary), PlanType: l.PlanType}
}

// eventMsg is an event_msg payload, with the fields of the types read.
type eventMsg struct {
	Type string `json:"type"`

	Item   jsontext.Value `json:"item"`    // item_completed
	TurnID string         `json:"turn_id"` // task_started, task_complete, item_completed

	DurationMs *int64 `json:"duration_ms"` // task_complete, turn_aborted
	Error      *struct {
		Message        string         `json:"message"`
		CodexErrorInfo jsontext.Value `json:"codex_error_info"`
	} `json:"error"` // task_complete
	Reason string `json:"reason"` // turn_aborted

	Info *struct {
		LastTokenUsage     rolloutTokens `json:"last_token_usage"`
		ModelContextWindow *int          `json:"model_context_window"`
	} `json:"info"` // token_count
	RateLimits jsontext.Value `json:"rate_limits"`

	ThreadSettings *struct {
		Model string `json:"model"`
	} `json:"thread_settings"` // thread_settings_applied
}

// rolloutItem is a ThreadItem as a rollout's item_completed records it:
// PascalCase types and snake-case fields, unlike the app-server's.
type rolloutItem struct {
	Type string `json:"type"`
	ID   string `json:"id"`

	Content     jsontext.Value `json:"content"`      // UserMessage, AgentMessage
	SummaryText []string       `json:"summary_text"` // Reasoning
	RawContent  jsontext.Value `json:"raw_content"`

	Command          jsontext.Value  `json:"command"` // CommandExecution: argv
	Cwd              string          `json:"cwd"`
	ParsedCmd        []parsedCommand `json:"parsed_cmd"`
	Status           string          `json:"status"`
	AggregatedOutput *string         `json:"aggregated_output"`
	ExitCode         *int            `json:"exit_code"`

	Changes map[string]rolloutChange `json:"changes"` // FileChange

	Path string `json:"path"` // ImageView

	Kind   string         `json:"kind"` // Extension: web.search, clock.sleep, image_gen.generation
	Query  string         `json:"query"`
	Action jsontext.Value `json:"action"`
}

type parsedCommand struct {
	Type  string `json:"type"` // read, search, list_files, unknown
	Name  string `json:"name"`
	Path  string `json:"path"`
	Query string `json:"query"`
}

type rolloutChange struct {
	Type        string `json:"type"` // add, delete, update
	Content     string `json:"content"`
	UnifiedDiff string `json:"unified_diff"` // hunks from "@@", with no file headers
	MovePath    string `json:"move_path"`
}

// argv is a command as one line: the script of a "sh -lc script", or
// the words joined.
func argv(raw jsontext.Value) string {
	var s string
	if jsonx.Unmarshal(raw, &s) == nil {
		return s
	}
	var words []string
	_ = jsonx.Unmarshal(raw, &words)
	if len(words) == 3 && (words[1] == "-lc" || words[1] == "-c") {
		return words[2]
	}
	return strings.Join(words, " ")
}

// threadItem is the item as the app-server would have sent it, so that
// callOf and outputOf read both alike. ok is false for an item that
// isn't a tool call.
func (it rolloutItem) threadItem() (threadItem, bool) {
	t := threadItem{ID: it.ID, Status: it.Status}
	switch it.Type {
	case "CommandExecution":
		t.Type, t.Command, t.Cwd = "commandExecution", argv(it.Command), it.Cwd
		t.AggregatedOutput, t.ExitCode = it.AggregatedOutput, it.ExitCode
		for _, p := range it.ParsedCmd {
			kind := p.Type
			if kind == "list_files" {
				kind = "listFiles"
			}
			t.CommandActions = append(t.CommandActions, commandAction{Type: kind, Name: p.Name, Path: p.Path, Query: p.Query})
		}
	case "FileChange":
		t.Type = "fileChange"
		paths := make([]string, 0, len(it.Changes))
		for p := range it.Changes {
			paths = append(paths, p)
		}
		sort.Strings(paths)
		for _, p := range paths {
			ch := it.Changes[p]
			u := fileUpdate{Path: p, Diff: ch.Content}
			u.Kind.Type, u.Kind.MovePath = ch.Type, ch.MovePath
			if ch.Type == "update" {
				u.Diff = ch.UnifiedDiff
			}
			t.Changes = append(t.Changes, u)
		}
	case "ImageView":
		t.Type, t.Path = "imageView", it.Path
	case "Extension":
		if it.Kind != "web.search" {
			return t, false
		}
		t.Type, t.Query = "webSearch", it.Query
		_ = jsonx.Unmarshal(it.Action, &t.Action)
	default:
		return t, false
	}
	return t, true
}

// source is which of a rollout's two accounts of a turn an event comes
// from.
type source int

const (
	fromBoth source = iota
	fromItem
	fromResponse
)

type tagged struct {
	src source
	ev  event.Event // nil for where Init goes
}

// replay turns a rollout's lines into events, a turn at a time.
type replay struct {
	out []event.Event

	turn   []tagged
	items  bool   // the turn has item_completed records
	turnID string // the last turn started, and whether it still runs
	inTurn bool

	meta      bool
	init      event.Init
	initModel bool
	agentPath string

	model     string
	tokens    usage.TokenUsage
	context   *event.Context
	lastQuota string
	prompted  bool // the first user-authored prompt has been seen
}

// line reads one rollout line.
func (r *replay) line(l rolloutLine) {
	switch l.Type {
	case "session_meta":
		if r.meta {
			return // a fork's parent, after the thread's own
		}
		var m sessionMeta
		if jsonx.Unmarshal(l.Payload, &m) != nil {
			return
		}
		r.meta = true
		r.init = event.Init{SessionID: m.ID, Cwd: m.Cwd, Version: m.CLIVersion}
		r.agentPath = m.AgentPath
		r.turn = append(r.turn, tagged{src: fromBoth}) // Init, once its model is known
	case "turn_context":
		var c turnContext
		if jsonx.Unmarshal(l.Payload, &c) != nil {
			return
		}
		if c.Model != "" {
			r.model = c.Model
		}
		if !r.initModel {
			r.init.Model, r.init.Mode, r.init.Effort, r.initModel = c.Model, c.ApprovalPolicy, c.Effort, true
		}
	case "compacted":
		var c struct {
			Latest *struct {
				Usage rolloutTokens `json:"usage"`
			} `json:"latest_token_usage_record"`
		}
		_ = jsonx.Unmarshal(l.Payload, &c)
		ev := event.Compacted{}
		if c.Latest != nil {
			ev.Before = int(c.Latest.Usage.TotalTokens)
		}
		r.add(fromBoth, ev)
	case "event_msg":
		r.eventMsg(l)
	case "response_item":
		r.responseItem(jsontext.Value(l.Payload))
	}
}

func (r *replay) add(src source, evs ...event.Event) {
	for _, ev := range evs {
		r.turn = append(r.turn, tagged{src: src, ev: ev})
	}
}

// flush keeps the turn's events from the source it is read from.
func (r *replay) flush() {
	drop := fromItem
	if r.items {
		drop = fromResponse
	}
	for _, t := range r.turn {
		switch {
		case t.src == drop:
		case t.ev == nil:
			r.out = append(r.out, r.init)
		default:
			r.out = append(r.out, t.ev)
		}
	}
	r.turn, r.items = r.turn[:0], false
}

func (r *replay) eventMsg(l rolloutLine) {
	var m eventMsg
	if jsonx.Unmarshal(l.Payload, &m) != nil {
		return
	}
	switch m.Type {
	case "task_started":
		r.tokens, r.context = usage.TokenUsage{}, nil
		r.turnID, r.inTurn = m.TurnID, true
	case "task_complete", "turn_aborted":
		r.inTurn = false
		end := event.TurnEnd{Reason: "done", Tokens: r.tokens, Turns: 1}
		if m.Type == "turn_aborted" {
			end.Reason = m.Reason
			if end.Reason == "" {
				end.Reason = "interrupted"
			}
		}
		if m.DurationMs != nil {
			end.Duration = time.Duration(*m.DurationMs) * time.Millisecond
		}
		if r.context != nil {
			r.add(fromBoth, *r.context)
		}
		if e := m.Error; e != nil {
			end.Reason, end.Err = "error", e.Message
			if string(e.CodexErrorInfo) == `"usage_limit_exceeded"` {
				r.add(fromBoth, event.Limited{})
			}
		}
		r.add(fromBoth, end)
		r.tokens, r.context = usage.TokenUsage{}, nil
		r.flush()
	case "token_count":
		if m.Info != nil {
			r.tokens.Add(m.Info.LastTokenUsage.usage())
			if w := m.Info.ModelContextWindow; w != nil {
				r.context = &event.Context{Tokens: int(m.Info.LastTokenUsage.TotalTokens), Window: *w}
			}
		}
		var lim rolloutLimits
		if len(m.RateLimits) > 0 && string(m.RateLimits) != "null" && jsonx.Unmarshal(m.RateLimits, &lim) == nil &&
			string(m.RateLimits) != r.lastQuota {
			r.lastQuota = string(m.RateLimits)
			q := quotaFrom(lim.snapshot())
			if t := parseTime(l.Timestamp); !t.IsZero() {
				q.FetchedAt = t
			}
			r.add(fromBoth, event.Quota{Quota: q})
		}
	case "thread_settings_applied":
		if s := m.ThreadSettings; s != nil && s.Model != "" {
			r.model = s.Model
		}
	case "item_completed":
		r.items = true
		// An item that completes after its turn ended ran beside the turns after it.
		outlived := m.TurnID != "" && r.turnID != "" && (m.TurnID != r.turnID || !r.inTurn)
		r.add(fromItem, r.item(m.Item, outlived)...)
	}
}

// item is what one completed item means as events. A command that
// outlived its turn ran in the background.
func (r *replay) item(raw jsontext.Value, outlived bool) []event.Event {
	var it rolloutItem
	if jsonx.Unmarshal(raw, &it) != nil {
		return nil
	}
	switch it.Type {
	case "UserMessage":
		var in []struct {
			Type string `json:"type"`
			Text string `json:"text"`
			Path string `json:"path"`
		}
		_ = jsonx.Unmarshal(it.Content, &in)
		m := event.Message{Role: "user", ID: it.ID}
		for _, p := range in {
			switch p.Type {
			case "text":
				m.Parts = append(m.Parts, event.Part{Kind: event.Text, Text: p.Text})
			case "local_image":
				m.Parts = append(m.Parts, event.Part{Kind: event.Image, Image: &event.ImageData{Path: p.Path}})
			}
		}
		if len(m.Parts) == 0 {
			return nil
		}
		return []event.Event{m}
	case "AgentMessage":
		var in []contentItem
		_ = jsonx.Unmarshal(it.Content, &in)
		if t := texts(in); t != "" {
			return []event.Event{r.assistant(it.ID, event.Part{Kind: event.Text, Text: t})}
		}
		return nil
	case "Reasoning":
		var raw []string
		_ = jsonx.Unmarshal(it.RawContent, &raw)
		if t := strings.TrimSpace(strings.Join(append(append([]string{}, it.SummaryText...), raw...), "\n\n")); t != "" {
			return []event.Event{r.assistant(it.ID, event.Part{Kind: event.Thinking, Text: t})}
		}
		return nil // encrypted, with no summary
	case "ContextCompaction":
		return nil // the compacted line says it
	}
	t, ok := it.threadItem()
	if !ok {
		return []event.Event{event.Other{Adapter: "codex", Type: "item/" + it.Type, Raw: raw}}
	}
	call, _ := callOf(t, raw)
	call.Input.Background = outlived && t.Type == "commandExecution"
	out := outputOf(t, raw)
	return []event.Event{r.assistant(call.ID, event.Part{Kind: event.ToolCall, Call: &call}), result(out)}
}

func (r *replay) assistant(id string, p event.Part) event.Message {
	return event.Message{Role: "assistant", ID: id, Model: r.model, Parts: []event.Part{p}}
}

func result(o tool.Output) event.Message {
	return event.Message{Role: "user", ID: o.CallID + ":result", Parts: []event.Part{{Kind: event.ToolResult, Output: &o}}}
}

// responseItem is a ResponseItem, with the fields of the types read.
type responseItem struct {
	Type    string `json:"type"`
	ID      string `json:"id"`
	Role    string `json:"role"` // message
	Content []struct {
		Type     string `json:"type"` // input_text, output_text, input_image
		Text     string `json:"text"`
		ImageURL string `json:"image_url"`
	} `json:"content"` // message, agent_message
	Summary []struct {
		Text string `json:"text"`
	} `json:"summary"` // reasoning

	CallID    string         `json:"call_id"` // function_call, custom_tool_call and their outputs
	Name      string         `json:"name"`
	Namespace string         `json:"namespace"`
	Arguments string         `json:"arguments"` // function_call: JSON
	Input     string         `json:"input"`     // custom_tool_call
	Output    jsontext.Value `json:"output"`    // a string, or content parts

	Recipient string `json:"recipient"` // agent_message
}

func (r *replay) responseItem(raw jsontext.Value) {
	var ri responseItem
	if jsonx.Unmarshal(raw, &ri) != nil {
		return
	}
	switch ri.Type {
	case "message":
		switch ri.Role {
		case "user":
			if !r.prompted {
				for _, c := range ri.Content {
					if c.Type == "input_text" && startupContext(c.Text) {
						r.add(fromBoth, event.StartNotice{Event: "SessionStart", Text: c.Text})
					}
				}
			}
			if m, ok := userPrompt(ri); ok {
				r.prompted = true
				r.add(fromResponse, m)
			}
		case "developer":
			if !r.prompted {
				for _, c := range ri.Content {
					if c.Type == "input_text" && strings.TrimSpace(c.Text) != "" {
						r.add(fromBoth, event.StartNotice{Event: "SessionStart", Text: c.Text})
					}
				}
			}
		case "assistant":
			var b []string
			for _, c := range ri.Content {
				if c.Text != "" {
					b = append(b, c.Text)
				}
			}
			if len(b) > 0 {
				r.add(fromResponse, r.assistant(ri.ID, event.Part{Kind: event.Text, Text: strings.Join(b, "\n")}))
			}
		}
	case "reasoning":
		var b []string
		for _, s := range ri.Summary {
			if s.Text != "" {
				b = append(b, s.Text)
			}
		}
		if len(b) > 0 {
			r.add(fromResponse, r.assistant(ri.ID, event.Part{Kind: event.Thinking, Text: strings.Join(b, "\n\n")}))
		}
	case "function_call", "custom_tool_call":
		call := responseCall(ri)
		r.add(fromResponse, r.assistant(call.ID, event.Part{Kind: event.ToolCall, Call: &call}))
	case "function_call_output", "custom_tool_call_output":
		r.add(fromResponse, result(tool.Output{CallID: ri.CallID, Text: outputText(ri.Output), Raw: raw}))
	case "agent_message":
		// A message from another agent. There is no item for one sent to
		// this thread, and it is how a spawned agent gets its task.
		self := r.agentPath
		if self == "" {
			self = "/root"
		}
		if ri.Recipient != self {
			return
		}
		m := event.Message{Role: "user", ID: ri.ID}
		for _, c := range ri.Content {
			if c.Type == "input_text" && c.Text != "" {
				m.Parts = append(m.Parts, event.Part{Kind: event.Text, Text: c.Text})
			}
		}
		if len(m.Parts) > 0 {
			r.add(fromBoth, m)
		}
	}
}

// userPrompt is a user message as the user wrote it, without the context
// Codex adds as user messages of its own. ok is false when nothing is
// left.
func userPrompt(ri responseItem) (event.Message, bool) {
	m := event.Message{Role: "user", ID: ri.ID}
	for _, c := range ri.Content {
		switch c.Type {
		case "input_text":
			if !injected(c.Text) {
				m.Parts = append(m.Parts, event.Part{Kind: event.Text, Text: c.Text})
			}
		case "input_image":
			if img := dataImage(c.ImageURL); img != nil {
				m.Parts = append(m.Parts, event.Part{Kind: event.Image, Image: img})
			} else if c.ImageURL != "" {
				m.Parts = append(m.Parts, event.Part{Kind: event.Image, Image: &event.ImageData{Path: c.ImageURL}})
			}
		}
	}
	for _, p := range m.Parts {
		if p.Kind == event.Text || p.Kind == event.Image {
			return m, true
		}
	}
	return m, false
}

// injected is whether a user message's text is context Codex adds rather
// than something the user typed: AGENTS.md, or a block wrapped in one
// tag such as <environment_context>, <user_instructions>,
// <recommended_plugins> or <turn_aborted>. Images come with text parts
// labelling them, "<image name=...>" and "</image>", which are Codex's
// too.
func injected(text string) bool {
	s := strings.TrimSpace(text)
	if strings.HasPrefix(s, "# AGENTS.md instructions") {
		return true
	}
	if strings.HasPrefix(s, "<image") || s == "</image>" {
		return true
	}
	if !strings.HasPrefix(s, "<") {
		return false
	}
	end := strings.IndexAny(s, "> ")
	if end < 2 {
		return false
	}
	tag := s[1:end]
	for _, c := range tag {
		if !(c == '_' || c == '-' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z') {
			return false
		}
	}
	return strings.HasSuffix(s, "</"+tag+">")
}

func startupContext(text string) bool {
	s := strings.TrimSpace(text)
	return injected(s) && !strings.HasPrefix(s, "<image") && s != "</image>" && !strings.HasPrefix(s, "<turn_aborted")
}

// dataImage is a data: URL's image.
func dataImage(url string) *event.ImageData {
	rest, ok := strings.CutPrefix(url, "data:")
	if !ok {
		return nil
	}
	head, data, ok := strings.Cut(rest, ",")
	media, isB64 := strings.CutSuffix(head, ";base64")
	if !ok || !isB64 {
		return nil
	}
	b, err := base64.StdEncoding.DecodeString(data)
	if err != nil {
		return nil
	}
	return &event.ImageData{MediaType: media, Data: b}
}

// outputText is a tool output as text: a string, or content parts.
func outputText(raw jsontext.Value) string {
	var s string
	if jsonx.Unmarshal(raw, &s) == nil {
		return s
	}
	var parts []contentItem
	_ = jsonx.Unmarshal(raw, &parts)
	return texts(parts)
}

// agentCall is one of Codex's agent tools as the Claude Code tool it is
// like. What it said is kept encrypted, so only a plain message is shown.
func agentCall(c tool.Call, ri responseItem) tool.Call {
	var a struct {
		Target  string `json:"target"`
		Message string `json:"message"`
	}
	_ = jsonx.Unmarshal([]byte(ri.Arguments), &a)
	if strings.HasPrefix(a.Message, "gAAAA") {
		a.Message = ""
	}
	switch ri.Name {
	case "send_message", "resume_agent":
		asClaude(&c, "SendMessage", map[string]any{"to": a.Target, "message": a.Message})
	case "followup_task":
		asClaude(&c, "SendMessage", map[string]any{"to": a.Target, "message": a.Message, "summary": "a follow-up task"})
	case "interrupt_agent", "close_agent":
		asClaude(&c, "TaskStop", map[string]any{"task_id": a.Target})
	case "list_agents":
		asClaude(&c, "ListAgents", map[string]any{})
	default: // wait_agent
		asClaude(&c, "TaskOutput", map[string]any{"task_id": "its subagents"})
	}
	return c
}

// responseCall is a function or custom tool call as the model made it.
// Turns without items are read from these; the commands Codex's "exec"
// tool runs are only known from items.
func responseCall(ri responseItem) tool.Call {
	c := tool.Call{ID: ri.CallID, Name: ri.Name, Raw: jsontext.Value(ri.Arguments)}
	if ri.Type == "custom_tool_call" {
		c.Raw, _ = jsonx.Marshal(ri.Input)
	}
	switch ri.Name {
	case "exec_command", "shell", "local_shell", "shell_command":
		var a struct {
			Cmd     jsontext.Value `json:"cmd"`
			Command jsontext.Value `json:"command"`
			Workdir string         `json:"workdir"`
			Timeout int            `json:"timeout_ms"`
		}
		_ = jsonx.Unmarshal([]byte(ri.Arguments), &a)
		cmd := a.Cmd
		if len(cmd) == 0 {
			cmd = a.Command
		}
		c.Kind, c.Input.Command, c.Input.Cwd, c.Input.Timeout = tool.Shell, argv(cmd), a.Workdir, a.Timeout
	case "apply_patch":
		c.Kind = tool.Edit
		for _, ln := range strings.Split(ri.Input, "\n") {
			for prefix, k := range map[string]tool.Kind{"*** Update File: ": tool.Edit, "*** Add File: ": tool.Write, "*** Delete File: ": tool.Delete} {
				if p, ok := strings.CutPrefix(ln, prefix); ok && c.Input.Path == "" {
					c.Kind, c.Input.Path = k, strings.TrimSpace(p)
				}
			}
		}
	case "view_image":
		var a struct {
			Path string `json:"path"`
		}
		_ = jsonx.Unmarshal([]byte(ri.Arguments), &a)
		c.Kind, c.Input.Path = tool.Read, a.Path
	case "spawn_agent":
		var a struct {
			Task  string `json:"task_name"`
			Type  string `json:"agent_type"`
			Model string `json:"model"`
		}
		_ = jsonx.Unmarshal([]byte(ri.Arguments), &a)
		// Its message is kept encrypted: the task names it.
		c.Kind, c.Input.Description, c.Input.Child, c.Input.Agent = tool.Subagent, a.Task, a.Task, a.Type
		if a.Type == "" {
			c.Input.Agent = a.Model
		}
	case "send_message", "followup_task", "resume_agent", "interrupt_agent", "close_agent", "wait_agent", "list_agents":
		return agentCall(c, ri)
	}
	if ri.Namespace != "" {
		c.Name = ri.Namespace + "." + ri.Name
	}
	return c
}
