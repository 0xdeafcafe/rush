package fleet

import (
	"context"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/ghpr"
	"github.com/0xdeafcafe/rush/internal/host"
)

// fakeGH has gh say root's pull requests are prs (gh pr list's JSON), and
// waits for it to be taken in.
func fakeGH(t *testing.T, root, prs string) {
	t.Helper()
	was := ghpr.RunGH
	t.Cleanup(func() { ghpr.RunGH = was })
	ghpr.RunGH = func(context.Context, string, ...string) ([]byte, error) { return []byte(prs), nil }
	if _, err := ghpr.LookBranches(context.Background(), root); err != nil {
		t.Fatal(err)
	}
}

// A session's pull request is the one GitHub has for its workspace's
// branch, whichever agent runs it, in place of Claude Code's links.
func TestPRMatchedByBranch(t *testing.T) {
	root := t.TempDir()
	fakeGH(t, root, `[{"number":7,"title":"t","state":"OPEN","headRefName":"rush/abc","headRefOid":"x","isDraft":false,"statusCheckRollup":[],"reviewDecision":"APPROVED","url":"u7"}]`)
	a := &Agent{Kind: "codex", Root: root, Repo: root + "/.claude/worktrees/abc", Workspace: &host.Workspace{Branch: "rush/abc"}}
	a.PRs = []agent.PR{{Number: 3, State: "OPEN"}}
	a.findPR()
	if a.PR == nil || a.PR.Number != 7 || a.PR.Review != "approved" || len(a.PRs) != 1 || a.PRs[0].Number != 7 {
		t.Fatalf("PR %+v, PRs %+v", a.PR, a.PRs)
	}
	// No workspace, but in a linked worktree: its branch.
	b := &Agent{Root: root, Repo: root + "/elsewhere", Job: agent.Job{}, Branch: "rush/abc"}
	b.findPR()
	if b.PR == nil || b.PR.Number != 7 {
		t.Fatalf("linked worktree: %+v", b.PR)
	}
	// In the main checkout its branch is everyone's: Claude Code's link stands.
	c := &Agent{Root: root, Repo: root, Branch: "rush/abc", PRs: []agent.PR{{Number: 3, State: "MERGED", Review: "APPROVED"}}}
	c.findPR()
	if c.PR == nil || c.PR.Number != 3 || c.PR.State != "merged" || c.PR.Review != "approved" {
		t.Fatalf("main checkout: %+v", c.PR)
	}
}
