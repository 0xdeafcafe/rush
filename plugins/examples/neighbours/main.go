// Command neighbours is the smallest useful rush plugin: one tool that
// tells Claude which other agents are working in the same repository, and
// on which branch, so it doesn't trip over them. It needs only the list
// capability, and shows the whole shape of a plugin: answer initialize,
// offer tools, answer tool calls, call rush back.
//
//	mkdir -p ~/.config/rush/plugins/neighbours
//	go build -o ~/.config/rush/plugins/neighbours/neighbours ./plugins/examples/neighbours
//	cp plugins/examples/neighbours/plugin.json ~/.config/rush/plugins/neighbours/
//	rush plugin approve neighbours
package main

import (
	"context"
	"encoding/json/jsontext"
	"fmt"
	"os"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

var conn *plugin.Conn

func main() {
	f := os.NewFile(3, "rush")
	if f == nil {
		fmt.Fprintln(os.Stderr, "run me from rush: I talk on fd 3")
		os.Exit(2)
	}
	conn = plugin.NewConn(f, handle)
	<-conn.Done()
}

func handle(ctx context.Context, method string, params jsontext.Value) (any, error) {
	switch method {
	case "initialize":
		return map[string]any{}, nil
	case "tools.list":
		return map[string]any{"tools": []map[string]any{{
			"name":        "neighbours",
			"description": "List the other agents working in this repository right now: their branch, state and what they're doing. Check before touching shared files, switching branches or running migrations.",
			"inputSchema": map[string]any{"type": "object", "properties": map[string]any{}, "additionalProperties": false},
		}}}, nil
	case "tools.call":
		// Who's calling comes with the call: rush's id for the session,
		// and its folder.
		var in struct {
			Session string `json:"session"`
			Cwd     string `json:"cwd"`
		}
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, err
		}
		text, err := neighbours(ctx, in.Session, in.Cwd)
		if err != nil {
			return result(err.Error(), true), nil
		}
		return result(text, false), nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

type session struct {
	ID, Name, Cwd, Repo, Branch, State, Detail string
	Worktree                                   bool
}

func neighbours(ctx context.Context, me, cwd string) (string, error) {
	var all []session
	if err := conn.Call(ctx, "sessions.list", nil, &all); err != nil {
		return "", err
	}
	// The main checkout and its worktrees are one repository: compare where
	// they keep their worktrees from, the part before .claude/worktrees.
	repo := ""
	for _, s := range all {
		if s.ID == me {
			repo = home(s.Repo)
		}
	}
	if repo == "" {
		repo = home(cwd)
	}
	var b strings.Builder
	for _, s := range all {
		if s.ID == me || s.State == "stopped" || repo == "" || home(s.Repo) != repo {
			continue
		}
		fmt.Fprintf(&b, "- %s (%s) on %s, %s", s.Name, s.ID, or(s.Branch, "?"), s.State)
		if s.Worktree {
			b.WriteString(" in its own worktree")
		}
		if s.Detail != "" {
			fmt.Fprintf(&b, ": %s", s.Detail)
		}
		b.WriteString("\n")
	}
	if b.Len() == 0 {
		return "No other agents are working in this repository.", nil
	}
	return b.String(), nil
}

// home is the main checkout a checkout belongs to, going by where rush and
// Claude Code put worktrees.
func home(repo string) string {
	if i := strings.Index(repo, "/.claude/worktrees/"); i >= 0 {
		return repo[:i]
	}
	return repo
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

func result(text string, isErr bool) map[string]any {
	return map[string]any{"content": []map[string]any{{"type": "text", "text": text}}, "isError": isErr}
}
