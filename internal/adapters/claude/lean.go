package claude

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/rush/internal/state"
)

// Claude Code is JavaScript run by Bun, and two thirds of its heap is its
// own code: compiled and bytecode. Bun reads its JS engine's options from
// BUN_JSC_* and runs a --preload before Claude Code starts; with these an
// idle session holds less. Measured (a turn of 137 file reads, then five
// idle minutes, physical footprint): 103 MB every time with both, against
// 108–126 MB without, for the same CPU. Each alone gets ~105; the JIT
// entirely off stops Claude Code from starting, and discarding unused
// bytecode saves a little more for twice the CPU.

// dfgOff is the engine option that keeps its two optimising compilers off.
const dfgOff = "BUN_JSC_useDFGJIT"

// leanPreload runs in Claude Code before it does: it takes rush's
// variables back out, so the commands, hooks and MCP servers it starts
// get the environment they would have without rush (the engine read them
// already), and collects garbage once a minute, so a session gone idle
// hands back what it freed.
var leanPreload = []byte(`// rush: Claude Code's engine options are its own, not its children's.
delete process.env.BUN_OPTIONS;
delete process.env.` + dfgOff + `;
// A full collection a minute, so an idle session hands back what it freed.
setInterval(() => { try { Bun.gc(true); } catch {} }, 60_000).unref();
`)

// leanEnv is what rush adds to Claude Code's environment, or nothing when
// you set BUN_OPTIONS or the option yourself: yours stand.
func leanEnv() []string {
	if os.Getenv("BUN_OPTIONS") != "" || os.Getenv(dfgOff) != "" {
		return nil
	}
	p := writeLeanPreload()
	if p == "" || strings.ContainsAny(p, " \t\"'\\") { // BUN_OPTIONS splits on spaces
		return nil
	}
	return []string{"BUN_OPTIONS=--preload " + p, dfgOff + "=0"}
}

// writeLeanPreload puts leanPreload in rush's cache, and is its path, or
// "" when it can't.
func writeLeanPreload() string {
	d := state.CachePath("claude")
	path := filepath.Join(d, "lean-preload.js")
	if b, err := os.ReadFile(path); err == nil && bytes.Equal(b, leanPreload) {
		return path
	}
	if os.MkdirAll(d, 0o700) != nil {
		return ""
	}
	tmp := path + ".tmp"
	if os.WriteFile(tmp, leanPreload, 0o600) != nil || os.Rename(tmp, path) != nil {
		return ""
	}
	return path
}
