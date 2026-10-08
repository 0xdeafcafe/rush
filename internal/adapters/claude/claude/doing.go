package claude

import (
	"encoding/json/jsontext"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// Doing says in words what a tool call is doing, for the one line a list row
// has: "running pnpm test", "reading view.go", "searching for PrettyModel".
// A command's own description wins when Claude gave one.
func Doing(name string, input jsontext.Value) string {
	switch name {
	case "BashOutput", "TaskOutput":
		return "checking a background task"
	case "KillShell", "TaskStop":
		return "stopping a background task"
	case "TaskCreate", "TaskUpdate":
		return "updating its todo list"
	case "Skill":
		var in struct{ Skill string }
		_ = jsonx.Unmarshal(input, &in)
		if in.Skill == "" {
			return "using"
		}
		return "using " + in.Skill
	case "ToolSearch":
		return "looking up tools"
	case "ScheduleWakeup", "Monitor":
		return "waiting"
	case "SendMessage":
		var in struct{ Summary string }
		_ = jsonx.Unmarshal(input, &in)
		if in.Summary == "" {
			return "messaging"
		}
		return "messaging " + in.Summary
	case "SubagentHandback":
		return "reporting back"
	case "ListAgents":
		return "listing agents"
	case "PushNotification":
		return "notifying you"
	case "EnterWorktree":
		return "entering a worktree"
	}
	return tool.Doing(Call("", name, input))
}
