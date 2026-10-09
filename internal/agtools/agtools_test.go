package agtools

import (
	"encoding/json/jsontext"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

func rpc(t *testing.T, msg string) map[string]any {
	t.Helper()
	var out map[string]any
	if err := jsonx.Unmarshal(Handle(jsontext.Value(msg)), &out); err != nil {
		t.Fatal(err)
	}
	return out
}

func TestHandshake(t *testing.T) {
	init := rpc(t, `{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}`)
	res := init["result"].(map[string]any)
	if res["protocolVersion"] != "2025-11-25" || res["capabilities"].(map[string]any)["tools"] == nil {
		t.Fatalf("initialize: %v", init)
	}
	if n := rpc(t, `{"jsonrpc":"2.0","method":"notifications/initialized"}`); n["result"] == nil || n["id"] != nil {
		t.Fatalf("a notification gets an empty result: %v", n)
	}
	list := rpc(t, `{"jsonrpc":"2.0","id":1,"method":"tools/list"}`)
	ts := list["result"].(map[string]any)["tools"].([]any)
	show := ts[0].(map[string]any)
	if show["name"] != "show" || show["_meta"].(map[string]any)["anthropic/alwaysLoad"] != true {
		t.Fatalf("tools/list: %v", show)
	}
	if got := Allowed(); len(got) != 4 || got[0] != Show {
		t.Fatalf("allowed: %v", got)
	}
}

func TestShow(t *testing.T) {
	ok := rpc(t, `{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"show","arguments":{"drawing":"+-+\n|a|\n+-+"}}}`)
	if r := ok["result"].(map[string]any); r["isError"] != false || !strings.Contains(r["content"].([]any)[0].(map[string]any)["text"].(string), "Shown") {
		t.Fatalf("show: %v", ok)
	}
	empty := rpc(t, `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"show","arguments":{"drawing":"  "}}}`)
	if empty["result"].(map[string]any)["isError"] != true {
		t.Fatalf("an empty drawing is an error: %v", empty)
	}
	if u := rpc(t, `{"jsonrpc":"2.0","id":4,"method":"resources/list"}`); u["error"] == nil {
		t.Fatalf("unknown method: %v", u)
	}
}

func TestServe(t *testing.T) {
	in := `{"jsonrpc":"2.0","id":1,"method":"tools/list"}` + "\n" + `{"jsonrpc":"2.0","method":"notifications/initialized"}` + "\n"
	var out strings.Builder
	if err := Serve(strings.NewReader(in), &out, Handle); err != nil {
		t.Fatal(err)
	}
	if lines := strings.Split(strings.TrimSpace(out.String()), "\n"); len(lines) != 1 || !strings.Contains(lines[0], `"show"`) {
		t.Fatalf("one answer, to the request alone: %q", out.String())
	}
}

type spawnRec struct{ Agents }

var spawned SpawnInput

func (spawnRec) Spawn(in SpawnInput) (string, error) { spawned = in; return "ok", nil }

// The call Claude names in _meta reaches Spawn, so its child is drawn there.
func TestSpawnKnowsItsCall(t *testing.T) {
	Handler(spawnRec{})(jsontext.Value(`{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"spawn_agent","arguments":{"agent":"a","prompt":"p"},"_meta":{"claudecode/toolUseId":"toolu_9"}}}`))
	if spawned.Step != "toolu_9" || spawned.Prompt != "p" {
		t.Fatalf("spawned %+v", spawned)
	}
}
