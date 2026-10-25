// Package prwatch is the bundled plugin that has agents wait on a pull
// request's checks through rush rather than poll GitHub: rush watch pr
// (see internal/watch), the prompt that tells agents of it, and the Bash
// hook's turning a poll into it. It's off until you turn it on.
package prwatch

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/watch"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "watch"

// Manifest is the watch plugin's.
var Manifest = plugin.Manifest{
	Name:     Name,
	Category: "pull-requests",
	Description: "Waits on pull requests' checks for your agents, so they never poll GitHub: rush watch pr asks for " +
		"them, cheaply and shared, and says once what happened. Agents are told to use it, and gh pr checks --watch " +
		"or a sleep before asking GitHub is turned into it.",
	Command:  []string{"rush"},
	Prompt:   watch.Prompt,
	MemoryMB: 16,
}

// On is whether you have it on.
func On() bool { return plugin.BundledOn(Name) }

// Run is the plugin on its connection to the broker: rush watch does the
// work, so it has nothing to do but be on.
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
