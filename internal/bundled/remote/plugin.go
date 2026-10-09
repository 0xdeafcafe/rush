// Package remote is the bundled plugins that are rush serve's switches
// (docs/remote.md): how it's reached (remote-tailscale, remote-cloudflared)
// and which clients it serves (remote-client-web, remote-client-tui). Each
// has nothing to run: rush serve looks at whether it's on, every few
// seconds, and does the work itself.
package remote

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() {
	for _, m := range []struct{ name, about string }{
		{"remote-tailscale", "Lets rush serve (your sessions in a browser, from your phone) be reached over your tailnet, " +
			"with the installed tailscale, on https port 8443. Off removes only that mapping."},
		{"remote-cloudflared", "Lets rush serve be reached through your Cloudflare tunnel, with the installed cloudflared " +
			"and the tunnel token in cloudflared-token in rush's folder. Off stops only that tunnel."},
		{"remote-client-web", "rush serve serves the web app: your sessions in a browser or on your phone's Home Screen. " +
			"Off, it answers only rush on your other machines."},
		{"remote-client-tui", "rush serve shows the sessions of the machines in remote.json in this machine's rush, " +
			"as rush remote attach does, without a terminal of its own."},
	} {
		plugin.RegisterBundle(plugin.Bundle{Optional: true, Run: run,
			Manifest: plugin.Manifest{Name: m.name, Description: m.about, Command: []string{"rush"}, MemoryMB: 16}})
	}
}

// run is a switch on its connection to the broker: it has nothing to do
// but be on.
func run(rw io.ReadWriteCloser) error {
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
