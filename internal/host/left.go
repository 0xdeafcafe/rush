package host

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Left is something an agent wrote outside its project and its scratch
// folder: what it may have left behind on the disk.
type Left struct {
	Path  string    `json:"path"`
	At    time.Time `json:"at"`             // its last write there
	Shell bool      `json:"shell,omitzero"` // read from a shell command, so a guess
}

// maxLeft is how many places a session's ledger keeps, the latest.
const maxLeft = 200

// noteLeft adds to the ledger what m's tool calls write outside the
// project and scratch, whichever agent made them. Called with mu held.
func (s *server) noteLeft(m event.Message) {
	var cwd, proj string
	for _, p := range m.Parts {
		c := p.Call
		if c == nil {
			continue
		}
		var paths []string
		shell := c.Kind == tool.Shell
		switch {
		case shell:
			paths = shellWrites(c.Input.Command)
		case c.Kind == tool.Move:
			paths = []string{c.Input.To}
		case c.Kind.Changes() && c.Kind != tool.Delete:
			paths = []string{c.Input.Path}
			for _, e := range c.Input.Edits {
				paths = append(paths, e.Path)
			}
		}
		for _, path := range paths {
			if path == "" {
				continue
			}
			if cwd == "" {
				cwd = or(c.Input.Cwd, s.cfg.Cwd)
				proj = s.projectOf(cwd)
			}
			if path = absPath(path, cwd); path != "" && outside(path, cwd, proj, TempDir(s.cfg.ID), s.cfg.Account.Dir) {
				s.info.Left = addLeft(s.info.Left, Left{Path: path, At: time.Now(), Shell: shell})
			}
		}
	}
}

// projectOf is projectOf, remembered for the session's folder: git runs
// once a folder, not for every call.
func (s *server) projectOf(cwd string) string {
	if s.projFor != cwd {
		s.projFor, s.proj = cwd, projectOf(cwd)
	}
	return s.proj
}

func addLeft(list []Left, l Left) []Left {
	list = slices.DeleteFunc(list, func(o Left) bool { return o.Path == l.Path })
	list = append(list, l)
	return list[max(0, len(list)-maxLeft):]
}

// absPath is p as an absolute path, ~ and $HOME read as home; relative ones
// from cwd. "" when it can't be told ($VAR, globs).
func absPath(p, cwd string) string {
	home, _ := os.UserHomeDir()
	switch {
	case p == "~" || strings.HasPrefix(p, "~/"):
		p = home + p[1:]
	case strings.HasPrefix(p, "$HOME/") || strings.HasPrefix(p, "${HOME}/"):
		p = home + p[strings.Index(p, "/"):]
	}
	if strings.ContainsAny(p, "$*?`") {
		return ""
	}
	if !filepath.IsAbs(p) {
		if cwd == "" {
			return ""
		}
		p = filepath.Join(cwd, p)
	}
	return filepath.Clean(p)
}

// outside is whether path is somewhere the ledger keeps: not the project
// or the folder the agent works in, not its scratch, not an agent's own
// state (its harness's or rush's), not a shared cache, not a device.
func outside(path, cwd, proj, tmp, harness string) bool {
	home, _ := os.UserHomeDir()
	for _, in := range []string{cwd, proj, tmp, harness, state.Dir(), "/dev",
		filepath.Join(home, ".cache"), filepath.Join(home, "Library", "Caches"), filepath.Join(home, "go", "pkg"),
		filepath.Join(home, ".npm"), filepath.Join(home, ".cargo"), filepath.Join(home, ".rustup")} {
		if in != "" && within(in, path) {
			return false
		}
	}
	// Every harness keeps its state in a dot folder of home: ~/.claude, ~/.codex…
	if rel, err := filepath.Rel(home, path); err == nil && strings.HasPrefix(rel, ".") && !strings.HasPrefix(rel, "..") {
		top := strings.Split(rel, string(filepath.Separator))[0]
		if slices.Contains(harnessDirs, top) {
			return false
		}
	}
	return path != "/" && path != home
}

// harnessDirs are home's dot folders agents keep their own state in.
var harnessDirs = []string{".claude", ".codex", ".gemini", ".kimi", ".pi", ".copilot", ".cursor", ".opencode", ".amp", ".qwen", ".antigravity"}

// shellWrites are the paths a shell command plainly writes: redirects,
// output flags, and what mkdir, touch, tee, cp, mv, ln, rsync and git clone
// make. Only absolute and home paths; the rest are where it works.
// shortcut: a word-split read, not a shell parser; quotes with spaces and a
// cd inside the command fool it. Enough to see most of what's left behind.
func shellWrites(cmd string) []string {
	var out []string
	keep := func(p string) {
		p = strings.Trim(p, `"'`)
		if strings.HasPrefix(p, "/") || strings.HasPrefix(p, "~") || strings.HasPrefix(p, "$HOME") || strings.HasPrefix(p, "${HOME}") {
			out = append(out, p)
		}
	}
	for _, seg := range splitAny(cmd, "&&", "||", ";", "|", "\n") {
		f := strings.Fields(seg)
		for len(f) > 0 && (strings.Contains(f[0], "=") || f[0] == "sudo" || f[0] == "env" || f[0] == "time" || f[0] == "command") {
			f = f[1:]
		}
		var args []string // what isn't a flag or a redirect
		for i := 0; i < len(f); i++ {
			w := strings.TrimLeft(f[i], "0123456789&")
			switch {
			case w == ">" || w == ">>" || w == "-o" || w == "--output" || w == "-O":
				if i+1 < len(f) {
					keep(f[i+1])
					i++
				}
			case strings.HasPrefix(w, ">"):
				keep(strings.TrimLeft(w, ">"))
			case strings.HasPrefix(w, "--output="):
				keep(strings.TrimPrefix(w, "--output="))
			case strings.HasPrefix(w, "-") || i == 0:
			default:
				args = append(args, f[i])
			}
		}
		if len(f) == 0 || len(args) == 0 {
			continue
		}
		switch filepath.Base(f[0]) {
		case "mkdir", "touch", "tee":
			for _, a := range args {
				keep(a)
			}
		case "cp", "mv", "ln", "rsync":
			keep(args[len(args)-1])
		case "git":
			if args[0] == "clone" && len(args) >= 3 {
				keep(args[len(args)-1])
			}
		}
	}
	return out
}

func splitAny(s string, seps ...string) []string {
	for _, sep := range seps[1:] {
		s = strings.ReplaceAll(s, sep, seps[0])
	}
	return strings.Split(s, seps[0])
}
