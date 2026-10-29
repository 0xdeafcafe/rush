// An MCP server on stdio, as small as one can be, for the broker's tests:
// it echoes what its one tool is given.
package main

import (
	"bufio"
	"encoding/json"
	"os"
)

func main() {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	w := json.NewEncoder(os.Stdout)
	for sc.Scan() {
		var m struct {
			ID     json.RawMessage `json:"id"`
			Method string          `json:"method"`
			Params struct {
				Arguments struct {
					Text string `json:"text"`
				} `json:"arguments"`
			} `json:"params"`
		}
		if json.Unmarshal(sc.Bytes(), &m) != nil || m.ID == nil {
			continue
		}
		var res any
		switch m.Method {
		case "initialize":
			res = map[string]any{"protocolVersion": "2025-06-18", "capabilities": map[string]any{"tools": map[string]any{}},
				"serverInfo": map[string]any{"name": "echo", "version": "1"}, "instructions": "Echo things."}
		case "tools/list":
			res = map[string]any{"tools": []any{map[string]any{"name": "echo", "inputSchema": map[string]any{"type": "object"}}}}
		case "tools/call":
			res = map[string]any{"content": []any{map[string]any{"type": "text", "text": "echo: " + m.Params.Arguments.Text}}}
		default:
			_ = w.Encode(map[string]any{"jsonrpc": "2.0", "id": m.ID, "error": map[string]any{"code": -32601, "message": "no " + m.Method}})
			continue
		}
		_ = w.Encode(map[string]any{"jsonrpc": "2.0", "id": m.ID, "result": res})
	}
}
