// Package hexview is the bundled plugin that lets a step's output be seen
// as its bytes: v on a step cycles to a hex viewer. The viewer is rush's
// own (internal/convo); the plugin is the switch for it.
package hexview

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Manifest is the hex plugin's.
var Manifest = plugin.Manifest{
	Name:     "hex",
	Category: "views",
	Description: "Adds a hex view to what agents' steps print and read: v on a step cycles to its bytes, " +
		"coloured by kind, with an ASCII column. Binary output opens in it.",
	Command:  []string{"rush"},
	MemoryMB: 16,
}

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
