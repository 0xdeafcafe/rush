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
	if got := Allowed(); len(got) != 5 || got[0] != Show || got[1] != Prefix+"view" {
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

// fakeViewer keeps what the view tool set.
type fakeViewer struct {
	Agents
	set   map[string]string
	clear []string
}

func (f *fakeViewer) SetView(name string, v jsontext.Value) (string, error) {
	f.set[name] = string(v)
	return "Shown", nil
}

func (f *fakeViewer) Catalogue() string { return "" }

func (f *fakeViewer) ClearView(name string) error {
	f.clear = append(f.clear, name)
	return nil
}

// The view tool is listed only where views are kept, sets one and puts
// one away.
func TestView(t *testing.T) {
	f := &fakeViewer{set: map[string]string{}}
	h := Handler(f)
	call := func(msg string) map[string]any {
		var out map[string]any
		if err := jsonx.Unmarshal(h(jsontext.Value(msg)), &out); err != nil {
			t.Fatal(err)
		}
		return out["result"].(map[string]any)
	}
	if !strings.Contains(string(h(jsontext.Value(`{"jsonrpc":"2.0","id":1,"method":"tools/list"}`))), `"name":"view"`) {
		t.Fatal("view isn't listed")
	}
	if strings.Contains(string(Handle(jsontext.Value(`{"jsonrpc":"2.0","id":1,"method":"tools/list"}`))), `"name":"view"`) {
		t.Fatal("view is listed where no views are kept")
	}
	if r := call(`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"view","arguments":{"name":"prs","view":{"title":"PRs"}}}}`); r["isError"] == true || f.set["prs"] == "" {
		t.Fatalf("set: %v %v", r, f.set)
	}
	if r := call(`{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"view","arguments":{"name":"prs","clear":true}}}`); r["isError"] == true || len(f.clear) != 1 {
		t.Fatalf("clear: %v %v", r, f.clear)
	}
	if r := call(`{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"view","arguments":{"name":"prs"}}}`); r["isError"] != true {
		t.Fatalf("no view wasn't refused: %v", r)
	}
}
