package host

import (
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// editsToMove is how many edits or shell commands in a row, in one other
// checkout, move a session there.
const editsToMove = 3

// followEdits moves a session to the checkout its agent works in, when
// that isn't its own: editsToMove edits, or shell commands that cd there,
// in a row. Claude Code puts its shell back in the folder it started in
// after a cd out of it, so where it says it works (followCwd) never
// leaves; an agent sent to work on another checkout, or one of its
// worktrees, edits there by full path and starts each command with a cd.
// Called with mu held; git runs off it.
func (s *server) followEdits(m event.Message) {
	for _, p := range m.Parts {
		if d := s.workedIn(p.Call); d != "" && !IsTemp(d) {
			s.edited = append(s.edited, d)
			s.edited = s.edited[max(0, len(s.edited)-editsToMove):]
		}
	}
	if len(s.edited) < editsToMove {
		return
	}
	dirs, was := append([]string(nil), s.edited...), s.cfg.Cwd
	go func() {
		for i, d := range dirs {
			dirs[i] = nearest(d)
		}
		top, ok := editedIn(was, dirs, actions.RepoRoot)
		if !ok {
			return
		}
		s.mu.Lock()
		defer s.mu.Unlock()
		if was != s.cfg.Cwd {
			return
		}
		s.edited = nil
		s.cfg.Cwd, s.info.Cwd, s.info.CwdAt = top, top, time.Now()
		s.saveConfig()
		s.publish()
	}()
}

// workedIn is the folder call c works in, when it says: an edit's, or a
// shell command's that cds first or runs elsewhere. "" otherwise: a
// command that doesn't runs where the shell was put back.
func (s *server) workedIn(c *tool.Call) string {
	switch {
	case c == nil:
		return ""
	case c.Kind == tool.Shell:
		shell := or(c.Input.Cwd, or(s.startCwd, s.cfg.Cwd))
		if d := cdTarget(c.Input.Command); d != "" {
			return absPath(d, shell)
		}
		return c.Input.Cwd
	case !c.Kind.Changes() || c.Kind == tool.Delete:
		return ""
	}
	path := c.Input.Path
	if c.Kind == tool.Move {
		path = c.Input.To
	}
	if path = absPath(path, or(c.Input.Cwd, s.cfg.Cwd)); path == "" {
		return ""
	}
	return filepath.Dir(path)
}

// cdTarget is the folder a shell command cds (or pushds) into before
// anything else, quotes taken off; "" when it doesn't, or cds home or
// back.
func cdTarget(cmd string) string {
	cmd = strings.TrimSpace(cmd)
	rest, ok := strings.CutPrefix(cmd, "cd ")
	if !ok {
		if rest, ok = strings.CutPrefix(cmd, "pushd "); !ok {
			return ""
		}
	}
	rest = strings.TrimSpace(rest)
	var d string
	if q := rest[:min(1, len(rest))]; q == `"` || q == "'" {
		end := strings.Index(rest[1:], q)
		if end < 0 {
			return ""
		}
		d = rest[1 : end+1]
	} else {
		d = rest
		if i := strings.IndexAny(d, " \t\n;&|)"); i >= 0 {
			d = d[:i]
		}
	}
	if d == "-" || d == "~" || d == "" {
		return ""
	}
	return d
}

// editedIn is the top of the one checkout, not was's, every one of dirs
// is in.
func editedIn(was string, dirs []string, root func(string) string) (string, bool) {
	top := ""
	for _, d := range dirs {
		t := root(d)
		if t == "" || top != "" && t != top {
			return "", false
		}
		top = t
	}
	if top == "" || top == root(was) {
		return "", false
	}
	return top, true
}

// nearest is d, or the closest folder above it that's there: a file
// written into a folder it made has git ask where it is.
func nearest(d string) string {
	for {
		if st, err := os.Stat(d); err == nil && st.IsDir() {
			return d
		}
		up := filepath.Dir(d)
		if up == d {
			return d
		}
		d = up
	}
}
