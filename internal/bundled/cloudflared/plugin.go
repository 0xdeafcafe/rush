// Package cloudflared is the bundled plugin that is the switch for reaching
// rush serve through your Cloudflare tunnel: while it's on, rush serve runs
// the installed cloudflared with your tunnel's token file, and stops just
// that process when it's off.
package cloudflared

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Manifest is the plugin's.
var Manifest = plugin.Manifest{
	Name: "cloudflared",
	Description: "Lets rush serve (your sessions in a browser, from your phone) be reached through your Cloudflare tunnel, " +
		"with the installed cloudflared and the tunnel token in cloudflared-token in rush's folder. Off stops only that tunnel.",
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
