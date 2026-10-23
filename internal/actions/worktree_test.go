package actions

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// repo makes a git repository with one commit on main, and gives its
// folder, symlinks resolved as git gives them.
func repo(t *testing.T) string {
	t.Helper()
	d, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	run(t, d, "init", "-q", "-b", "main")
	run(t, d, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "first")
	return d
}

func run(t *testing.T, dir string, args ...string) string {
	t.Helper()
	out, err := exec.Command("git", append([]string{"-C", dir}, args...)...).CombinedOutput()
	if err != nil {
		t.Fatalf("git %s: %v: %s", strings.Join(args, " "), err, out)
	}
	return strings.TrimSpace(string(out))
}

// A worktree made from inside another goes under the main checkout, not
// inside that worktree, and starts from the main checkout's HEAD, not the
// worktree's.
func TestMakeWorktreeFromAWorktreeGoesUnderMain(t *testing.T) {
	main := repo(t)
	first, err := MakeWorktree(main, "one", "rush/one", "")
	if err != nil {
		t.Fatal(err)
	}
	if want := filepath.Join(main, ".claude", "worktrees", "one"); first.Path != want || first.Repo != main || first.Target != "main" {
		t.Fatalf("first: %+v", first)
	}
	// Work in the first that main hasn't got.
	run(t, first.Path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "wip")
	mainHead := run(t, main, "rev-parse", "HEAD")
	if err := os.MkdirAll(filepath.Join(first.Path, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	second, err := MakeWorktree(filepath.Join(first.Path, "sub"), "two", "rush/two", "")
	if err != nil {
		t.Fatal(err)
	}
	if want := filepath.Join(main, ".claude", "worktrees", "two"); second.Path != want {
		t.Fatalf("made in %s, want %s: nested in the worktree it was made from", second.Path, want)
	}
	if second.Base != mainHead || run(t, second.Path, "rev-parse", "HEAD") != mainHead {
		t.Fatalf("based on %s, want the main checkout's HEAD %s", second.Base, mainHead)
	}
	if got := MainRoot(second.Path); got != main {
		t.Fatalf("MainRoot of the second: %s", got)
	}
	// NewWorktreeOn, as the fork sheet and plugins use it, the same.
	dir, err := NewWorktreeOn(first.Path, "three", "worktree-three", "")
	if err != nil {
		t.Fatal(err)
	}
	if dir != filepath.Join(main, ".claude", "worktrees", "three") {
		t.Fatalf("NewWorktreeOn nested: %s", dir)
	}
	// WorktreeAt knows the first for what it is; the main checkout isn't one.
	if m, ok := WorktreeAt(first.Path); !ok || m.Branch != "rush/one" || m.Repo != main || m.Target != "main" || m.Base != mainHead {
		t.Fatalf("WorktreeAt: %+v %v", m, ok)
	}
	if _, ok := WorktreeAt(main); ok {
		t.Fatal("the main checkout taken for a worktree")
	}
}
