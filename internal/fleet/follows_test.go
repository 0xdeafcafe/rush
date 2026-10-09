package fleet

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// A row follows its transcript into a checkout or up a folder, not into
// its memory or rush's own folder.
func TestFollows(t *testing.T) {
	root := t.TempDir()
	repo, mem := filepath.Join(root, "repo"), filepath.Join(root, "claude", "memory")
	os.MkdirAll(filepath.Join(repo, ".git"), 0o755)
	os.MkdirAll(mem, 0o755)
	l := &Loader{git: map[string]gitInfo{}}
	now := time.Now()
	for _, c := range []struct {
		cwd, dir string
		want     bool
	}{
		{repo, mem, false},                     // its memory
		{filepath.Join(root, "x"), repo, true}, // into a checkout
		{filepath.Join(root, "notes", "a"), filepath.Join(root, "notes"), true}, // up
		{"", mem, true}, // nowhere yet
	} {
		if got := l.follows(c.cwd, c.dir, now); got != c.want {
			t.Errorf("follows(%q, %q) = %v", c.cwd, c.dir, got)
		}
	}
}
