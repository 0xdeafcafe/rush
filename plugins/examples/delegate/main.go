// Command delegate is an example rush plugin. It gives Claude tools to hand
// work to agents of their own, in rush, and to follow them: start one, list
// them, message one, and read what one said last.
//
// It speaks rush's plugin protocol: JSON-RPC 2.0 in length-prefixed frames
// on fd 3. Build it into its folder and approve it:
//
//	mkdir -p ~/.config/rush/plugins/delegate
//	go build -o ~/.config/rush/plugins/delegate/delegate ./plugins/examples/delegate
//	cp plugins/examples/delegate/plugin.json ~/.config/rush/plugins/delegate/
//	rush plugin approve delegate
package main

import (
	"context"
	"encoding/json/jsontext"
	"fmt"
	"os"
	"strings"
	"sync"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// last is what each agent this plugin started said last, as it happens.
var (
	mu    sync.Mutex
	last  = map[string]string{}
	state = map[string]string{}
	conn  *plugin.Conn
)

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
		return map[string]any{"tools": tools}, nil
	case "tools.call":
		var in struct {
			Session   string         `json:"session"`
			Name      string         `json:"name"`
			Arguments jsontext.Value `json:"arguments"`
		}
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, err
		}
		text, err := call(ctx, in.Name, in.Arguments)
		if err != nil {
			return result(err.Error(), true), nil
		}
		return result(text, false), nil
	case "session.event":
		var ev struct {
			Session string `json:"session"`
			Type    string `json:"type"`
			Text    string `json:"text"`
			State   string `json:"state"`
		}
		_ = jsonx.Unmarshal(params, &ev)
		mu.Lock()
		switch ev.Type {
		case "text", "result":
			if ev.Text != "" {
				last[ev.Session] = ev.Text
			}
		case "info":
			state[ev.Session] = ev.State
		case "closed":
			state[ev.Session] = "stopped"
		}
		mu.Unlock()
		return nil, nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

func call(ctx context.Context, name string, args jsontext.Value) (string, error) {
	var in struct {
		Prompt string `json:"prompt"`
		Cwd    string `json:"cwd"`
		ID     string `json:"id"`
		Text   string `json:"text"`
	}
	_ = jsonx.Unmarshal(args, &in)
	switch name {
	case "start_agent":
		var out struct {
			ID string `json:"id"`
		}
		if err := conn.Call(ctx, "sessions.start", map[string]any{"cwd": in.Cwd, "prompt": in.Prompt}, &out); err != nil {
			return "", err
		}
		if err := conn.Call(ctx, "sessions.subscribe", map[string]any{"id": out.ID}, nil); err != nil {
			return "", err
		}
		return "Started agent " + out.ID + ". Check on it with agent_result.", nil
	case "list_agents":
		var ss []struct {
			ID, Name, Cwd, State, Detail, StartedBy string
		}
		if err := conn.Call(ctx, "sessions.list", nil, &ss); err != nil {
			return "", err
		}
		var b strings.Builder
		for _, s := range ss {
			fmt.Fprintf(&b, "%s  %-8s %s  (%s)", s.ID, s.State, s.Name, s.Cwd)
			if s.Detail != "" {
				fmt.Fprintf(&b, ": %s", s.Detail)
			}
			if s.StartedBy != "" {
				fmt.Fprintf(&b, " [started by %s]", s.StartedBy)
			}
			b.WriteString("\n")
		}
		if b.Len() == 0 {
			return "No agents.", nil
		}
		return b.String(), nil
	case "message_agent":
		if err := conn.Call(ctx, "sessions.send", map[string]any{"id": in.ID, "text": in.Text}, nil); err != nil {
			return "", err
		}
		// Following it again is harmless, and picks it back up after a restart.
		_ = conn.Call(ctx, "sessions.subscribe", map[string]any{"id": in.ID}, nil)
		return "Sent.", nil
	case "agent_result":
		mu.Lock()
		defer mu.Unlock()
		st, said := state[in.ID], last[in.ID]
		if st == "" && said == "" {
			return "", fmt.Errorf("no news from %s yet (only agents started with start_agent are followed)", in.ID)
		}
		return fmt.Sprintf("State: %s\nLast said:\n%s", st, said), nil
	}
	return "", fmt.Errorf("no tool named %s", name)
}

func result(text string, isErr bool) map[string]any {
	return map[string]any{"content": []map[string]any{{"type": "text", "text": text}}, "isError": isErr}
}

func schema(props map[string]string, required ...string) map[string]any {
	p := map[string]any{}
	for k, d := range props {
		p[k] = map[string]any{"type": "string", "description": d}
	}
	return map[string]any{"type": "object", "properties": p, "required": required, "additionalProperties": false}
}

var tools = []map[string]any{
	{"name": "start_agent", "description": "Start another Claude agent, in rush, on a task of its own. It runs alongside you; check on it with agent_result.",
		"inputSchema": schema(map[string]string{"prompt": "The task, as you'd give it to a colleague.", "cwd": "The absolute path of the folder it works in."}, "prompt", "cwd")},
	{"name": "list_agents", "description": "List every rush agent: its id, state, name, folder and what it's doing.",
		"inputSchema": schema(map[string]string{})},
	{"name": "message_agent", "description": "Send a message to an agent you started.",
		"inputSchema": schema(map[string]string{"id": "The agent's id.", "text": "The message."}, "id", "text")},
	{"name": "agent_result", "description": "The state of an agent you started, and the last thing it said.",
		"inputSchema": schema(map[string]string{"id": "The agent's id."}, "id")},
}
