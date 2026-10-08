package pi

import (
	"encoding/base64"
	"encoding/json/jsontext"
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// message is Pi's AgentMessage, with the fields of every role rush reads;
// which are set depends on Role.
type message struct {
	Role    string         `json:"role"` // user, assistant, toolResult, bashExecution, custom, branchSummary, compactionSummary
	Content jsontext.Value `json:"content"`

	Model        string  `json:"model"` // assistant
	Provider     string  `json:"provider"`
	Usage        *tokens `json:"usage"`
	StopReason   string  `json:"stopReason"`
	ErrorMessage string  `json:"errorMessage"`
	ResponseID   string  `json:"responseId"`
	Timestamp    int64   `json:"timestamp"`

	ToolCallID string         `json:"toolCallId"` // toolResult
	ToolName   string         `json:"toolName"`
	IsError    bool           `json:"isError"`
	Details    jsontext.Value `json:"details"`

	Command   string `json:"command"` // bashExecution
	Output    string `json:"output"`
	ExitCode  *int   `json:"exitCode"`
	Cancelled bool   `json:"cancelled"`

	CustomType string `json:"customType"` // custom
	Display    bool   `json:"display"`

	Summary      string `json:"summary"` // branchSummary, compactionSummary
	TokensBefore int    `json:"tokensBefore"`
}

// block is one of a message's content blocks.
type block struct {
	Type      string         `json:"type"` // text, thinking, image, toolCall
	Text      string         `json:"text"`
	Thinking  string         `json:"thinking"`
	Data      string         `json:"data"`
	MimeType  string         `json:"mimeType"`
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	Arguments jsontext.Value `json:"arguments"`
}

// tokens is Pi's Usage. Its input is what wasn't read from the cache.
type tokens struct {
	Input       int64 `json:"input"`
	Output      int64 `json:"output"`
	CacheRead   int64 `json:"cacheRead"`
	CacheWrite  int64 `json:"cacheWrite"`
	Reasoning   int64 `json:"reasoning"`
	TotalTokens int64 `json:"totalTokens"`
	Cost        struct {
		Total float64 `json:"total"`
	} `json:"cost"`
}

func (t tokens) usage() usage.TokenUsage {
	return usage.TokenUsage{Input: t.Input, Output: t.Output, CacheRead: t.CacheRead, CacheWrite5m: t.CacheWrite, Reasoning: t.Reasoning}
}

// context is how much of the window the request filled: all it read and
// wrote.
func (t tokens) context() int {
	if t.TotalTokens > 0 {
		return int(t.TotalTokens)
	}
	return int(t.Input + t.Output + t.CacheRead + t.CacheWrite)
}

// blocks are a message's content: a bare string is one text block.
func (m *message) blocks() []block {
	if len(m.Content) == 0 {
		return nil
	}
	if m.Content.Kind() == '"' {
		var s string
		_ = jsonx.Unmarshal(m.Content, &s)
		return []block{{Type: "text", Text: s}}
	}
	var bs []block
	_ = jsonx.Unmarshal(m.Content, &bs)
	return bs
}

// text is a message's text blocks, joined.
func (m *message) text() string {
	var out []string
	bs := m.blocks()
	for i := range bs {
		if bs[i].Type == "text" && bs[i].Text != "" {
			out = append(out, bs[i].Text)
		}
	}
	return strings.Join(out, "\n")
}

// id is what an assistant message is known by: the provider's response id,
// else its time.
func (m *message) id() string {
	if m.ResponseID != "" {
		return m.ResponseID
	}
	return "pi-" + strconv.FormatInt(m.Timestamp, 10)
}

// events are what a whole message says, as a live session and a session
// read back both tell it; nil for one rush doesn't show.
func (m *message) events(id string) []event.Event {
	switch m.Role {
	case "user":
		if out := m.user(id); len(out.Parts) > 0 {
			return []event.Event{out}
		}
	case "assistant":
		if out := m.assistant(id); len(out.Parts) > 0 || out.Tokens != nil {
			return []event.Event{out}
		}
	case "toolResult":
		o := outputOf(m)
		return []event.Event{event.Message{Role: "user", ID: m.ToolCallID + ":result", Parts: []event.Part{{Kind: event.ToolResult, Output: &o}}}}
	case "bashExecution":
		return m.bash()
	case "custom":
		if t := m.text(); m.Display && t != "" {
			return []event.Event{injected(id, t)}
		}
	case "branchSummary":
		if m.Summary != "" {
			return []event.Event{injected(id, m.Summary)}
		}
	case "compactionSummary":
		return []event.Event{event.Compacted{Before: m.TokensBefore}}
	}
	return nil
}

// user is a message of yours: its text and images.
func (m *message) user(id string) event.Message {
	out := event.Message{Role: "user", ID: id}
	bs := m.blocks()
	for i := range bs {
		switch b := &bs[i]; b.Type {
		case "text":
			out.Parts = append(out.Parts, event.Part{Kind: event.Text, Text: b.Text})
		case "image":
			out.Parts = append(out.Parts, event.Part{Kind: event.Image, Image: image(b)})
		}
	}
	return out
}

// assistant is the model's message: its text, thinking and tool calls,
// and what it cost in tokens.
func (m *message) assistant(id string) event.Message {
	out := event.Message{Role: "assistant", ID: id, Model: m.Model}
	bs := m.blocks()
	for i := range bs {
		switch b := &bs[i]; {
		case b.Type == "text" && b.Text != "":
			out.Parts = append(out.Parts, event.Part{Kind: event.Text, Text: b.Text})
		case b.Type == "thinking" && b.Thinking != "":
			out.Parts = append(out.Parts, event.Part{Kind: event.Thinking, Text: b.Thinking})
		case b.Type == "toolCall":
			c := callOf(b)
			out.Parts = append(out.Parts, event.Part{Kind: event.ToolCall, Call: &c})
		}
	}
	if m.Usage != nil {
		u := m.Usage.usage()
		out.Tokens = &u
	}
	return out
}

// bash is a command you ran yourself with !, shown as its call and result.
func (m *message) bash() []event.Event {
	c := tool.Call{ID: "bash-" + strconv.FormatInt(m.Timestamp, 10), Name: "bash", Kind: tool.Shell, Input: tool.Input{Command: m.Command}}
	o := tool.Output{CallID: c.ID, Text: m.Output, Stdout: m.Output, Exit: m.ExitCode,
		IsError: m.Cancelled || (m.ExitCode != nil && *m.ExitCode != 0)}
	return []event.Event{
		event.Message{Role: "assistant", ID: c.ID, Parts: []event.Part{{Kind: event.ToolCall, Call: &c}}},
		event.Message{Role: "user", ID: c.ID + ":result", Parts: []event.Part{{Kind: event.ToolResult, Output: &o}}},
	}
}

// injected is text pi put in the conversation itself.
func injected(id, text string) event.Message {
	return event.Message{Role: "user", ID: id, Injected: true, Parts: []event.Part{{Kind: event.Text, Text: text}}}
}

func image(b *block) *event.ImageData {
	data, _ := base64.StdEncoding.DecodeString(b.Data)
	return &event.ImageData{MediaType: b.MimeType, Data: data}
}

// args are the arguments of Pi's built-in tools, all of them at once.
type args struct {
	Command string  `json:"command"` // bash
	Timeout float64 `json:"timeout"` // bash, seconds
	Path    string  `json:"path"`    // read, edit, write, grep, find, ls
	Offset  int     `json:"offset"`  // read
	Limit   int     `json:"limit"`   // read
	Content string  `json:"content"` // write
	Pattern string  `json:"pattern"` // grep, find
	Edits   []struct {
		OldText string `json:"oldText"`
		NewText string `json:"newText"`
	} `json:"edits"` // edit
	OldText string    `json:"oldText"` // edit, as older pis took one
	NewText string    `json:"newText"`
	Agent   string    `json:"agent"` // subagent
	Task    string    `json:"task"`
	Tasks   []subtask `json:"tasks"` // subagent, in parallel
	Chain   []subtask `json:"chain"` // subagent, one after another
}

// subtask is a subagent extension's task: which agent, and what to do.
type subtask struct {
	Agent string `json:"agent"`
	Task  string `json:"task"`
}

// callOf is a tool call as rush draws it. Pi's own tools are read, bash
// (or powershell), edit, write, grep, find and ls; the subagent extension's
// is a subagent; other extensions' are drawn by name.
func callOf(b *block) tool.Call {
	c := tool.Call{ID: b.ID, Name: b.Name, Raw: b.Arguments}
	var a args
	_ = jsonx.Unmarshal(b.Arguments, &a)
	switch b.Name {
	case "bash", "powershell":
		c.Kind, c.Input.Command, c.Input.Timeout = tool.Shell, a.Command, int(a.Timeout*1000)
	case "read":
		c.Kind, c.Input.Path, c.Input.Offset, c.Input.Limit = tool.Read, a.Path, a.Offset, a.Limit
	case "edit":
		c.Kind, c.Input.Path = tool.Edit, a.Path
		for _, e := range a.Edits {
			c.Input.Edits = append(c.Input.Edits, tool.Replace{Old: e.OldText, New: e.NewText})
		}
		if len(a.Edits) == 0 && (a.OldText != "" || a.NewText != "") {
			c.Input.Edits = []tool.Replace{{Old: a.OldText, New: a.NewText}}
		}
	case "write":
		c.Kind, c.Input.Path, c.Input.Content = tool.Write, a.Path, a.Content
	case "grep":
		c.Kind, c.Input.Pattern, c.Input.Path = tool.Search, a.Pattern, a.Path
	case "find":
		c.Kind, c.Input.Pattern, c.Input.Path = tool.Glob, a.Pattern, a.Path
	case "ls":
		c.Kind, c.Input.Pattern, c.Input.Path = tool.Glob, "*", a.Path
	case "subagent":
		c.Kind, c.Input.Agent, c.Input.Prompt = tool.Subagent, a.Agent, a.Task
		if many := slices.Concat(a.Tasks, a.Chain); len(many) > 0 {
			var agents, tasks []string
			for _, t := range many {
				agents, tasks = append(agents, t.Agent), append(tasks, t.Agent+": "+t.Task)
			}
			c.Input.Agent, c.Input.Prompt = strings.Join(agents, ", "), strings.Join(tasks, "\n\n")
		}
		c.Input.Description = oneLine(c.Input.Prompt)
	}
	return c
}

// outputOf is what a tool result says. An edit's details carry its diff.
func outputOf(m *message) tool.Output {
	o := tool.Output{CallID: m.ToolCallID, Text: m.text(), IsError: m.IsError, Raw: m.Details}
	if m.ToolName == "bash" || m.ToolName == "powershell" {
		o.Stdout = o.Text
	}
	if m.ToolName == "edit" && len(m.Details) > 0 {
		var d struct {
			Diff string `json:"diff"`
		}
		if jsonx.Unmarshal(m.Details, &d) == nil && d.Diff != "" {
			o.Patches = parseDiff(d.Diff)
		}
	}
	return o
}

// parseDiff reads Pi's own diff: a line per change, "+12 text", "-12
// text" or " 12 text", numbered in the new file, the old one and the old
// one again, with a "..." line between hunks.
func parseDiff(diff string) []tool.Patch {
	var out []tool.Patch
	var cur *tool.Patch
	shift := 0 // lines added less lines removed, before the hunk being read
	for ln := range strings.SplitSeq(strings.TrimRight(diff, "\n"), "\n") {
		if ln == "" {
			continue
		}
		sign, rest := ln[:1], ln[1:]
		numText, text, _ := strings.Cut(strings.TrimLeft(rest, " "), " ")
		if sign == " " && numText == "..." {
			if cur != nil {
				shift += cur.NewLines - cur.OldLines
				out, cur = append(out, *cur), nil
			}
			continue
		}
		n, err := strconv.Atoi(numText)
		if err != nil || (sign != "+" && sign != "-" && sign != " ") {
			continue
		}
		if cur == nil {
			cur = &tool.Patch{}
			switch sign {
			case "+":
				cur.NewStart, cur.OldStart = n, n-shift
			default:
				cur.OldStart, cur.NewStart = n, n+shift
			}
		}
		cur.Lines = append(cur.Lines, sign+text)
		switch sign {
		case "+":
			cur.NewLines++
		case "-":
			cur.OldLines++
		default:
			cur.OldLines++
			cur.NewLines++
		}
	}
	if cur != nil {
		out = append(out, *cur)
	}
	return out
}

// reason is how a run ended, in rush's words, from its last message's stop
// reason.
func reason(stop string) string {
	switch stop {
	case "", "stop", "toolUse":
		return "done"
	case "aborted":
		return "interrupted"
	case "length":
		return "max_tokens"
	}
	return stop // error
}
