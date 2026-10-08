// Package agtools is rush's own MCP server, run inside a rush session's
// host rather than as a process: Claude Code names it in initialize and sends
// every MCP message for it over the session's control channel. Its tools are
// things only rush can do: draw a drawing in its own frame, and hand work to
// another agent (agents.go). An agent that only runs MCP servers as
// processes runs `rush mcp-tools`: Serve.
package agtools

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"io"
	"slices"
	"strings"
	"sync"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Server is the MCP server's name; Claude Code calls its tools mcp__rush__*.
const Server = "rush"

// Prefix starts the name Claude Code gives each of these tools.
const Prefix = "mcp__" + Server + "__"

// Show is the show tool as Claude Code names it.
const Show = Prefix + "show"

// ShowInput is what Claude sends to show.
type ShowInput struct {
	Title   string `json:"title"`
	Drawing string `json:"drawing"`
}

type tool struct {
	Name        string         `json:"name"`
	Description string         `json:"description"`
	InputSchema map[string]any `json:"inputSchema"`
	Meta        map[string]any `json:"_meta,omitempty"`
}

var tools = []tool{{
	Name: "show",
	Description: "Show the user a drawing: an ASCII or box-drawing diagram, a flow or architecture sketch, a layout mockup, " +
		"a tree, or any text whose exact layout matters. rush draws it in a frame of its own, full width, never wrapped " +
		"or folded away, and the user can copy it whole. Use it instead of a code block whenever the picture is the point. " +
		"The drawing is shown as-is: plain text, no markdown, no fences.",
	InputSchema: map[string]any{
		"type": "object",
		"properties": map[string]any{
			"title":   map[string]any{"type": "string", "description": "A few words naming what the drawing shows."},
			"drawing": map[string]any{"type": "string", "description": "The drawing, exactly as it should appear, lines separated by \\n."},
		},
		"required":             []string{"drawing"},
		"additionalProperties": false,
	},
	// Loaded with the built-in tools, not deferred behind tool search, so
	// Claude reaches for it without first looking it up.
	Meta: map[string]any{"anthropic/alwaysLoad": true},
}}

// Names are rush's tools by their own names, which all never ask: show
// only draws, and the agent tools start what the Agent tool would.
func Names() []string {
	out := make([]string, 0, len(tools)+len(agentTools))
	for _, t := range tools {
		out = append(out, t.Name)
	}
	for _, t := range agentTools {
		out = append(out, t.Name)
	}
	return out
}

// Allowed is what to pass --allowedTools so the tools run without asking.
func Allowed() []string {
	out := Names()
	for i, n := range out {
		out[i] = Prefix + n
	}
	return out
}

// Handle answers one JSON-RPC message with show alone: Handler(nil).
func Handle(msg jsontext.Value) jsontext.Value { return Handler(nil)(msg) }

// Handler answers one JSON-RPC message from an agent's MCP client, with
// the agent tools too when ag runs them. A notification gets an empty
// result, which is what Claude Code expects back.
func Handler(ag Agents) func(msg jsontext.Value) jsontext.Value {
	return func(msg jsontext.Value) jsontext.Value { return handle(ag, msg) }
}

func handle(ag Agents, msg jsontext.Value) jsontext.Value {
	var m struct {
		ID     jsontext.Value `json:"id"`
		Method string         `json:"method"`
		Params jsontext.Value `json:"params"`
	}
	if err := jsonx.Unmarshal(msg, &m); err != nil {
		return reply(nil, nil, &rpcError{Code: -32700, Message: "parse error"})
	}
	if len(m.ID) == 0 {
		return reply(nil, map[string]any{}, nil)
	}
	switch m.Method {
	case "initialize":
		var p struct {
			ProtocolVersion string `json:"protocolVersion"`
		}
		_ = jsonx.Unmarshal(m.Params, &p)
		return reply(m.ID, map[string]any{
			"protocolVersion": p.ProtocolVersion,
			"capabilities":    map[string]any{"tools": map[string]any{}},
			"serverInfo":      map[string]any{"name": Server, "version": "1"},
		}, nil)
	case "tools/list":
		list := tools
		if ag != nil {
			list = append(slices.Clone(tools), agentList(ag)...)
		}
		return reply(m.ID, map[string]any{"tools": list}, nil)
	case "tools/call":
		var p struct {
			Name      string         `json:"name"`
			Arguments jsontext.Value `json:"arguments"`
		}
		_ = jsonx.Unmarshal(m.Params, &p)
		if res, ok := callAgent(ag, p.Name, p.Arguments); ok {
			return reply(m.ID, res, nil)
		}
		return reply(m.ID, call(p.Name, p.Arguments), nil)
	case "ping":
		return reply(m.ID, map[string]any{}, nil)
	}
	return reply(m.ID, nil, &rpcError{Code: -32601, Message: "method not found: " + m.Method})
}

// call runs a tool. The drawing itself is the call's input, which the
// transcript keeps, so showing it needs nothing more than a yes.
func call(name string, args jsontext.Value) map[string]any {
	switch name {
	case "show":
		var in ShowInput
		_ = jsonx.Unmarshal(args, &in)
		if strings.TrimSpace(in.Drawing) == "" {
			return result("Nothing to show: the drawing is empty.", true)
		}
		return result("Shown to the user.", false)
	}
	return result("rush has no tool named "+name+".", true)
}

func result(text string, isErr bool) map[string]any {
	return map[string]any{"content": []map[string]any{{"type": "text", "text": text}}, "isError": isErr}
}

type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func reply(id jsontext.Value, res any, e *rpcError) jsontext.Value {
	out := map[string]any{"jsonrpc": "2.0"}
	if id != nil {
		out["id"] = id
	}
	if e != nil {
		out["error"] = e
	} else {
		out["result"] = res
	}
	b, _ := jsonx.Marshal(out)
	return b
}

// Serve is the server over stdio, a message a line, for an agent that runs
// its MCP servers as processes (Codex, the ACP agents). A notification gets
// nothing back; each request is answered as it's done, as an agent's call
// may wait on another agent for a while.
func Serve(r io.Reader, w io.Writer, handle func(jsontext.Value) jsontext.Value) error {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64<<10), 16<<20)
	var mu sync.Mutex
	var wg sync.WaitGroup
	var failed error
	for sc.Scan() {
		var m struct {
			ID jsontext.Value `json:"id"`
		}
		if jsonx.Unmarshal(sc.Bytes(), &m) == nil && len(m.ID) == 0 {
			continue
		}
		msg := jsontext.Value(bytes.Clone(sc.Bytes()))
		wg.Go(func() {
			out := append(handle(msg), '\n')
			mu.Lock()
			defer mu.Unlock()
			if _, err := w.Write(out); err != nil && failed == nil {
				failed = err
			}
		})
	}
	wg.Wait() // what was asked before the agent hung up is still answered
	mu.Lock()
	defer mu.Unlock()
	if failed != nil {
		return failed
	}
	return sc.Err()
}
