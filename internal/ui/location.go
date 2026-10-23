package ui

import (
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// sessionLocation uses live cwd immediately, without borrowing git metadata
// from a different checkout while the fleet snapshot catches up.
func sessionLocation(a *fleet.Agent, c *hostConn) (cwd, repo, root, branch string) {
	cwd, repo, root, branch = a.Cwd, a.Repo, a.Root, a.Branch
	if c != nil && c.sess != nil && c.sess.Info.Cwd != "" {
		cwd = c.sess.Info.Cwd
	}
	if repo != "" && !locationWithin(repo, cwd) {
		repo, root, branch = "", "", ""
	}
	return
}

func locationWithin(root, path string) bool {
	if path == "" {
		return false
	}
	rel, err := filepath.Rel(root, path)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}

func locationBranch(repo, root, branch string) string {
	if repo != "" && root != "" && repo != root {
		tree := filepath.Base(repo)
		if tree == branch {
			return branch + " (worktree)"
		}
		if branch == "" {
			return "worktree " + tree
		}
		return branch + " · worktree " + tree
	}
	return branch
}

func locationSummary(a *fleet.Agent) string {
	cwd, repo, root, branch := sessionLocation(a, nil)
	if repo == "" {
		if host.IsTemp(cwd) {
			return "tmp/" + filepath.Base(cwd)
		}
		return tildify(cwd)
	}
	s := filepath.Base(firstNonEmpty(root, repo))
	if b := locationBranch(repo, root, branch); b != "" {
		s += " · " + b
	}
	if rel, err := filepath.Rel(repo, cwd); err == nil && rel != "." {
		s += " · ./" + rel
	}
	return s
}

func locationFolder(x *barCtx) string {
	cwd, repo, _, _ := sessionLocation(x.a, x.c)
	room := max(0, x.room)
	if repo == "" { // not a repository: the folder itself
		return shortDir(tildify(cwd), room)
	}
	// A repository by its project's name, and where in it when that's not
	// its top: a worktree's own folder is named for its branch, which the
	// branch beside it says already.
	name := filepath.Base(firstNonEmpty(x.a.Root, repo))
	rel, err := filepath.Rel(repo, cwd)
	if err != nil || rel == "." {
		return ansi.Truncate(name, room, "…")
	}
	return name + "/" + shortDir(rel, max(0, room-ansi.StringWidth(name)-1))
}
