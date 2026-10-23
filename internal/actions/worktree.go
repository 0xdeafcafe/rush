package actions

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// RepoRoot is the top of the git checkout dir is in, or "" if it isn't in
// one. In a linked worktree it's the worktree's own top: see MainRoot for
// the checkout it was made from.
func RepoRoot(dir string) string {
	out, err := gitOut(dir, "rev-parse", "--show-toplevel")
	if err != nil {
		return ""
	}
	return out
}

// MainRoot is the main checkout of the repository dir is in, through any
// linked worktree, or "" if it isn't in one: where rush makes worktrees,
// so one made from inside another goes beside it, not inside it.
func MainRoot(dir string) string {
	common, err := gitOut(dir, "rev-parse", "--path-format=absolute", "--git-common-dir")
	if err != nil || common == "" {
		return ""
	}
	if filepath.Base(common) != ".git" {
		return RepoRoot(dir) // a bare repository's, or one with its .git elsewhere
	}
	return filepath.Dir(common)
}

// Made is a linked worktree: where it is, its branch, and where it came
// from.
type Made struct {
	Path   string // the worktree's top
	Dir    string // the folder matching the one asked from, inside it
	Repo   string // the main checkout it belongs to
	Branch string
	Base   string // the commit it started from, in full
	// Target is the branch the main checkout had checked out when it was
	// made: where its work is meant to land.
	Target string
}

// NewWorktree makes a git worktree for the checkout dir is in, where Claude
// Code puts its own (.claude/worktrees/<name>), on a new branch named after
// it, and returns the folder that matches dir inside it.
func NewWorktree(dir, name string) (string, error) {
	return NewWorktreeOn(dir, name, "worktree-"+name, "")
}

// NewWorktreeOn is NewWorktree on a branch of your choosing, made from base
// (the main checkout's HEAD when it's "").
func NewWorktreeOn(dir, name, branch, base string) (string, error) {
	m, err := MakeWorktree(dir, name, branch, base)
	return m.Dir, err
}

// MakeWorktree makes a worktree named name on a new branch, always in the
// main checkout's .claude/worktrees, whichever worktree dir is in, and
// from base: the main checkout's HEAD when it's "", never the HEAD of the
// worktree dir happens to be in.
func MakeWorktree(dir, name, branch, base string) (Made, error) {
	main := MainRoot(dir)
	if main == "" {
		return Made{}, fmt.Errorf("%s isn't in a git repository", dir)
	}
	m := Made{Repo: main, Branch: branch, Path: filepath.Join(main, ".claude", "worktrees", name)}
	if _, err := os.Stat(m.Path); err == nil {
		return Made{}, fmt.Errorf("a worktree named %s already exists", name)
	}
	m.Target, _ = gitOut(main, "symbolic-ref", "--short", "-q", "HEAD")
	if base == "" {
		base = "HEAD"
	}
	sha, err := gitOut(main, "rev-parse", "--verify", "-q", base+"^{commit}")
	if err != nil || sha == "" {
		return Made{}, fmt.Errorf("%s has no commit %s to start a worktree from", filepath.Base(main), base)
	}
	m.Base = sha
	if out, err := exec.Command("git", "-C", main, "worktree", "add", "-b", branch, m.Path, sha).CombinedOutput(); err != nil {
		return Made{}, fmt.Errorf("git worktree add: %s", strings.TrimSpace(string(out)))
	}
	m.Dir = m.Path
	// The same folder inside it as dir is in its own checkout.
	if top := RepoRoot(dir); top != "" {
		real, err := filepath.EvalSymlinks(dir)
		if err != nil {
			real = dir
		}
		if rel, err := filepath.Rel(top, real); err == nil && rel != "." && !strings.HasPrefix(rel, "..") {
			if st, err := os.Stat(filepath.Join(m.Path, rel)); err == nil && st.IsDir() {
				m.Dir = filepath.Join(m.Path, rel)
			}
		}
	}
	return m, nil
}

// WorktreeAt is the linked worktree dir is in, as git says it: ok is false
// when dir is in a main checkout, or in none. Its Base is where its branch
// left the main checkout's: the commit they last had in common.
func WorktreeAt(dir string) (m Made, ok bool) {
	top, main := RepoRoot(dir), MainRoot(dir)
	if top == "" || main == "" || filepath.Clean(top) == filepath.Clean(main) {
		return Made{}, false
	}
	m = Made{Path: top, Dir: dir, Repo: main}
	m.Branch, _ = gitOut(top, "symbolic-ref", "--short", "-q", "HEAD")
	m.Target, _ = gitOut(main, "symbolic-ref", "--short", "-q", "HEAD")
	if m.Target != "" {
		m.Base, _ = gitOut(top, "merge-base", "HEAD", m.Target)
	}
	return m, true
}

// gitOut runs git in dir and gives its output, trimmed.
func gitOut(dir string, args ...string) (string, error) {
	out, err := exec.Command("git", append([]string{"-C", dir}, args...)...).Output()
	return strings.TrimSpace(string(out)), err
}
