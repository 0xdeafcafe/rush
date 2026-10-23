package fleet

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

func run(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

// A worktree goes only when every change in it is committed, and pushed or
// in main.
func TestWorktreeSafety(t *testing.T) {
	root, _ := filepath.EvalSymlinks(t.TempDir())
	remote, repo := filepath.Join(root, "remote.git"), filepath.Join(root, "repo")
	_ = os.MkdirAll(repo, 0o755)
	run(t, root, "init", "-q", "--bare", remote)
	run(t, repo, "init", "-q", "-b", "main")
	_ = os.WriteFile(filepath.Join(repo, "a.txt"), []byte("a\n"), 0o644)
	run(t, repo, "add", ".")
	run(t, repo, "commit", "-qm", "first")
	run(t, repo, "remote", "add", "origin", remote)
	run(t, repo, "push", "-q", "origin", "main")
	wt := filepath.Join(repo, ".claude", "worktrees", "feature")
	run(t, repo, "worktree", "add", "-q", "-b", "feature", wt)

	agent := &Agent{Key: "k"}
	agent.Spend.Dirs = []string{filepath.Join(wt, "sub")}
	agent.Cwd = repo
	find := func() Worktree {
		t.Helper()
		ws := FindWorktrees([]*Agent{agent})
		if len(ws) != 1 || ws[0].Path != wt || len(ws[0].Agents) != 1 || !ws[0].Claude || ws[0].Branch != "feature" {
			t.Fatalf("found %+v", ws)
		}
		ws[0].Check()
		return ws[0]
	}

	// A new file: uncommitted.
	_ = os.WriteFile(filepath.Join(wt, "b.txt"), []byte("b\n"), 0o644)
	if w := find(); w.Safe() || w.Changed != 1 {
		t.Fatalf("uncommitted: %+v", w)
	}
	// Committed, not pushed.
	run(t, wt, "add", ".")
	run(t, wt, "commit", "-qm", "b")
	w := find()
	if w.Safe() || w.Unpushed != 1 || w.Losses() != "1 commit not pushed or in main" {
		t.Fatalf("unpushed: %+v %q", w, w.Losses())
	}
	if err := RemoveWorktree(w, false); err == nil {
		t.Fatal("removed a worktree with an unpushed commit")
	}
	// Cherry-picked into main, not pushed: main keeps it, so it's safe.
	run(t, repo, "cherry-pick", "feature")
	if w = find(); !w.Safe() || w.Pushed() {
		t.Fatalf("in main: %+v %q", w, w.Losses())
	}
	// The one rule: its work is in main, so it can go, by itself too.
	if v := w.Verdict(); !v.CanGo() {
		t.Fatalf("in local main: %+v", v)
	}
	run(t, repo, "reset", "-q", "--hard", "HEAD~1")
	// Pushed, and an ignored build folder: safe, and it goes.
	run(t, wt, "push", "-q", "origin", "feature")
	_ = os.WriteFile(filepath.Join(wt, ".gitignore"), []byte("dist/\n.gitignore\n"), 0o644)
	_ = os.MkdirAll(filepath.Join(wt, "dist"), 0o755)
	_ = os.WriteFile(filepath.Join(wt, "dist", "x.js"), []byte("x"), 0o644)
	if w = find(); !w.Safe() || w.Size == 0 {
		t.Fatalf("pushed: %+v", w)
	}
	if err := RemoveWorktree(w, false); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(wt); !os.IsNotExist(err) {
		t.Fatal("the worktree is still there")
	}
	if ws := FindWorktrees([]*Agent{agent}); len(ws) != 0 {
		t.Fatalf("git still lists it: %+v", ws)
	}
	run(t, repo, "rev-parse", "--verify", "-q", "feature") // the branch stays
}

// Forcing removes one that would lose work, after the confirmation.
func TestWorktreeForce(t *testing.T) {
	root, _ := filepath.EvalSymlinks(t.TempDir())
	repo := filepath.Join(root, "repo")
	_ = os.MkdirAll(repo, 0o755)
	run(t, repo, "init", "-q", "-b", "main")
	_ = os.WriteFile(filepath.Join(repo, "a.txt"), []byte("a\n"), 0o644)
	run(t, repo, "add", ".")
	run(t, repo, "commit", "-qm", "first")
	wt := filepath.Join(root, "wt")
	run(t, repo, "worktree", "add", "-q", "-b", "x", wt)
	_ = os.WriteFile(filepath.Join(wt, "dirty.txt"), []byte("d\n"), 0o644)
	ws := FindWorktrees([]*Agent{{Key: "k", Job: jobIn(wt)}})
	if len(ws) != 1 {
		t.Fatalf("found %+v", ws)
	}
	ws[0].Check()
	if ws[0].Safe() || !ws[0].NoRemote {
		t.Fatalf("uncommitted with no remote isn't safe: %+v", ws[0])
	}
	// Never the repository itself, nor a folder holding it, even forced.
	for _, bad := range []Worktree{{Path: repo, Repo: repo}, {Path: root, Repo: repo}, {Path: wt, Repo: root}} {
		if err := RemoveWorktree(bad, true); err == nil {
			t.Fatalf("removed %s", bad.Path)
		}
	}
	if _, err := os.Stat(filepath.Join(repo, "a.txt")); err != nil {
		t.Fatal("the repository was touched")
	}
	if err := RemoveWorktree(ws[0], true); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(wt); !os.IsNotExist(err) {
		t.Fatal("still there")
	}
}

func jobIn(dir string) agent.Job { return agent.Job{Cwd: dir} }

// A checkout something hollowed out, its tracked files gone from disk and
// nothing staged, loses nothing: the tidy-up removes it. An edit beside
// the deletions still keeps it.
func TestWorktreeDeletedFiles(t *testing.T) {
	root, _ := filepath.EvalSymlinks(t.TempDir())
	remote, repo := filepath.Join(root, "remote.git"), filepath.Join(root, "repo")
	_ = os.MkdirAll(repo, 0o755)
	run(t, root, "init", "-q", "--bare", remote)
	run(t, repo, "init", "-q", "-b", "main")
	for _, f := range []string{"a.txt", "b.txt", "c.txt"} {
		_ = os.WriteFile(filepath.Join(repo, f), []byte(f+"\n"), 0o644)
	}
	run(t, repo, "add", ".")
	run(t, repo, "commit", "-qm", "first")
	run(t, repo, "remote", "add", "origin", remote)
	run(t, repo, "push", "-q", "origin", "main")
	wt := filepath.Join(root, "wt")
	run(t, repo, "worktree", "add", "-q", "-b", "hollow", wt)
	run(t, wt, "push", "-q", "origin", "hollow")
	find := func() Worktree {
		t.Helper()
		ws := FindWorktrees([]*Agent{{Key: "k", Job: jobIn(wt)}})
		if len(ws) != 1 {
			t.Fatalf("found %+v", ws)
		}
		ws[0].Check()
		return ws[0]
	}

	_ = os.Remove(filepath.Join(wt, "a.txt"))
	_ = os.Remove(filepath.Join(wt, "b.txt"))
	_ = os.WriteFile(filepath.Join(wt, "c.txt"), []byte("edited\n"), 0o644)
	if w := find(); w.Safe() || w.Changed != 1 || w.Deleted != 2 {
		t.Fatalf("an edit beside deletions: %+v", w)
	}

	run(t, wt, "checkout", "-q", "--", "c.txt")
	w := find()
	if !w.Pushed() || w.Changed != 0 || w.Deleted != 2 {
		t.Fatalf("only deletions: %+v %q", w, w.Losses())
	}
	if _, err := TidyWorktree(w); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(wt); !os.IsNotExist(err) {
		t.Fatal("the worktree is still there")
	}
	run(t, repo, "rev-parse", "--verify", "-q", "hollow") // the branch stays
}
