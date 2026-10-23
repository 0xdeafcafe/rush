package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

func TestLocationWorktreeAndNestedFolder(t *testing.T) {
	a := &fleet.Agent{Job: agent.Job{Cwd: "/work/fix/web"}, Repo: "/work/fix", Root: "/src/app", Branch: "feature"}
	if got := locationSummary(a); got != "app · feature · worktree fix · ./web" {
		t.Fatal(got)
	}
	if got := locationFolder(&barCtx{a: a, room: 80}); got != "app/web" {
		t.Fatal(got)
	}
	a.Branch = "fix"
	if got := locationSummary(a); got != "app · fix (worktree) · ./web" {
		t.Fatal(got)
	}
}

func TestLocationLiveCwdDoesNotBorrowOldBranch(t *testing.T) {
	a := &fleet.Agent{Job: agent.Job{Cwd: "/src/app"}, Repo: "/src/app", Root: "/src/app", Branch: "main"}
	c := &hostConn{sess: &convo.Session{Info: host.Info{Cwd: "/work/other"}}}
	cwd, repo, root, branch := sessionLocation(a, c)
	if cwd != "/work/other" || repo != "" || root != "" || branch != "" {
		t.Fatalf("%q %q %q %q", cwd, repo, root, branch)
	}
}

func TestLocationDeepPathKeepsFolderAndBranch(t *testing.T) {
	a := &fleet.Agent{Job: agent.Job{Cwd: "/a/very/long/working/directory/project/web"}, Repo: "/a/very/long/working/directory/project", Branch: "feature"}
	for _, width := range []int{24, 44, 80} {
		x := &barCtx{a: a, room: width}
		got := locationFolder(x)
		if !strings.HasSuffix(got, "/web") || ansi.StringWidth(got)+ansi.StringWidth(" · feature") > width {
			t.Fatalf("width %d: %q", width, got)
		}
	}
}
