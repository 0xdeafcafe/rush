// Package gate is the bundled plugin that queues intensive programs
// (tsc, go, cargo…) across sessions: see internal/gate. Its settings say
// which programs, in what scope, how many at once and how far apart; rush
// gate run and the shims read them from disk, each its own process.
package gate

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "gate"

// RulesEnv, when set, is taken as the overrides with the gate on, over
// the settings: for scripts and tests.
const RulesEnv = "RUSH_GATE_RULES"

// Manifest is the gate plugin's.
var Manifest = plugin.Manifest{
	Name: Name,
	Description: "Queues intensive programs (tsc, go, cargo…) your agents run, so only a few run at once " +
		"in a worktree, a repo or the whole system. rush gate status shows the queues.",
	Command: []string{"rush"},
	Settings: []plugin.SettingSpec{
		{Key: "names", Title: "Programs", Type: "text", Default: gate.Defaults["names"],
			Description: "The programs to queue, by name, separated by commas"},
		{Key: "scope", Title: "Shared across", Type: "choice", Choices: gate.Scopes, Default: gate.Defaults["scope"],
			Description: "One queue per worktree, per repo with all its worktrees, or for the whole system"},
		{Key: "parallel", Title: "At once", Type: "choice", Choices: []string{"1", "2", "3", "4", "5", "6", "7", "8"},
			Default: gate.Defaults["parallel"]},
		{Key: "stagger", Title: "Between starts", Type: "choice", Choices: []string{"0s", "2s", "5s", "10s", "30s"},
			Default: gate.Defaults["stagger"]},
		{Key: "overrides", Title: "Per program", Type: "text",
			Description: "Scope/at once/between starts for one program, any part left out: tsc=system/1/10s, go=worktree/4"},
	},
	Requires: plugin.Requires{OS: []string{"darwin", "linux"}},
	MemoryMB: 32,
}

// Rules is the rule for each program, or nil when the gate is off.
func Rules() map[string]gate.Rule {
	if v, ok := os.LookupEnv(RulesEnv); ok {
		return gate.ParseRules(map[string]string{"names": "", "overrides": v})
	}
	if !plugin.BundledOn(Name) {
		return nil
	}
	return gate.ParseRules(plugin.SettingValuesOf(&Manifest))
}

// WriteShims makes gate.BinDir hold a shim for each program the gate
// queues, and nothing else: none when it's off.
func WriteShims() {
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
	// TMPDIR as given and as resolved: a session's runs through a link.
	tmp, resolved := os.TempDir(), ""
	if r, err := filepath.EvalSymlinks(tmp); err == nil {
		resolved = r
	}
	for _, d := range []string{tmp, resolved, "/tmp", "/private/tmp"} {
		if d == "" {
			continue
		}
		if strings.HasPrefix(path, filepath.Clean(d)+string(filepath.Separator)) {
			return true
		}
	}
	return strings.Contains(path, string(filepath.Separator)+"go-build")
}

// HookCommand is Claude Code's PreToolUse hook for Bash while the gate is
// on, or "".
func HookCommand() string {
	if exe := Exe(); exe != "" && Rules() != nil {
		return quote(exe) + " gate hook"
	}
	return ""
}

// Run is the plugin, on its connection to the broker: it only keeps the
// shims in step with the settings.
func Run(rw io.ReadWriteCloser) error {
	conn := plugin.NewConn(rw, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		switch method {
		case "initialize", "ui.settings":
			WriteShims()
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
