package host

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func gitIn(t *testing.T, dir string, args ...string) string {
	t.Helper()
	out, err := exec.Command("git", append([]string{"-C", dir}, args...)...).CombinedOutput()
	if err != nil {
		t.Fatalf("git %s: %v: %s", strings.Join(args, " "), err, out)
	}
	return strings.TrimSpace(string(out))
}

// A session asking for its own worktree gets one before its agent starts,
// from the main checkout, and records it: path, branch, base and target,
// and that rush made it. Its prompt tells the agent so.
func TestOwnWorktreeRecordedAtStart(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	main, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	gitIn(t, main, "init", "-q", "-b", "main")
	gitIn(t, main, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "first")
	head := gitIn(t, main, "rev-parse", "HEAD")
	s := &server{cfg: Config{ID: "abc123", Cwd: main, OwnWorktree: true}, clients: map[*conn]struct{}{}}
	if err := os.MkdirAll(dir(s.cfg.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	s.makeOwnWorktree()
	w := s.cfg.Workspace
	want := filepath.Join(main, ".claude", "worktrees", "abc123")
	if w == nil || w.Path != want || w.Branch != "rush/abc123" || w.Base != head || w.Target != "main" || w.Repo != main || !w.Own {
		t.Fatalf("workspace %+v", w)
	}
	if s.cfg.Cwd != want || s.info.Cwd != want || s.info.Workspace != w || s.cfg.OwnWorktree {
		t.Fatalf("cwd %q, info %q, asked again %v", s.cfg.Cwd, s.info.Cwd, s.cfg.OwnWorktree)
	}
	if !IsOwnWorktree(w.Path, w.Branch) {
		t.Fatal("not known for rush's own")
	}
	saved, err := ReadConfig(s.cfg.ID)
	if err != nil || saved.Workspace == nil || saved.Workspace.Branch != "rush/abc123" {
		t.Fatalf("saved %+v, %v", saved.Workspace, err)
	}
	if n := workspaceNote(w); !strings.Contains(n, want) || !strings.Contains(n, "rush/abc123") {
		t.Fatalf("note %q", n)
	}

	// One that went into a worktree by itself has it filled in from git,
	// not as rush's own.
	other := filepath.Join(main, ".claude", "worktrees", "agent-x")
	gitIn(t, main, "worktree", "add", "-q", "-b", "feature", other)
	s2 := &server{cfg: Config{ID: "def456", Cwd: other}, clients: map[*conn]struct{}{}}
	if err := os.MkdirAll(dir(s2.cfg.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	s2.noteWorkspace(other)
	if w := s2.info.Workspace; w == nil || w.Own || w.Branch != "feature" || w.Path != other || w.Target != "main" || w.Base != head {
		t.Fatalf("found %+v", w)
	}
	if workspaceNote(s2.cfg.Workspace) != "" {
		t.Fatal("told about a worktree it made itself")
	}

	// Its worktree removed, a later start goes back to the main checkout.
	gitIn(t, main, "worktree", "remove", "--force", want)
	s.mu.Lock()
	s.leaveGoneWorktree()
	s.mu.Unlock()
	if s.cfg.Cwd != main || s.cfg.Workspace != nil {
		t.Fatalf("after removal: %q %+v", s.cfg.Cwd, s.cfg.Workspace)
	}
}
