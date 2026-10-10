package host

import (
	"os"
	"path/filepath"
	"time"

	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// editsToMove is how many edits in a row, in one other checkout, move a
// session there.
const editsToMove = 3

// followEdits moves a session to the checkout its agent edits in, when
// that isn't its own: editsToMove edits in a row there. Claude Code puts
// its shell back in the folder it started in after a cd out of it, so
// where it says it works (followCwd) never leaves; an agent sent to work
// on another checkout, or one of its worktrees, edits there by full path.
// Called with mu held; git runs off it.
func (s *server) followEdits(m event.Message) {
	for _, p := range m.Parts {
		c := p.Call
		if c == nil || !c.Kind.Changes() || c.Kind == tool.Delete {
			continue
		}
		path := c.Input.Path
		if c.Kind == tool.Move {
			path = c.Input.To
		}
		if path = absPath(path, or(c.Input.Cwd, s.cfg.Cwd)); path == "" || IsTemp(path) {
			continue
		}
		s.edited = append(s.edited, filepath.Dir(path))
		s.edited = s.edited[max(0, len(s.edited)-editsToMove):]
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
