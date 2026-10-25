// Package ts is the bundled plugin that gives every agent TypeScript code
// intelligence from one language server per checkout: tsrs (the Rust port
// of tsgo) where it's installed, else tsgo. Agents share it, the gate
// admits it, and it's stopped past its memory cap or once idle.
// docs/typescript.md has why.
package ts

import (
	"context"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "ts"

// Prompt is added to every rush-mode session's system prompt.
const Prompt = "For TypeScript and JavaScript symbols, use rush's ts tools (definition, references, hover, " +
	"implementation, callers, callees, outline, diagnostics, find_symbol, rename, rename_file; or `rush ts <tool> --file F --line N " +
	"--symbol S` from a shell) rather than grepping: they're type-aware and share one language server per " +
	"checkout. Give file and line: find_symbol searches every project, which in a monorepo is slow and heavy. " +
	"Rename and move files with rename and rename_file: they rewrite every use and import."

// Manifest is the plugin's.
var Manifest = plugin.Manifest{
	Name:     Name,
	Category: "code",
	Description: "TypeScript code intelligence for every agent: definitions, references, hover, callers, outlines and " +
		"type errors from one language server per checkout, shared by all agents in it. tsrs where it's installed " +
		"(npm i -g @maschwenk/tsrs), else the project's tsgo. Started through the gate, stopped past its memory cap " +
		"or after it's idle.",
	Command:  []string{"rush"},
	Tools:    true,
	Prompt:   Prompt,
	MemoryMB: 128,
	Settings: []plugin.SettingSpec{
		{Key: "engine", Title: "Engine", Type: "choice", Choices: []string{"auto", "tsrs", "tsgo"}, Default: "auto",
			Description: "auto runs tsrs where it's installed, else tsgo"},
		{Key: "cap", Title: "Memory cap", Type: "choice", Choices: []string{"2G", "3G", "4G", "6G", "8G", "off"}, Default: "4G",
			Description: "A server holding more is stopped, and the call told to narrow its query"},
		{Key: "idle", Title: "Idle", Type: "choice", Choices: []string{"5m", "10m", "30m", "1h"}, Default: "10m",
			Description: "A server asked nothing this long is closed"},
	},
	CLI:      cliSpecs(),
	Requires: plugin.Requires{OS: []string{"darwin", "linux"}},
}

func cliSpecs() []plugin.CLISpec {
	out := make([]plugin.CLISpec, 0, len(Ops)+1)
	for _, op := range Ops {
		usage := "--file F --line N (--symbol S | --character C)"
		switch op.Needs {
		case "file":
			usage = "--file F"
		case "query":
			usage = "--query Q"
		}
		switch op.Name {
		case "rename":
			usage += " --new-name N"
		case "rename_file":
			usage += " --to PATH"
		}
		short, _, _ := strings.Cut(op.Description, ". ")
		out = append(out, plugin.CLISpec{Name: strings.ReplaceAll(op.Name, "_", "-"), Usage: usage, Description: strings.TrimSuffix(short, ".")})
	}
	return append(out, plugin.CLISpec{Name: "status", Description: "the language servers running, by checkout, and what each holds"})
}

// options is the pool's options from the settings.
func options() Options {
	v := plugin.SettingValuesOf(&Manifest)
	o := Options{Engine: v["engine"], Log: events.Log}
	if o.Engine == "" {
		o.Engine = "auto"
	}
	if c := v["cap"]; c != "off" {
		if c == "" {
			c = "4G"
		}
		o.Cap, _ = gate.ParseSize(c)
	}
	o.Idle = 10 * time.Minute
	if d, err := time.ParseDuration(v["idle"]); err == nil && d > 0 {
		o.Idle = d
	}
	return o
}

// Run is the plugin, on its connection to the broker.
func Run(rw io.ReadWriteCloser) error {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	pool := NewPool(ctx, options())
	conn := plugin.NewConn(rw, func(cctx context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "initialize":
			return map[string]any{}, nil
		case "ui.settings":
			pool.SetOptions(options())
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": toolList()}, nil
		case "tools.call":
			var in struct {
				Name      string `json:"name"`
				Arguments Ask    `json:"arguments"`
				Cwd       string `json:"cwd"`
			}
			if err := jsonx.Unmarshal(params, &in); err != nil {
				return nil, err
			}
			op := OpNamed(in.Name)
			if op == nil {
				return result("no tool "+in.Name, true), nil
			}
			text, err := Do(cctx, pool, op, in.Arguments, in.Cwd)
			if err != nil {
				return result(err.Error(), true), nil
			}
			return result(text, false), nil
		case "cli.run":
			var r plugin.CLIRun
			if err := jsonx.Unmarshal(params, &r); err != nil {
				return nil, err
			}
			return runCLI(cctx, pool, r), nil
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	<-conn.Done()
	pool.CloseAll()
	return nil
}

func result(text string, isErr bool) map[string]any {
	return map[string]any{"content": []map[string]any{{"type": "text", "text": text}}, "isError": isErr}
}

func toolList() []map[string]any {
	str := func(d string) map[string]any { return map[string]any{"type": "string", "description": d} }
	num := func(d string) map[string]any {
		return map[string]any{"type": "integer", "minimum": 1, "description": d}
	}
	out := make([]map[string]any, 0, len(Ops))
	for _, op := range Ops {
		props := map[string]any{}
		var req []string
		switch op.Needs {
		case "position":
			props["file"] = str("The file, absolute or from your folder")
			props["line"] = num("The line, from 1")
			props["symbol"] = str("The name on that line to ask about (or give character)")
			props["character"] = num("The column, from 1 (or give symbol)")
			req = []string{"file", "line"}
		case "file":
			props["file"] = str("The file, absolute or from your folder")
			req = []string{"file"}
		case "query":
			props["query"] = str("The name, or part of it")
			props["file"] = str("Any file in the checkout to search, when it isn't your folder's")
			req = []string{"query"}
		}
		switch op.Name {
		case "rename":
			props["new_name"] = str("The new name")
			req = append(req, "new_name")
		case "rename_file":
			props["to"] = str("The new path, absolute or from your folder")
			req = append(req, "to")
		}
		out = append(out, map[string]any{"name": op.Name, "description": op.Description,
			"inputSchema": map[string]any{"type": "object", "properties": props, "required": req, "additionalProperties": false}})
	}
	return out
}

// runCLI is `rush ts <tool> …`, for agents with only a shell.
func runCLI(ctx context.Context, pool *Pool, r plugin.CLIRun) plugin.CLIResult {
	if r.Command == "status" {
		return plugin.CLIResult{Stdout: status(pool)}
	}
	op := OpNamed(strings.ReplaceAll(r.Command, "-", "_"))
	if op == nil {
		return plugin.CLIResult{Stderr: "rush ts: no command " + r.Command + "\n", Exit: 2}
	}
	var a Ask
	for i := 0; i < len(r.Args); i++ {
		k, v, ok := strings.Cut(r.Args[i], "=")
		if !ok && i+1 < len(r.Args) {
			i++
			v = r.Args[i]
		}
		switch k {
		case "--file", "-f":
			a.File = v
		case "--line", "-l":
			a.Line, _ = strconv.Atoi(v)
		case "--character", "-c":
			a.Character, _ = strconv.Atoi(v)
		case "--symbol", "-s":
			a.Symbol = v
		case "--query", "-q":
			a.Query = v
		case "--new-name":
			a.NewName = v
		case "--to":
			a.To = v
		default:
			return plugin.CLIResult{Stderr: "rush ts: unknown " + k + "\n", Exit: 2}
		}
	}
	text, err := Do(ctx, pool, op, a, r.Cwd)
	if err != nil {
		return plugin.CLIResult{Stderr: "rush ts: " + err.Error() + "\n", Exit: 1}
	}
	return plugin.CLIResult{Stdout: text + "\n"}
}

func status(pool *Pool) string {
	ss := pool.Servers()
	if len(ss) == 0 {
		return "no language servers running\n"
	}
	var b strings.Builder
	for root, s := range ss {
		fmt.Fprintf(&b, "%s  %s (pid %d)  %s  idle %s\n", root, s.Engine.Name, s.PID(), gate.Size(s.Footprint()), s.Idle().Round(time.Second))
	}
	return b.String()
}
