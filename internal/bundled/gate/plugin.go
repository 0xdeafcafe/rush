// Package gate is the bundled plugin that shares the machine between the
// intensive programs (tsc, go, cargo…) sessions run: see internal/gate.
// Its settings say which programs, the memory and time each may take, and
// the memory kept free; rush gate run and the shims read them from disk,
// each its own process.
package gate

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"

	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "gate"

// RulesEnv, when set, is taken as the per-program limits with the gate
// on, over the settings: for scripts and tests.
const RulesEnv = "RUSH_GATE_RULES"

// Manifest is the gate plugin's.
var Manifest = plugin.Manifest{
	Name: Name,
	Description: "Shares the machine between the intensive programs (tsc, vitest, go, golangci-lint…) your agents run: " +
		"each starts once there's memory for it, fairly by agent, and gives way on the CPU as it ages; a run that " +
		"outlives its agent or its limits is stopped, and the newest paused while memory is critical. Package scripts' " +
		"runs too, through a node preload. Watchers and dev servers go straight through. rush gate status shows them. " +
		"Past the gate, when the machine is all but out of memory it stops the largest program any agent started.",
	Command: []string{"rush"},
	Settings: []plugin.SettingSpec{
		{Key: "names", Title: "Programs", Type: "text", Default: gate.Defaults["names"],
			Description: "The programs to gate, by name, separated by commas"},
		{Key: "limit", Title: "Time limit", Type: "choice", Choices: []string{"10m", "15m", "30m", "1h", "2h"},
			Default: gate.Defaults["limit"], Description: "How long a run may go before it's stopped"},
		{Key: "reserve", Title: "Kept free", Type: "choice", Choices: []string{"5%", "10%", "15%", "20%", "25%"},
			Default: gate.Defaults["reserve"], Description: "The share of memory no run starts into"},
		{Key: "floor", Title: "Last resort", Type: "choice", Choices: []string{"off", "3%", "5%", "8%", "10%"},
			Default: gate.Defaults["floor"], Description: "The memory left at which the largest program any agent started is stopped, gated or not"},
		{Key: "limits", Title: "Per program", Type: "text",
			Description: "Memory a run is expected to need and its time limit, either left out: tsc=6G/45m, vitest=4G. " +
				"A run holding three times its memory is stopped"},
	},
	Requires: plugin.Requires{OS: []string{"darwin", "linux"}},
	MemoryMB: 32,
}

// Rules is the rule for each program, or nil when the gate is off.
func Rules() map[string]gate.Rule {
	if v, ok := os.LookupEnv(RulesEnv); ok {
		return gate.ParseRules(map[string]string{"names": "", "limits": v})
	}
	if !plugin.BundledOn(Name) {
		return nil
	}
	return gate.ParseRules(plugin.SettingValuesOf(&Manifest))
}

// Reserve is the share of memory kept free of gated runs.
func Reserve() float64 {
	if _, ok := os.LookupEnv(RulesEnv); ok || !plugin.BundledOn(Name) {
		return gate.ParseReserve(nil)
	}
	return gate.ParseReserve(plugin.SettingValuesOf(&Manifest))
}

// Floor is the memory left, in percent, at which the guard stops agents'
// programs; 0 when it's off or the gate is.
func Floor() int {
	if _, ok := os.LookupEnv(RulesEnv); ok {
		return gate.ParseFloor(nil)
	}
	if !plugin.BundledOn(Name) {
		return 0
	}
	return gate.ParseFloor(plugin.SettingValuesOf(&Manifest))
}

// Tell and Log are the guard's: set by rush's command, which can reach
// sessions' inboxes and logs as this package can't.
var (
	Tell func(session, text string)
	Log  func(kind string, fields map[string]any)
)

// WriteShims makes gate.BinDir hold a shim for each program the gate
// queues, and nothing else: none when it's off. It keeps the node
// preload in step too.
func WriteShims() {
	defer writeNode()
	d := gate.BinDir()
	exe := ExeFor(d)
	if exe == "" {
		return
	}
	if os.MkdirAll(d, 0o700) != nil {
		return
	}
	rules := Rules()
	ents, _ := os.ReadDir(d)
	for _, e := range ents {
		if _, ok := rules[e.Name()]; !ok {
			_ = os.Remove(filepath.Join(d, e.Name()))
		}
	}
	for name := range rules {
		if strings.Contains(name, "/") || strings.HasPrefix(name, ".") {
			continue
		}
		path := filepath.Join(d, name)
		want := []byte("#!/bin/sh\nexec " + quote(exe) + " gate shim " + quote(name) + " \"$@\"\n")
		if b, err := os.ReadFile(path); err == nil && bytes.Equal(b, want) {
			continue
		}
		if os.WriteFile(path+".tmp", want, 0o700) == nil {
			_ = os.Rename(path+".tmp", path)
		}
	}
}

// writeNode writes the node preload, always, since a session's
// NODE_OPTIONS names it for its whole life, and the names it reads: none
// when the gate is off, so it does nothing.
func writeNode() {
	d := gate.NodeDir()
	if os.MkdirAll(d, 0o700) != nil {
		return
	}
	writeIfChanged(gate.PreloadPath(), []byte(gate.Preload))
	rules, exe := Rules(), ExeFor(d)
	if rules == nil || exe == "" {
		_ = os.Remove(gate.NamesPath())
		return
	}
	names := make([]string, 0, len(rules))
	for n := range rules {
		names = append(names, n)
	}
	slices.Sort(names)
	writeIfChanged(gate.NamesPath(), []byte(exe+"\n"+strings.Join(names, "\n")+"\n"))
}

// NodeOptions is NODE_OPTIONS with the preload required first, or as it
// is when it's there already or the preload isn't written.
func NodeOptions(cur string) string {
	p := gate.PreloadPath()
	if strings.Contains(cur, p) || strings.ContainsAny(p, " \"'\\") {
		return cur
	}
	if _, err := os.Stat(p); err != nil {
		return cur
	}
	return strings.TrimSpace("--require " + p + " " + cur)
}

func writeIfChanged(path string, want []byte) {
	if b, err := os.ReadFile(path); err == nil && bytes.Equal(b, want) {
		return
	}
	if os.WriteFile(path+".tmp", want, 0o600) == nil {
		_ = os.Rename(path+".tmp", path)
	}
}

// Exe is the running rush, links resolved, or "".
func Exe() string {
	exe, err := os.Executable()
	if err != nil {
		return ""
	}
	if r, err := filepath.EvalSymlinks(exe); err == nil {
		exe = r
	}
	return exe
}

// ExeFor is the rush a stand-in written into dir runs: the running one,
// unless that's a scratch build (a test's, go run's, one in the temp
// folder) and dir outlives it; then the rush on PATH, or "" for none.
func ExeFor(dir string) string {
	exe := Exe()
	if !scratch(exe) || scratch(dir) {
		return exe
	}
	p, err := exec.LookPath("rush")
	if err != nil {
		return ""
	}
	if r, err := filepath.EvalSymlinks(p); err == nil {
		p = r
	}
	if scratch(p) {
		return ""
	}
	return p
}

// scratch is whether path is somewhere that comes and goes: the temp
// folder, or go's build cache.
func scratch(path string) bool {
	tmp := os.TempDir()
	if r, err := filepath.EvalSymlinks(tmp); err == nil {
		tmp = r
	}
	for _, d := range []string{tmp, "/tmp", "/private/tmp"} {
		if strings.HasPrefix(path, filepath.Clean(d)+string(filepath.Separator)) {
			return true
		}
	}
	return strings.Contains(path, string(filepath.Separator)+"go-build")
}

// HookCommand is Claude Code's PreToolUse hook for Bash: it gates what
// the gate's rules name, and runs a long call as a script rush can follow.
// "" when there's no rush to run it.
func HookCommand() string {
	if exe := Exe(); exe != "" {
		return quote(exe) + " gate hook"
	}
	return ""
}

// Run is the plugin, on its connection to the broker: it keeps the shims
// in step with the settings, and runs the guard while the floor is set.
func Run(rw io.ReadWriteCloser) error {
	var (
		mu        sync.Mutex
		stopGuard context.CancelFunc
	)
	guard := func() {
		mu.Lock()
		defer mu.Unlock()
		// Started afresh each time, so it guards by the settings as they are.
		if stopGuard != nil {
			stopGuard()
			stopGuard = nil
		}
		if f := Floor(); f > 0 {
			ctx, cancel := context.WithCancel(context.Background())
			stopGuard = cancel
			names := map[string]bool{}
			for n := range Rules() {
				names[n] = true
			}
			go gate.Guard{Floor: f, Names: names, Tell: Tell, Log: Log}.Run(ctx)
		}
	}
	defer func() {
		mu.Lock()
		if stopGuard != nil {
			stopGuard()
		}
		mu.Unlock()
	}()
	conn := plugin.NewConn(rw, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		switch method {
		case "initialize", "ui.settings":
			WriteShims()
			guard()
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": []any{}}, nil
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	<-conn.Done()
	return nil
}

// quote is s as one word to sh.
func quote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }
