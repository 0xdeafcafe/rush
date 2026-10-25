// Package haven is the bundled plugin for Haven, the tool that runs your
// projects' stacks and cleans up after them. It's off until you turn it
// on, runs only where haven is installed, and answers the cleanup.report
// service: what Haven would clean (haven machine clean, which only reports
// without --yes) and what its stacks hold in memory, with what a cleanup
// agent may do about it.
package haven

import (
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "haven"

// Manifest is Haven's.
var Manifest = plugin.Manifest{
	Name:     Name,
	Category: "machine",
	Description: "Tells rush's cleanup what Haven knows: what haven machine clean would remove and what its stacks " +
		"hold in memory, and how a cleanup agent may free it without losing data. Off until you turn it on; runs where haven is installed.",
	Command:  []string{"rush"},
	Requires: plugin.Requires{Bin: []string{"haven"}},
	Provides: []string{plugin.ServiceCleanupReport},
	MemoryMB: 32,
}

// wait is what one haven command may take.
const wait = 30 * time.Second

// rules is what a cleanup agent may do with Haven's report.
const rules = `- If Haven's report ticks things, haven machine clean --yes removes temporary and merged worktrees, cold job scratch and build caches, and never drops databases.
- For memory, a stack whose worktree nobody works in, that isn't the focus stack, may be stopped with haven down --stack <slug>, which keeps its data.
- Never haven down --destroy, db reset or db prune.`

// Run is the plugin's main.
func Run(rw io.ReadWriteCloser) error {
	conn := plugin.NewConn(rw, func(ctx context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "initialize":
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": []any{}}, nil
		case "service.call":
			var in struct {
				Service string `json:"service"`
			}
			_ = jsonx.Unmarshal(params, &in)
			if in.Service == plugin.ServiceCleanupReport {
				return Report(ctx), nil
			}
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	<-conn.Done()
	return nil
}

// Report is Haven's cleanup report: nothing is removed to make it.
func Report(ctx context.Context) plugin.CleanupReport {
	return plugin.CleanupReport{
		Title: "Haven",
		Report: "haven machine clean (its report; nothing was removed):\n" + run(ctx, "machine", "clean") +
			"\n\nhaven status --json stacks,footprint,focus:\n" + run(ctx, "status", "--json", "stacks,footprint,focus"),
		Rules: rules,
	}
}

func run(ctx context.Context, args ...string) string {
	ctx, cancel := context.WithTimeout(ctx, wait)
	defer cancel()
	cmd := exec.CommandContext(ctx, "haven", args...)
	cmd.Env = append(os.Environ(), "HAVEN_AGENT=1", "NO_COLOR=1")
	out, _ := cmd.CombinedOutput()
	s := strings.TrimSpace(string(out))
	if r := []rune(s); len(r) > 6000 {
		s = string(r[:6000]) + "\n…"
	}
	return s
}
