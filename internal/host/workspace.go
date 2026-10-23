package host

import (
	"os"
	"path/filepath"

	"github.com/0xdeafcafe/rush/internal/actions"
)

// Workspace is the git worktree a session works in, and its branch: one
// session, one worktree, one branch, and maybe one pull request, found by
// the branch. Nil while a session works in a main checkout, or in none.
type Workspace struct {
	Path   string `json:"path"`             // the worktree's top
	Repo   string `json:"repo"`             // the main checkout it belongs to
	Branch string `json:"branch,omitempty"` // "" when it's detached
	// Base is the commit the worktree started from, in full: work since
	// it is the session's own.
	Base string `json:"base,omitempty"`
	// Target is the branch the main checkout had checked out when the
	// session started: where its work is meant to land.
	Target string `json:"target,omitempty"`
	// Own is a worktree rush made for the session before its agent
	// started; one the agent went into by itself isn't, and rush never
	// removes it on its own.
	Own bool `json:"own,omitzero"`
}

// ownBranch is the branch rush makes a session's own worktree on, named
// for its id, as the worktree's folder is.
func ownBranch(id string) string { return "rush/" + id }

// IsOwnWorktree is whether a worktree at path on branch is one rush made
// for a session: <main>/.claude/worktrees/<id> on rush/<id>.
func IsOwnWorktree(path, branch string) bool {
	id := filepath.Base(path)
	return branch == ownBranch(id) && filepath.Base(filepath.Dir(path)) == "worktrees" &&
		filepath.Base(filepath.Dir(filepath.Dir(path))) == ".claude"
}

// workspaceOf is a worktree as a Workspace.
func workspaceOf(m actions.Made, own bool) *Workspace {
	return &Workspace{Path: m.Path, Repo: m.Repo, Branch: m.Branch, Base: m.Base, Target: m.Target, Own: own}
}

// workspaceNote is the line rush's prompt tells an agent in its own
// worktree, so it doesn't make another.
func workspaceNote(w *Workspace) string {
	if w == nil || !w.Own {
		return ""
	}
	return "You are already in your own git worktree " + w.Path + " on branch " + w.Branch + "; commit there; don't create another worktree."
}

// makeOwnWorktree gives a new session asking for one (Config.OwnWorktree)
// its own worktree, before its agent starts: from the main checkout's
// HEAD, at <main>/.claude/worktrees/<id> on rush/<id>. Outside a git
// repository, or when git won't, it stays where it is. Called without mu
// held, before anything else can reach the session.
func (s *server) makeOwnWorktree() {
	s.mu.Lock()
	cwd, id := s.cfg.Cwd, s.cfg.ID
	want := s.cfg.OwnWorktree && s.cfg.Workspace == nil
	if want {
		s.info.Detail = "making its own worktree"
		s.publish()
	}
	s.mu.Unlock()
	if !want {
		return
	}
	m, err := actions.MakeWorktree(cwd, id, ownBranch(id), "")
	s.mu.Lock()
	defer s.mu.Unlock()
	s.info.Detail = ""
	s.cfg.OwnWorktree = false // asked once: a restart doesn't make another
	if err != nil {
		s.event("worktree-not-made", map[string]any{"cwd": cwd, "err": err.Error()})
		s.saveConfig()
		s.publish()
		return
	}
	s.cfg.Workspace = workspaceOf(m, true)
	s.cfg.Cwd, s.info.Cwd, s.info.Workspace = m.Dir, m.Dir, s.cfg.Workspace
	s.event("worktree-made", map[string]any{"path": m.Path, "branch": m.Branch, "base": m.Base, "target": m.Target})
	s.saveConfig()
	s.publish()
}

// noteWorkspace fills in the workspace of a session that went into a
// worktree by itself (or started in one), from git, so it's known by its
// branch like one rush made. Runs git: never with mu held.
func (s *server) noteWorkspace(cwd string) {
	m, ok := actions.WorktreeAt(cwd)
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.cfg.Cwd != cwd {
		return // it moved again meanwhile
	}
	w := s.cfg.Workspace
	switch {
	case ok && (w == nil || w.Path != m.Path || w.Branch != m.Branch):
		own := w != nil && w.Own && w.Path == m.Path
		s.cfg.Workspace = workspaceOf(m, own)
		if own {
			s.cfg.Workspace.Base, s.cfg.Workspace.Target = w.Base, w.Target // as it started
		}
	case !ok && w != nil && !w.Own:
		s.cfg.Workspace = nil // back out of a worktree it had gone into
	default:
		return
	}
	s.info.Workspace = s.cfg.Workspace
	s.saveConfig()
	s.publish()
}

// leaveGoneWorktree moves a session whose worktree has been removed (its
// work landed) back to the main checkout, so a message to it later starts
// its agent somewhere that's there. Called with mu held.
func (s *server) leaveGoneWorktree() {
	w := s.cfg.Workspace
	if w == nil || w.Repo == "" {
		return
	}
	if _, err := os.Stat(s.cfg.Cwd); err == nil {
		return
	}
	s.event("worktree-gone", map[string]any{"path": w.Path, "to": w.Repo})
	s.cfg.Cwd, s.info.Cwd, s.cfg.Workspace, s.info.Workspace = w.Repo, w.Repo, nil, nil
	s.saveConfig()
}
