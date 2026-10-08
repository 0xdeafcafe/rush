// Package queue is a plugin bundled with rush: `rush queue send|remove`
// sends now, or drops, a message waiting in a session's queue, from a
// script or a remote client, without the view.
package queue

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"flag"
	"fmt"
	"io"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Manifest: Manifest, Run: Run}) }

const usage = "<session> <n> [--was TEXT]"

// Manifest is the queue plugin's.
var Manifest = plugin.Manifest{
	Name: "queue",
	Description: "rush queue send and rush queue remove: send now, or drop, a message waiting in a rush-mode session's queue, " +
		"from a script or a remote client. It never sees what's queued.",
	Command:    []string{"rush"},
	Sessions:   []string{plugin.CapQueued},
	Workspaces: []string{"~"},
	CLI: []plugin.CLISpec{
		{Name: "send", Usage: usage, Description: "send the message queued at place n (from 0, as info lists them) now"},
		{Name: "remove", Usage: usage, Description: "drop the message queued at place n; --was names it by its text, if the queue moved"},
	},
	MemoryMB: 32,
}

// Run is the plugin, on its connection to the broker.
func Run(rw io.ReadWriteCloser) error {
	ready := make(chan struct{})
	var conn *plugin.Conn
	conn = plugin.NewConn(rw, func(ctx context.Context, method string, params jsontext.Value) (any, error) {
		<-ready
		switch method {
		case "initialize":
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": []any{}}, nil
		case "cli.run":
			var r plugin.CLIRun
			if err := jsonx.Unmarshal(params, &r); err != nil {
				return nil, err
			}
			return run(ctx, conn, r), nil
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	close(ready)
	<-conn.Done()
	return nil
}

// run is one CLI command.
func run(ctx context.Context, conn *plugin.Conn, r plugin.CLIRun) plugin.CLIResult {
	id, n, was, err := parse(r.Args)
	if err != nil {
		return plugin.CLIResult{Stderr: fmt.Sprintf("%v\nusage: rush queue %s %s\n", err, r.Command, usage), Exit: 2}
	}
	if err := conn.Call(ctx, "sessions.queued."+r.Command, map[string]any{"id": id, "index": n, "was": was}, nil); err != nil {
		return plugin.CLIResult{Stderr: err.Error() + "\n", Exit: 1}
	}
	did := map[string]string{"send": "sent queued message %d to %s\n", "remove": "removed queued message %d from %s\n"}[r.Command]
	return plugin.CLIResult{Stdout: fmt.Sprintf(did, n, id)}
}

// parse reads <session> <n> [--was TEXT], the flag before or after.
func parse(args []string) (id string, n int, was string, err error) {
	fs := flag.NewFlagSet("queue", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	fs.StringVar(&was, "was", "", "")
	var pos []string
	for len(args) > 0 {
		if err := fs.Parse(args); err != nil {
			return "", 0, "", err
		}
		args = fs.Args()
		if len(args) > 0 {
			pos, args = append(pos, args[0]), args[1:]
		}
	}
	if len(pos) != 2 {
		return "", 0, "", errors.New("a session and a place in its queue, please")
	}
	n, err = strconv.Atoi(pos[1])
	if err != nil || n < 0 {
		return "", 0, "", fmt.Errorf("queue place %q is not a number from 0", pos[1])
	}
	return strings.TrimSpace(pos[0]), n, was, nil
}
