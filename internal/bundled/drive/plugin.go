// Package drive is the bundled plugin that has rush drive pull requests
// for agents: rush pr follow, reply and resolve (see internal/prdrive),
// and the prompt that tells agents of them. It's off until you turn it
// on; off, rush pr refuses and the followers already running end.
package drive

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/prdrive"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "drive"

// Manifest is the drive plugin's.
var Manifest = plugin.Manifest{
	Name: Name,
	Description: "Drives pull requests for your agents: rush pr follow says what's left before merge, in the order to " +
		"do it, then follows the PR and wakes the agent only with news it can act on (a failed check with its log, a " +
		"trusted review finding, a conflict, ready for approval). Agents are told to use it.",
	Command:  []string{"rush"},
	Prompt:   prdrive.Prompt,
	MemoryMB: 16,
}

// On is whether you have it on.
func On() bool { return plugin.BundledOn(Name) }

// Run is the plugin on its connection to the broker: rush pr does the
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
