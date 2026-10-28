// Package cleanup is the bundled plugin that lets rush offer a cleanup
// agent when the disk runs low or the memory stays under pressure
// (ui/machinecare.go does the watching and the offering). It's off until
// you turn it on; rush recommends it the first time the disk is low.
//
// What the agent is told comes from rush's own check of worktrees and
// /tmp, and from every plugin that provides the cleanup.report service:
// Haven, go-clean, rust-clean, js-clean and git-clean, and any plugin you
// install that says it does.
package cleanup

import (
	"context"
	"encoding/json/jsontext"
	"io"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "cleanup"

// Manifest is the cleanup plugin's.
var Manifest = plugin.Manifest{
	Name:     Name,
	Category: "machine",
	Description: "When the disk runs low or the memory stays under pressure, rush asks whether to start a cleanup agent. " +
		"It reads your worktrees, what agents left in /tmp and what other plugins report (Haven, go-clean, rust-clean, js-clean, git-clean), " +
		"removes what can go without losing work, and says what it did. Nothing starts until you say yes.",
	Command:  []string{"rush"},
	Uses:     []string{plugin.ServiceCleanupReport},
	MemoryMB: 16,
}

// On says whether you have it on. It reads the disk.
func On() bool { return plugin.BundledOn(Name) }

// Run is a switch on its connection to the broker: rush does the work.
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
