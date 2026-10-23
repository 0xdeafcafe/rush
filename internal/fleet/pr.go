package fleet

import (
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/ghpr"
)

// PR is a session's pull request, found by its branch: its number, state
// (open, draft, merged or closed), checks (pass, fail or pending, and the
// first failing one's name), review (approved or changes) and URL. See
// ghpr.Status.
type PR = ghpr.Status

// PRBranch is the branch a session's pull request is found by: its
// Workspace's, or a linked worktree's it works in. "" for one in a main
// checkout, whose branch is everyone's.
func (a *Agent) PRBranch() string {
	if w := a.Workspace; w != nil {
		return w.Branch
	}
	if a.Repo != "" && a.Root != "" && a.Repo != a.Root {
		return a.Branch
	}
	return ""
}

// findPR sets PR from GitHub's pull requests for its repository, as last
// said (never waiting on GitHub), in place of Claude Code's links where
// both have one; Claude Code's stay where gh can't say.
func (a *Agent) findPR() {
	a.PR = nil
	if b := a.PRBranch(); b != "" && a.Root != "" {
		if p, ok := ghpr.For(a.Root, b); ok {
			a.PR = &p
			a.PRs = []agent.PR{asLinked(p)}
			return
		}
	}
	if len(a.PRs) > 0 {
		p := fromLinked(a.PRs[0])
		a.PR = &p
	}
}

// asLinked is a pull request as the list's older links have it.
func asLinked(p PR) agent.PR {
	l := agent.PR{URL: p.URL, Title: p.Title, Number: p.Number, State: strings.ToUpper(p.State)}
	switch p.Review {
	case "approved":
		l.Review = "APPROVED"
	case "changes":
		l.Review = "CHANGES_REQUESTED"
	}
	switch p.Checks {
	case "pass":
		l.Checks.Passed = 1
	case "fail":
		l.Checks.Failed = 1
	case "pending":
		l.Checks.Pending = 1
	}
	return l
}

// fromLinked is a pull request Claude Code linked to a session, as a PR.
func fromLinked(l agent.PR) PR {
	p := PR{Number: l.Number, Title: l.Title, URL: l.URL, State: strings.ToLower(l.State)}
	switch strings.ToUpper(l.Review) {
	case "APPROVED":
		p.Review = "approved"
	case "CHANGES_REQUESTED":
		p.Review = "changes"
	}
	switch c := l.Checks; {
	case c.Failed > 0:
		p.Checks = "fail"
	case c.Pending > 0:
		p.Checks = "pending"
	case c.Passed > 0:
		p.Checks = "pass"
	}
	return p
}
