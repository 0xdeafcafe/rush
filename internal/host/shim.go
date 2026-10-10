package host

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent"
	bgate "github.com/0xdeafcafe/rush/internal/bundled/gate"
	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A session rush starts finds its stand-ins first on PATH: a codex or
// claude its shell runs is `rush spawn codex …`, which hosts the run so
// you can talk to it while it works, or runs the real program when it
// can't.

// ShimDir is the folder of stand-ins.
func ShimDir() string { return state.CachePath("shims") }

// shimScript is the stand-in for program: rush spawn, or the real one
// off PATH when this rush has gone.
func shimScript(exe, program string) []byte {
	return []byte("#!/bin/sh\n# rush's stand-in: rush hosts the run when it can.\n" +
		"a=" + quote(exe) + "\n" +
		`[ -x "$a" ] && exec "$a" spawn ` + quote(program) + ` "$@"` + "\n" +
		"p=; IFS=:; for x in $PATH; do [ \"$x\" = " + quote(ShimDir()) + " ] || p=\"$p${p:+:}$x\"; done; unset IFS\n" +
		"PATH=$p; export PATH\nexec " + quote(program) + ` "$@"` + "\n")
}

// WriteShims puts a stand-in for every registered agent's program in
// ShimDir, and is the folder, or "" when it can't.
func WriteShims() string {
	d := ShimDir()
	exe := bgate.ExeFor(d) // never a scratch build: every agent here runs them
	if exe == "" {
		return ""
	}
	if os.MkdirAll(d, 0o700) != nil {
		return ""
	}
	for _, a := range agent.All() {
		p := agent.ProgramOf(a.Kind())
		if p == "" {
			continue
		}
		path, want := filepath.Join(d, p), shimScript(exe, p)
		if b, err := os.ReadFile(path); err == nil && bytes.Equal(b, want) {
			continue
		}
		tmp := path + ".tmp"
		if os.WriteFile(tmp, want, 0o700) != nil || os.Rename(tmp, path) != nil {
			return ""
		}
	}
	return d
}

// WithoutShims is path with the stand-ins' folder, and the gate's, taken
// out.
func WithoutShims(path string) string {
	d, g := filepath.Clean(ShimDir()), filepath.Clean(gate.BinDir())
	var keep []string
	for _, p := range filepath.SplitList(path) {
		if p != "" && filepath.Clean(p) != d && filepath.Clean(p) != g {
			keep = append(keep, p)
		}
	}
	return strings.Join(keep, string(filepath.ListSeparator))
}

// shimEnv is what a session's agent gets so its shell finds the
// stand-ins: PATH with them first, and which session it is. Claude Code
// runs each command in a shell of your own profile's making, which sets
// PATH again: it sources CLAUDE_ENV_FILE after, so that says it too.
func (s *server) shimEnv() []string {
	env := []string{"RUSH_SESSION=" + s.cfg.ID}
	var sh strings.Builder
	// One you set is still read; another session's is its own.
	if own := os.Getenv("CLAUDE_ENV_FILE"); own != "" && !strings.HasPrefix(own, Root()+string(filepath.Separator)) {
		sh.WriteString("[ -f " + quote(own) + " ] && . " + quote(own) + "\n")
	}
	sh.WriteString("export RUSH_SESSION=" + quote(s.cfg.ID) + "\n")
	if os.Getenv("RUSH_NO_SHIM") == "" {
		if d := WriteShims(); d != "" {
			// The gate's shims come next: whichever agent it is, a queued
			// program it runs waits its turn.
			bgate.WriteShims()
			g := gate.BinDir()
			env = append(env, "PATH="+d+string(filepath.ListSeparator)+g+string(filepath.ListSeparator)+WithoutShims(os.Getenv("PATH")))
			sh.WriteString("export PATH=" + quote(d) + ":" + quote(g) + "\":$PATH\"\n")
			// And every node process loads the gate's preload, for what
			// package scripts run from node_modules/.bin, past the shims.
			if no := bgate.NodeOptions(os.Getenv("NODE_OPTIONS")); no != os.Getenv("NODE_OPTIONS") {
				env = append(env, "NODE_OPTIONS="+no)
				p := gate.PreloadPath()
				sh.WriteString("case \" $NODE_OPTIONS \" in *" + quote(p) + "*) ;; *) export NODE_OPTIONS=\"--require " + p + " ${NODE_OPTIONS:-}\" ;; esac\n")
			}
			// Other agents run commands in a login zsh, whose profile puts
			// PATH in its own order: the stand-ins go first again after it.
			if z := writeZsh(d); z != "" && !agent.ReadsAsClaude(agent.Kind(s.cfg.Kind)) {
				home, _ := os.UserHomeDir()
				env = append(env, "RUSH_ZDOTDIR="+or(os.Getenv("RUSH_ZDOTDIR"), or(os.Getenv("ZDOTDIR"), home)), "ZDOTDIR="+z)
			}
		}
	}
	f := filepath.Join(dir(s.cfg.ID), "shell-env.sh")
	if os.WriteFile(f, []byte(sh.String()), 0o600) == nil {
		env = append(env, "CLAUDE_ENV_FILE="+f)
	}
	return env
}

// zshFiles are the startup files zsh reads from ZDOTDIR.
var zshFiles = []string{".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"}

// writeZsh puts startup files for zsh in a folder beside the stand-ins,
// and is it, or "" when it can't: each reads your own (from where
// RUSH_ZDOTDIR says, your home unless you moved them), and those read
// last in a shell put the stand-ins first on PATH.
func writeZsh(shims string) string {
	z := filepath.Join(shims, "zsh")
	if os.MkdirAll(z, 0o700) != nil {
		return ""
	}
	g := gate.BinDir()
	first := "path=(" + quote(shims) + " " + quote(g) + " ${${path:#" + quote(shims) + "}:#" + quote(g) + "})\n"
	for _, f := range zshFiles {
		last := ""
		if f == ".zshenv" || f == ".zshrc" || f == ".zlogin" {
			last = first
		}
		body := "# rush: your own " + f + ", read from where it is.\n" +
			"_rush_z=$ZDOTDIR; ZDOTDIR=${RUSH_ZDOTDIR:-$HOME}\n" +
			"[ -f \"$ZDOTDIR/" + f + "\" ] && . \"$ZDOTDIR/" + f + "\"\n" +
			"export RUSH_ZDOTDIR=$ZDOTDIR; ZDOTDIR=$_rush_z; unset _rush_z\n" + last
		path := filepath.Join(z, f)
		if b, err := os.ReadFile(path); err == nil && string(b) == body {
			continue
		}
		if os.WriteFile(path+".tmp", []byte(body), 0o600) != nil || os.Rename(path+".tmp", path) != nil {
			return ""
		}
	}
	return z
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

// quote is s as one word to sh.
func quote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }
