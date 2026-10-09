package codex

import (
	"encoding/json/jsontext"
	"net/url"
	"path"
	"regexp"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// threadItem is Codex's ThreadItem, with the fields of every type rush
// reads; which are set depends on Type.
type threadItem struct {
	Type string `json:"type"`
	ID   string `json:"id"`

	Text    string         `json:"text"`    // agentMessage
	Content jsontext.Value `json:"content"` // userMessage: []UserInput; reasoning: []string
	Summary []string       `json:"summary"` // reasoning

	Command          string          `json:"command"` // commandExecution
	Cwd              string          `json:"cwd"`
	CommandActions   []commandAction `json:"commandActions"`
	AggregatedOutput *string         `json:"aggregatedOutput"`
	ExitCode         *int            `json:"exitCode"`
	Status           string          `json:"status"`
	ProcessID        string          `json:"processId"` // its terminal, which can outlive the call

	Changes []fileUpdate `json:"changes"` // fileChange

	Server       string                    `json:"server"` // mcpToolCall
	Tool         string                    `json:"tool"`   // mcpToolCall, dynamicToolCall, collabAgentToolCall
	Arguments    jsontext.Value            `json:"arguments"`
	Result       jsontext.Value            `json:"result"`
	Error        *struct{ Message string } `json:"error"`
	ContentItems []contentItem             `json:"contentItems"` // dynamicToolCall
	Success      *bool                     `json:"success"`

	Query  string `json:"query"` // webSearch
	Action *struct {
		Type    string   `json:"type"`
		Query   string   `json:"query"`
		Queries []string `json:"queries"`
		URL     string   `json:"url"`
		Pattern string   `json:"pattern"`
	} `json:"action"`
	Results jsontext.Value `json:"results"`

	Path string `json:"path"` // imageView

	Prompt    string   `json:"prompt"` // collabAgentToolCall
	Receivers []string `json:"receiverThreadIds"`
	Model     string   `json:"model"`

	Kind          string `json:"kind"` // subAgentActivity: started, interacted, interrupted, completed
	AgentThreadID string `json:"agentThreadId"`
	AgentPath     string `json:"agentPath"`
}

type commandAction struct {
	Type  string `json:"type"` // read, listFiles, search, unknown
	Name  string `json:"name"`
	Path  string `json:"path"`
	Query string `json:"query"`
}

type fileUpdate struct {
	Path string `json:"path"`
	Kind struct {
		Type     string `json:"type"` // add, delete, update
		MovePath string `json:"move_path"`
	} `json:"kind"`
	Diff string `json:"diff"`
}

type contentItem struct {
	Type string `json:"type"`
	Text string `json:"text"`
}

// userInput is one of Codex's UserInput parts.
type userInput struct {
	Type string `json:"type"`
	Text string `json:"text"`
	URL  string `json:"url"`
	Path string `json:"path"`
	Name string `json:"name"`
}

// callOf is the tool call an item is, if it is one.
func callOf(it threadItem, raw jsontext.Value) (tool.Call, bool) {
	c := tool.Call{ID: it.ID, Raw: raw}
	switch it.Type {
	case "commandExecution":
		c.Name, c.Kind = "shell", tool.Shell
		c.Input.Command, c.Input.Cwd = script(it.Command), it.Cwd
		// Codex reads one-step commands itself; a lone read or search
		// is drawn as one.
		if len(it.CommandActions) == 1 {
			switch a := it.CommandActions[0]; a.Type {
			case "read":
				c.Kind, c.Input.Path = tool.Read, a.Path
			case "search":
				if a.Query != "" {
					c.Kind, c.Input.Pattern, c.Input.Path = tool.Search, a.Query, a.Path
				}
			}
		}
	case "fileChange":
		c.Name, c.Kind = "apply_patch", tool.Edit
		if len(it.Changes) > 0 {
			ch := it.Changes[0]
			c.Input.Path = ch.Path
			if len(it.Changes) == 1 {
				switch ch.Kind.Type {
				case "add":
					c.Kind = tool.Write
					if !isDiff(ch.Diff) {
						c.Input.Content = ch.Diff
					}
				case "delete":
					c.Kind = tool.Delete
				case "update":
					if ch.Kind.MovePath != "" {
						c.Kind, c.Input.To = tool.Move, ch.Kind.MovePath
					}
				}
			}
		}
	case "mcpToolCall":
		c.Name, c.Kind = "mcp__"+it.Server+"__"+it.Tool, tool.MCP // as Claude Code names them
		c.Input.Server, c.Input.Tool = it.Server, it.Tool
		c.Raw = it.Arguments
	case "dynamicToolCall":
		c.Name, c.Raw = it.Tool, it.Arguments
	case "webSearch":
		c.Name, c.Kind, c.Input.Query = "web_search", tool.WebSearch, it.Query
		if a := it.Action; a != nil {
			switch a.Type {
			case "search":
				if c.Input.Query == "" {
					c.Input.Query = a.Query
					if a.Query == "" && len(a.Queries) > 0 {
						c.Input.Query = strings.Join(a.Queries, ", ")
					}
				}
			case "openPage", "findInPage":
				c.Kind, c.Input.URL, c.Input.Pattern = tool.Fetch, a.URL, a.Pattern
			}
		}
	case "imageView":
		c.Name, c.Kind, c.Input.Path = "view_image", tool.Read, localPath(it.Path)
	case "collabAgentToolCall":
		to := ""
		if len(it.Receivers) > 0 {
			to = it.Receivers[0]
		}
		switch it.Tool {
		case "spawnAgent":
			c.Name, c.Kind, c.Input.Prompt, c.Input.Agent, c.Input.Child = it.Tool, tool.Subagent, it.Prompt, it.Model, to
		case "sendInput", "sendMessage", "followupTask", "resumeAgent":
			asClaude(&c, "SendMessage", map[string]any{"to": to, "message": it.Prompt})
		case "interruptAgent", "closeAgent":
			asClaude(&c, "TaskStop", map[string]any{"task_id": to})
		case "listAgents":
			asClaude(&c, "ListAgents", map[string]any{})
		default: // wait
			asClaude(&c, "TaskOutput", map[string]any{"task_id": or(to, "its subagents")})
		}
	case "subAgentActivity":
		// Codex 0.155 says its agent tools this way: the thread, by its task.
		task := path.Base(it.AgentPath)
		switch it.Kind {
		case "started":
			c.Name, c.Kind, c.Input.Child, c.Input.Description = "spawn_agent", tool.Subagent, it.AgentThreadID, task
		case "interacted":
			asClaude(&c, "SendMessage", map[string]any{"to": task})
		case "interrupted":
			asClaude(&c, "TaskStop", map[string]any{"task_id": task})
		default: // completed: its turn ended, which its task says
			return tool.Call{}, false
		}
	default:
		return tool.Call{}, false
	}
	return c, true
}

// asClaude makes c a call to Claude Code's tool name, with its input, so
// it's drawn as that: a message, a stop, a check on its agents.
func asClaude(c *tool.Call, name string, in map[string]any) {
	c.Name, c.Kind = name, tool.Other
	c.Raw, _ = jsonx.Marshal(in)
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

// outputOf is what a finished tool call item returned.
func outputOf(it threadItem, raw jsontext.Value) tool.Output {
	o := tool.Output{CallID: it.ID, Raw: raw}
	failed := it.Status == "failed" || it.Status == "declined"
	switch it.Type {
	case "commandExecution":
		if it.AggregatedOutput != nil {
			o.Text, o.Stdout = *it.AggregatedOutput, *it.AggregatedOutput
		}
		o.Exit = it.ExitCode
		o.IsError = failed || (it.ExitCode != nil && *it.ExitCode != 0)
		if it.Status == "declined" && o.Text == "" {
			o.Text = "declined"
		}
	case "fileChange":
		for _, ch := range it.Changes {
			switch {
			case isDiff(ch.Diff):
				o.Patches = append(o.Patches, parseDiff(ch.Path, ch.Diff)...)
			case ch.Kind.Type == "add":
				o.Patches = append(o.Patches, wholeFile(ch.Path, ch.Diff, true)...)
			case ch.Kind.Type == "delete":
				o.Patches = append(o.Patches, wholeFile(ch.Path, ch.Diff, false)...)
			}
			if ch.Kind.Type == "add" {
				o.Created = true
			}
		}
		o.IsError = failed
	case "mcpToolCall":
		var res struct {
			Content []contentItem `json:"content"`
		}
		_ = jsonx.Unmarshal(it.Result, &res)
		o.Text = texts(res.Content)
		if it.Error != nil {
			o.IsError, o.Text = true, it.Error.Message
		}
		o.IsError = o.IsError || failed
	case "dynamicToolCall":
		o.Text = texts(it.ContentItems)
		o.IsError = failed || (it.Success != nil && !*it.Success)
	default:
		o.IsError = failed
	}
	return o
}

func texts(cs []contentItem) string {
	var b []string
	for _, c := range cs {
		if c.Text != "" {
			b = append(b, c.Text)
		}
	}
	return strings.Join(b, "\n")
}

// userMessage is a userMessage item as a user Message.
func userMessage(it threadItem) event.Message {
	var in []userInput
	_ = jsonx.Unmarshal(it.Content, &in)
	m := event.Message{Role: "user", ID: it.ID}
	for _, p := range in {
		switch p.Type {
		case "text":
			m.Parts = append(m.Parts, event.Part{Kind: event.Text, Text: p.Text})
		case "localImage":
			m.Parts = append(m.Parts, event.Part{Kind: event.Image, Image: &event.ImageData{Path: p.Path}})
		case "image":
			img := dataImage(p.URL)
			if img == nil {
				img = &event.ImageData{Path: p.URL}
			}
			m.Parts = append(m.Parts, event.Part{Kind: event.Image, Image: img})
		}
	}
	return m
}

// reasoningText is a reasoning item's summary and, when Codex sends it,
// its raw reasoning.
func reasoningText(it threadItem) string {
	var content []string
	_ = jsonx.Unmarshal(it.Content, &content)
	return strings.TrimSpace(strings.Join(append(append([]string{}, it.Summary...), content...), "\n\n"))
}

// todoStatus is a plan step's status in rush's words.
func todoStatus(s string) string {
	if s == "inProgress" {
		return "in_progress"
	}
	return s
}

// shellRe is a command Codex wrapped in a login shell: /bin/zsh -lc '…',
// or the same in double quotes.
var shellRe = regexp.MustCompile(`^(?:\S*/)?(?:ba|z)?sh -l?c (?:'((?:[^']|'\\'')*)'|"((?:[^"\\]|\\.)*)")$`)

// dquoted are the escapes a shell undoes inside double quotes.
var dquoted = strings.NewReplacer(`\"`, `"`, `\\`, `\`, `\$`, `$`, "\\`", "`")

// script is what a command runs, out of the shell Codex runs it in.
func script(cmd string) string {
	m := shellRe.FindStringSubmatch(cmd)
	switch {
	case m == nil:
		return cmd
	case m[1] != "" || !strings.Contains(cmd, `"`):
		return strings.ReplaceAll(m[1], `'\''`, "'")
	}
	return dquoted.Replace(m[2])
}

// localPath is a file:// URL, as Codex records the images it views, as the
// path it names; anything else as it is.
func localPath(p string) string {
	if u, err := url.Parse(p); err == nil && u.Scheme == "file" {
		return u.Path
	}
	return p
}
