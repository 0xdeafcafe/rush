package claude

import (
	"encoding/json/jsontext"
	"regexp"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// kinds is what each of Claude Code's tools does.
var kinds = map[string]tool.Kind{
	"Bash":            tool.Shell,
	"Read":            tool.Read,
	"Edit":            tool.Edit,
	"MultiEdit":       tool.Edit,
	"Write":           tool.Write,
	"NotebookEdit":    tool.Notebook,
	"Grep":            tool.Search,
	"Glob":            tool.Glob,
	"WebFetch":        tool.Fetch,
	"WebSearch":       tool.WebSearch,
	"Task":            tool.Subagent,
	"Agent":           tool.Subagent,
	"TodoWrite":       tool.Todo,
	"AskUserQuestion": tool.Question,
	"ExitPlanMode":    tool.PlanMode,
}

// KindOf is what a Claude Code tool does.
func KindOf(name string) tool.Kind {
	if k, ok := kinds[name]; ok {
		return k
	}
	if strings.HasPrefix(name, "mcp__") && strings.Contains(strings.TrimPrefix(name, "mcp__"), "__") {
		return tool.MCP
	}
	return tool.Other
}

// Call reads one of Claude Code's tool calls into rush's own.
func Call(id, name string, input jsontext.Value) tool.Call {
	c := tool.Call{ID: id, Name: name, Kind: kinds[name], Raw: input}
	var in struct {
		Command      string `json:"command"`
		Description  string `json:"description"`
		Background   bool   `json:"run_in_background"`
		Timeout      int    `json:"timeout"`
		FilePath     string `json:"file_path"`
		NotebookPath string `json:"notebook_path"`
		Path         string `json:"path"`
		Pattern      string `json:"pattern"`
		Query        string `json:"query"`
		URL          string `json:"url"`
		Prompt       string `json:"prompt"`
		SubagentType string `json:"subagent_type"`
		Content      string `json:"content"`
		OldString    string `json:"old_string"`
		NewString    string `json:"new_string"`
		ReplaceAll   bool   `json:"replace_all"`
		Edits        []struct {
			OldString  string `json:"old_string"`
			NewString  string `json:"new_string"`
			ReplaceAll bool   `json:"replace_all"`
		} `json:"edits"`
		Offset int `json:"offset"`
		Limit  int `json:"limit"`
		Todos  []struct {
			Content    string `json:"content"`
			ActiveForm string `json:"activeForm"`
			Status     string `json:"status"`
		} `json:"todos"`
		Questions []struct {
			Question string `json:"question"`
		} `json:"questions"`
	}
	_ = jsonx.Unmarshal(input, &in)
	x := &c.Input
	x.Description, x.Prompt = in.Description, in.Prompt
	switch c.Kind {
	case tool.Shell:
		x.Command, x.Background, x.Timeout = in.Command, in.Background, in.Timeout
	case tool.Read:
		x.Path, x.Offset, x.Limit = in.FilePath, in.Offset, in.Limit
	case tool.Edit:
		x.Path = in.FilePath
		if name == "MultiEdit" {
			for _, e := range in.Edits {
				x.Edits = append(x.Edits, tool.Replace{Old: e.OldString, New: e.NewString, All: e.ReplaceAll})
			}
		} else {
			x.Edits = []tool.Replace{{Old: in.OldString, New: in.NewString, All: in.ReplaceAll}}
		}
	case tool.Write:
		x.Path, x.Content = in.FilePath, in.Content
	case tool.Notebook:
		x.Path = in.NotebookPath
	case tool.Search, tool.Glob:
		x.Pattern, x.Path = in.Pattern, in.Path
	case tool.Fetch:
		x.URL = in.URL
	case tool.WebSearch:
		x.Query = in.Query
	case tool.Subagent:
		x.Agent = in.SubagentType
	case tool.Todo:
		for _, t := range in.Todos {
			x.Todos = append(x.Todos, tool.TodoItem{Label: t.Content, Active: t.ActiveForm, Status: t.Status})
		}
	case tool.Question:
		if len(in.Questions) > 0 {
			c.Title = in.Questions[0].Question
		}
	case tool.Other:
		if server, t, ok := strings.Cut(strings.TrimPrefix(name, "mcp__"), "__"); ok && strings.HasPrefix(name, "mcp__") {
			c.Kind = tool.MCP
			x.Server, x.Tool = strings.TrimPrefix(server, "claude_ai_"), t
			break
		}
		// Claude Code's own tools rush has no kind for: what they're
		// about, for drawing them by name.
		x.Command, x.Path, x.Pattern, x.URL, x.Query = in.Command, in.FilePath, in.Pattern, in.URL, in.Query
	}
	return c
}

var exitRe = regexp.MustCompile(`(?m)^(?:Error: )?Exit code (\d+)`)

// Output reads a tool result into rush's own: text is what Claude read,
// result is Claude Code's structured account of the run (its
// toolUseResult), when there is one.
func Output(c tool.Call, text string, isError bool, result jsontext.Value) tool.Output {
	o := tool.Output{CallID: c.ID, Text: text, IsError: isError, Raw: result}
	var r struct {
		Type            string       `json:"type"`
		Stdout          string       `json:"stdout"`
		Stderr          string       `json:"stderr"`
		StructuredPatch []tool.Patch `json:"structuredPatch"`
		File            struct {
			NumLines   int `json:"numLines"`
			StartLine  int `json:"startLine"`
			TotalLines int `json:"totalLines"`
		} `json:"file"`
	}
	_ = jsonx.Unmarshal(result, &r)
	switch c.Kind {
	case tool.Shell:
		o.Stdout, o.Stderr = r.Stdout, r.Stderr
		exit := 0
		if isError {
			exit = -1
			// "Exit code N" in a success's output is just output.
			if m := exitRe.FindStringSubmatch(text); m != nil {
				exit, _ = strconv.Atoi(m[1])
			}
		}
		o.Exit = &exit
	case tool.Edit, tool.Write:
		o.Patches = r.StructuredPatch
		o.Created = r.Type == "create"
	case tool.Read:
		if f := r.File; f.NumLines > 0 {
			o.Lines = &tool.Span{Start: f.StartLine, Count: f.NumLines, Total: f.TotalLines}
		}
	}
	return o
}
