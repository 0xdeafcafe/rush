// Package allow is the bundled plugin that answers yes to every
// permission prompt a Claude Code session asks: the checks
// bypassPermissions still asks for (`cd a && rm -rf ../../b/*`) too.
// Questions and plans still come to you, and plan mode still asks. It is
// for machines where something else guards what agents do: a sandbox, a
// vault for credentials. The conn does the answering
// (agent.StartOptions.AllowAll); the plugin is the switch for it.
package allow

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "allow"

// Manifest is the allow plugin's.
var Manifest = plugin.Manifest{
	Name: Name,
	Description: "Answers yes to every permission prompt your Claude Code sessions ask, even the ones " +
		"bypass mode still asks. Questions and plans still come to you. For machines where a sandbox " +
		"or a vault guards what agents do. Sessions pick it up the next time their Claude Code starts.",
	Command:  []string{"rush"},
	Requires: plugin.Requires{OS: []string{"darwin", "linux"}},
	MemoryMB: 16,
}

// On is whether it's on. It reads the disk.
func On() bool { return plugin.BundledOn(Name) }

// Run is the plugin on its connection to the broker: it has nothing to do
// but be on.
func Run(rw io.ReadWriteCloser) error {
	conn := plugin.NewConn(rw, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		switch method {
		case "initialize":
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": []any{}}, nil
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	<-conn.Done()
	return nil
}
