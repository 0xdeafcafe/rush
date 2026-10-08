package convo

import (
	"encoding/json/jsontext"
	"fmt"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// A step keeps its call in Claude Code's words (Step.Tool, Step.Input,
// Step.Result) beside rush's own, for what still reads those: another
// agent's call is put as the Claude tool of its kind.

// stepTool is a call as a step's Tool and Input: Claude Code's own as it
// sent them, or another agent's as the Claude tool of its kind.
func stepTool(c *tool.Call) (string, jsontext.Value) {
	if claudes(c) {
		return c.Name, c.Raw
	}
	return claudeTool(c)
}

// claudes says a call is Claude Code's own: one of its tools, of that
// tool's kind, with the input it sent.
func claudes(c *tool.Call) bool {
	return len(c.Raw) > 0 && c.Raw[0] == '{' && kindOf(c.Name) == c.Kind
}

// The native agent (agent.Native) is Claude Code: what its tools do, and
// its calls as rush's own, come from it.

func native() agent.Native {
	n, _ := agent.NativeAgent()
	return n
}

// kindOf is what the native agent's tool of this name does.
func kindOf(name string) tool.Kind {
	if n := native(); n != nil {
		return n.KindOf(name)
	}
	return tool.Other
}

// nativeCall is a call in the native agent's words as rush's own.
func nativeCall(id, name string, input jsontext.Value) tool.Call {
	if n := native(); n != nil {
		return n.Call(id, name, input)
	}
	return tool.Call{ID: id, Name: name, Raw: input}
}

// nativeOutput is how call c came out, from what the native agent said.
func nativeOutput(c tool.Call, text string, isError bool, result jsontext.Value) tool.Output { //nolint:gocritic // agent.Native's signature
	if n := native(); n != nil {
		return n.Output(c, text, isError, result)
	}
	return tool.Output{CallID: c.ID, Text: text, IsError: isError}
}

// nativeDoing is a call in the native agent's words, in a few words.
func nativeDoing(name string, input jsontext.Value) string {
	c := tool.Call{Name: name, Raw: input}
	if d, ok := native().(agent.Describer); ok {
		return d.Doing(&c)
	}
	return tool.Doing(c)
}

// claudeNames are Claude Code's tools for each kind of call.
var claudeNames = map[tool.Kind]string{
	tool.Shell: "Bash", tool.Read: "Read", tool.Edit: "Edit", tool.Write: "Write", tool.Search: "Grep",
	tool.Glob: "Glob", tool.Fetch: "WebFetch", tool.WebSearch: "WebSearch", tool.Subagent: "Agent",
	tool.Todo: "TodoWrite", tool.Question: "AskUserQuestion", tool.PlanMode: "ExitPlanMode", tool.Notebook: "NotebookEdit",
}

// claudeTool is a call as the Claude tool of its kind, with that tool's
// input; a call of no Claude kind keeps its name and input.
func claudeTool(c *tool.Call) (string, jsontext.Value) { //nolint:gocyclo // one case per kind of call
	in := c.Input
	name, ok := claudeNames[c.Kind]
	var v map[string]any
	switch {
	case c.Kind == tool.MCP:
		name = "mcp__" + in.Server + "__" + in.Tool
	case !ok:
		name = c.Name
		if name == "" {
			name = c.Kind.String()
		}
	}
	switch c.Kind {
	case tool.Shell:
		v = map[string]any{"command": in.Command, "description": in.Description, "run_in_background": in.Background}
	case tool.Read:
		v = map[string]any{"file_path": in.Path}
		if in.Offset > 0 {
			v["offset"] = in.Offset
		}
		if in.Limit > 0 {
			v["limit"] = in.Limit
		}
	case tool.Edit:
		v = map[string]any{"file_path": in.Path}
		switch len(in.Edits) {
		case 0:
		case 1:
			v["old_string"], v["new_string"], v["replace_all"] = in.Edits[0].Old, in.Edits[0].New, in.Edits[0].All
		default:
			name = "MultiEdit"
			edits := make([]map[string]any, 0, len(in.Edits))
			for _, e := range in.Edits {
				edits = append(edits, map[string]any{"old_string": e.Old, "new_string": e.New, "replace_all": e.All})
			}
			v["edits"] = edits
		}
	case tool.Write:
		v = map[string]any{"file_path": in.Path, "content": in.Content}
	case tool.Notebook:
		v = map[string]any{"notebook_path": in.Path}
	case tool.Search, tool.Glob:
		v = map[string]any{"pattern": in.Pattern, "path": in.Path}
	case tool.Fetch:
		v = map[string]any{"url": in.URL, "prompt": in.Prompt}
	case tool.WebSearch:
		v = map[string]any{"query": in.Query}
	case tool.Subagent:
		v = map[string]any{"description": in.Description, "prompt": in.Prompt, "subagent_type": in.Agent}
	case tool.Todo:
		v = map[string]any{"todos": claudeTodos(in.Todos)}
	case tool.Question:
		v = map[string]any{"questions": []map[string]any{{"question": c.Title}}}
	default:
		if len(c.Raw) > 0 && c.Raw[0] == '{' {
			return name, c.Raw
		}
		v = map[string]any{}
		for k, x := range map[string]string{"description": in.Description, "command": in.Command, "file_path": in.Path,
			"pattern": in.Pattern, "url": in.URL, "query": in.Query, "prompt": in.Prompt} {
			if x != "" {
				v[k] = x
			}
		}
		if len(v) == 0 && c.Title != "" {
			v["description"] = c.Title
		}
	}
	b, _ := jsonx.Marshal(v)
	return name, b
}

// claudeResult is an output as Claude Code's structured account of a run.
func claudeResult(o *tool.Output) jsontext.Value {
	v := map[string]any{}
	if o.Stdout != "" || o.Stderr != "" {
		v["stdout"], v["stderr"] = o.Stdout, o.Stderr
	}
	if len(o.Patches) > 0 {
		v["structuredPatch"] = o.Patches
	}
	if o.Created {
		v["type"] = "create"
	}
	if l := o.Lines; l != nil {
		v["file"] = map[string]int{"numLines": l.Count, "startLine": l.Start, "totalLines": l.Total}
	}
	if len(v) == 0 {
		return nil
	}
	b, _ := jsonx.Marshal(v)
	return b
}

// resultText is what a call returned, saying how a command exited when it
// failed, as Claude Code's text does.
func resultText(o *tool.Output) string {
	text := o.Text
	if text == "" && (o.Stdout != "" || o.Stderr != "") {
		text = strings.TrimRight(o.Stdout+"\n"+o.Stderr, "\n")
	}
	if o.IsError && o.Exit != nil && *o.Exit > 0 && !exitRe.MatchString(text) {
		text = fmt.Sprintf("Exit code %d\n%s", *o.Exit, text)
	}
	return text
}

// claudeTodos is a plan as TodoWrite's todos.
func claudeTodos(todos []tool.TodoItem) []map[string]string {
	out := make([]map[string]string, 0, len(todos))
	for _, t := range todos {
		out = append(out, map[string]string{"content": t.Label, "activeForm": t.Active, "status": t.Status})
	}
	return out
}

// taskType is Claude Code's word for a task of kind k.
func taskType(k event.TaskKind) string {
	switch k {
	case event.ShellTask:
		return "local_bash"
	case event.SubagentTask:
		return "local_agent"
	case event.MonitorTask:
		return "monitor_mcp"
	case event.WorkflowTask:
		return "local_workflow"
	}
	return "task"
}
