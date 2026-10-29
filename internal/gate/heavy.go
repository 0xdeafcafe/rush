package gate

import "slices"

// Heavy is whether name run with args is the kind of run the gate queues:
// a build, a check, a test run. A watcher, a dev server, a language server
// or a question (--help, --version) runs for hours or a moment, and would
// hold a slot all that time or wait for one for nothing; it goes straight
// through.
func Heavy(name string, args []string) bool {
	for _, a := range args {
		switch a {
		case "--watch", "--watch=true", "--lsp", "--stdio", "--help", "-h", "--version":
			return false
		case "-w":
			if name == "tsc" || name == "tsgo" || name == "vitest" || name == "jest" {
				return false
			}
		}
	}
	sub := subcommand(args)
	if want, ok := heavySubs[name]; ok {
		return slices.Contains(want, sub)
	}
	return !slices.Contains(lightSubs, sub)
}

// heavySubs are the programs that do many things, of which only these
// cost: go run, cargo run and next dev are servers as often as not, go env
// and go list are quick.
var heavySubs = map[string][]string{
	"go":         {"build", "test", "vet", "install", "generate"},
	"cargo":      {"build", "test", "check", "clippy", "bench", "doc", "install", "b", "t", "c", "d", "nextest", "llvm-cov", "miri", "fuzz", "tarpaulin", "mutants", "hack", "udeps", "rustc", "rustdoc"},
	"vite":       {"build"},
	"next":       {"build"},
	"webpack":    {"", "build", "bundle"},
	"playwright": {"test"},
}

// lightSubs are the subcommands of any other gated program that never cost.
var lightSubs = []string{"watch", "dev", "serve", "preview", "start", "version", "help", "completion", "init"}

// subcommand is args' first word that isn't a flag, a flag's value or a
// toolchain (cargo +nightly build), or "".
func subcommand(args []string) string {
	for i := 0; i < len(args); i++ {
		switch a := args[i]; {
		case globalValues[a]:
			i++
		case a != "" && a[0] != '-' && a[0] != '+':
			return a
		}
	}
	return ""
}

// globalValues are flags given before a subcommand whose next word is
// their value: go -C dir build, cargo --manifest-path x/Cargo.toml test.
var globalValues = map[string]bool{"-C": true, "--manifest-path": true, "--config": true, "-Z": true, "--color": true}
