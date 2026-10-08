package drafts

import (
	"context"
	"encoding/json/jsontext"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// The plugin as the broker drives it: events in, commands, and the calls it
// makes back.
func TestPluginOverItsConnection(t *testing.T) {
	data := t.TempDir()
	mine, theirs := net.Pipe()
	calls := make(chan string, 20)
	broker := plugin.NewConn(mine, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		calls <- method + " " + string(params)
		return map[string]any{}, nil
	})
	done := make(chan struct{})
	go func() { _ = Run(theirs); close(done) }()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := broker.Call(ctx, "initialize", map[string]any{"dataDir": data}, nil); err != nil {
		t.Fatal(err)
	}
	s1 := &plugin.UISession{ID: "s1", Name: "one"}
	_ = broker.Notify("ui.event", plugin.UIEvent{Kind: plugin.EvInputChanged, UI: "w", Session: s1, Text: "hello", Box: &plugin.Box{Text: "hello", Cursor: 2}})
	if err := broker.Call(ctx, "ui.command", map[string]any{"command": "stash", "ui": "w", "box": "s1", "session": s1,
		"input": plugin.Box{Text: "hello", Cursor: 2}}, nil); err != nil {
		t.Fatal(err)
	}
	want := func(prefix string) map[string]any {
		t.Helper()
		for {
			select {
			case c := <-calls:
				if len(c) > len(prefix) && c[:len(prefix)] == prefix {
					var v map[string]any
					_ = jsonx.Unmarshal([]byte(c[len(prefix):]), &v)
					return v
				}
			case <-time.After(3 * time.Second):
				t.Fatalf("no %s", prefix)
			}
		}
	}
	if set := want("ui.input.set "); set["if"] != "hello" || set["session"] != "s1" {
		t.Fatalf("stash set: %v", set)
	}
	if note := want("ui.box.note "); note["text"] == "" {
		t.Fatalf("note: %v", note)
	}
	_ = broker.Notify("ui.event", plugin.UIEvent{Kind: plugin.EvInputSent, UI: "w", Session: s1, Text: "other"})
	if set := want("ui.input.set "); set["if"] != "" || set["box"].(map[string]any)["text"] != "hello" {
		t.Fatalf("after send, the stash should come back: %v", set)
	}
	if err := broker.Call(ctx, "ui.command", map[string]any{"command": "history", "ui": "w", "box": "s1"}, nil); err != nil {
		t.Fatal(err)
	}
	if p := want("ui.pick "); p["pick"].(map[string]any)["id"] != "history" {
		t.Fatalf("pick: %v", p)
	}
	broker.Close()
	<-done
	if b, err := os.ReadFile(filepath.Join(data, "drafts.json")); err != nil || len(b) == 0 {
		t.Fatalf("not kept on disk: %v", err)
	}
}
