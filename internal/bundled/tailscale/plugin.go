// Package tailscale is the bundled plugin that is the switch for putting
// rush serve on your tailnet: while it's on, rush serve runs the installed
// tailscale's `serve` for its own port (remote.Expose), and takes just
// that mapping down when it's off.
package tailscale

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Manifest is the plugin's.
var Manifest = plugin.Manifest{
	Name: "tailscale",
	Description: "Lets rush serve (your sessions in a browser, from your phone) be reached over your tailnet, " +
		"with the installed tailscale, on https port 8443. Off removes only that mapping.",
	Command:  []string{"rush"},
	MemoryMB: 16,
}

// Run is the plugin on its connection to the broker: it has nothing to do
// but be on. rush serve does the rest.
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
