package fleet

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func TestHostedLocationIgnoresStaleTranscriptCwd(t *testing.T) {
	root := t.TempDir()
	current, old := filepath.Join(root, "current"), filepath.Join(root, "old")
	for _, dir := range []string{current, old} {
		if err := os.MkdirAll(filepath.Join(dir, ".git"), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, ".git", "HEAD"), []byte("ref: refs/heads/"+filepath.Base(dir)), 0644); err != nil {
			t.Fatal(err)
		}
	}
	l := NewLoader(&state.Store{})
	p := agent.Profile{Name: "test"}
	info := host.Info{ID: "test", Cwd: current, Kind: "test-harness"}
	l.spend[state.Key(p.Name, "a:"+info.ID)] = Spend{Dir: old}
	a := l.hostedAgent(p, info, nil, time.Now())
	if a.Cwd != current || a.Repo != current || a.Branch != "current" {
		t.Fatalf("cwd=%q repo=%q branch=%q", a.Cwd, a.Repo, a.Branch)
	}
}

func TestHostedLocationFollowsNewerTranscriptCd(t *testing.T) {
	root := t.TempDir()
	started, moved := filepath.Join(root, "started"), filepath.Join(root, "moved")
	for _, dir := range []string{started, moved} {
		if err := os.MkdirAll(filepath.Join(dir, ".git"), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, ".git", "HEAD"), []byte("ref: refs/heads/"+filepath.Base(dir)), 0644); err != nil {
			t.Fatal(err)
		}
	}
	now := time.Now()
	l := NewLoader(&state.Store{})
	p := agent.Profile{Name: "test"}
	// Claude Code's cwd stays where it started; the transcript saw a later cd.
	info := host.Info{ID: "test", Cwd: started, CwdAt: now.Add(-time.Hour), Kind: "test-harness"}
	l.spend[state.Key(p.Name, "a:"+info.ID)] = Spend{Dir: moved, DirAt: now}
	a := l.hostedAgent(p, info, nil, now)
	if a.Cwd != moved || a.Branch != "moved" {
		t.Fatalf("cwd=%q branch=%q", a.Cwd, a.Branch)
	}
}

func TestJobCwdUsesWorktreeWithoutLosingSubdirectory(t *testing.T) {
	for _, tt := range []struct{ cwd, want string }{{"/src/main", "/work/fix"}, {"/work/fix/web", "/work/fix/web"}, {"/work/fix-other", "/work/fix"}} {
		if got := jobCwd(agent.Job{Cwd: tt.cwd, WorktreePath: "/work/fix"}); got != tt.want {
			t.Fatalf("%q: %q", tt.cwd, got)
		}
	}
}

func TestMainCheckoutDoesNotCacheMissingRepository(t *testing.T) {
	root := t.TempDir()
	seen := map[string]string{}
	if got := mainCheckout(root, seen); got != "" {
		t.Fatal(got)
	}
	if err := os.Mkdir(filepath.Join(root, ".git"), 0755); err != nil {
		t.Fatal(err)
	}
	if got := mainCheckout(root, seen); got != root {
		t.Fatalf("after init: %q", got)
	}
}
