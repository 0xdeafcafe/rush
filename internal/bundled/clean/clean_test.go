package clean

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/proc"
)

func git(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir, "-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"}, args...)...)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

// Only branches on the remote's default branch, checked out nowhere, are
// offered; and one that moved on since isn't deleted.
func TestMergedBranches(t *testing.T) {
	root := t.TempDir()
	origin, repo := filepath.Join(root, "origin.git"), filepath.Join(root, "repo")
	git(t, root, "init", "--bare", origin)
	git(t, root, "clone", "-q", origin, repo)
	git(t, repo, "commit", "-q", "--allow-empty", "-m", "one")
	git(t, repo, "push", "-q", "origin", "main")
	git(t, repo, "remote", "set-head", "origin", "main")
	git(t, repo, "branch", "done")
	git(t, repo, "branch", "later")
	git(t, repo, "branch", "held")
	git(t, repo, "worktree", "add", "-q", filepath.Join(root, "wt"), "held")
	git(t, repo, "checkout", "-q", "-b", "wip")
	git(t, repo, "commit", "-q", "--allow-empty", "-m", "unpushed")
	git(t, repo, "checkout", "-q", "main")

	ctx := context.Background()
	base, merged := mergedBranches(ctx, repo)
	if base != "origin/main" || !slices.Equal(merged, []string{"done", "later"}) {
		t.Fatalf("merged into %q: %q", base, merged)
	}
	// "later" gets work of its own after it was listed.
	git(t, repo, "checkout", "-q", "later")
	git(t, repo, "commit", "-q", "--allow-empty", "-m", "new work")
	git(t, repo, "checkout", "-q", "main")
	said, err := deleteBranches(repo, merged)(ctx)
	if err != nil || said != "Deleted 1 branch." {
		t.Fatalf("delete: %q %v", said, err)
	}
	out, _ := exec.Command("git", "-C", repo, "branch", "--format=%(refname:short)").Output()
	if got := strings.Fields(string(out)); !slices.Equal(got, []string{"held", "later", "main", "wip"}) {
		t.Fatalf("branches left: %q", got)
	}
}

// The list shows sizes, totals and what's running, each on its tab.
func TestPick(t *testing.T) {
	a := &app{k: &kit{name: "go-clean", procs: []string{"gopls"}}}
	things := []thing{
		{ID: "gocache", Title: "Go build cache", Path: "/c", Does: "runs go clean -cache", size: 3 << 30},
		{ID: "merged:r", Title: "r: branches merged", Meta: "2 branches", Does: "deletes them"},
	}
	run := []running{{Proc: &proc.Proc{PID: 7, Comm: "gopls", Footprint: 512 << 20, Start: time.Now()}, Cmd: "gopls serve"}}
	p := a.pick(things, time.Now(), run)
	if !slices.Equal(p.Tabs, []string{"On disk · 3.0 GB", "Running · 512 MB"}) {
		t.Fatalf("tabs: %q", p.Tabs)
	}
	if len(p.Items) != 3 || p.Items[1].Meta != "2 branches" || p.Items[2].Tab != 1 || !strings.HasPrefix(p.Items[2].ID, "pid:7:") {
		t.Fatalf("items: %+v", p.Items)
	}
	if p := a.pick(nil, time.Time{}, nil); p.Empty[0] != "Measuring…" {
		t.Fatalf("before measuring: %+v", p)
	}
}

// rust-clean finds cargo's caches where CARGO_HOME says, and leaves a
// folder named target alone unless it's a Cargo project's.
func TestRustThings(t *testing.T) {
	home := t.TempDir()
	for _, d := range []string{"registry/cache", "git"} {
		if err := os.MkdirAll(filepath.Join(home, d), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("CARGO_HOME", home)
	t.Setenv("SCCACHE_DIR", filepath.Join(home, "none"))
	t.Setenv("CARGO_TARGET_DIR", "")
	var ids []string
	for _, th := range rustKit.things(context.Background(), nil) {
		ids = append(ids, th.ID)
	}
	if !slices.Equal(ids, []string{"crates", "cargo-git"}) {
		t.Fatalf("things: %q", ids)
	}
}
