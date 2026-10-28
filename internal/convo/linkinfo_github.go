package convo

import (
	"fmt"
	"time"

	"github.com/0xdeafcafe/rush/internal/ghpr"
)

// The kinds of GitHub link rush knows: pull requests and issues, asked
// through gh.
const (
	LinkGitHubPR    = "github-pr"
	LinkGitHubIssue = "github-issue"
)

func init() {
	RegisterLink(LinkKind[ghpr.PR]{
		Name: LinkGitHubPR,
		Key: func(u string) string {
			if repo, n, ok := ghpr.ParseURL(u); ok {
				return ghpr.PRURL(repo, n)
			}
			return ""
		},
		Look:  ghpr.View,
		Badge: prBadge,
		// An open pull request's checks and review move; a merged or
		// closed one stays.
		Fresh: func(p *ghpr.PR) time.Duration {
			if p.State == "OPEN" {
				return 2 * time.Minute
			}
			return 0
		},
	})
	RegisterLink(LinkKind[ghpr.Issue]{
		Name: LinkGitHubIssue,
		Key: func(u string) string {
			if repo, n, ok := ghpr.ParseIssueURL(u); ok {
				return ghpr.IssueURL(repo, n)
			}
			return ""
		},
		Look:  ghpr.ViewIssue,
		Badge: issueBadge,
		Fresh: func(*ghpr.Issue) time.Duration { return 10 * time.Minute },
	})
}

// prBadge is where a pull request stands, in a few coloured words:
// merged, closed, a draft, or open with its checks and review.
func prBadge(p *ghpr.PR) string {
	switch p.State {
	case "MERGED":
		return paint(cBlue, "⇡ merged")
	case "CLOSED":
		return dim("⇡ closed")
	}
	s := paint(cGreen, "⇡ open")
	if p.Draft {
		s = dim("⇡ draft")
	}
	ok, failed, pending := p.Counts()
	switch {
	case failed > 0:
		s += faint(" · ") + paint(cRed, fmt.Sprintf("✗ %d failing", failed))
	case pending > 0:
		s += faint(" · ") + sub(fmt.Sprintf("%d running", pending))
	case ok > 0:
		s += faint(" · ") + paint(cGreen, "✓ checks")
	}
	switch p.Decision {
	case "APPROVED":
		s += faint(" · ") + paint(cGreen, "approved")
	case "CHANGES_REQUESTED":
		s += faint(" · ") + paint(cRed, "changes asked")
	case "REVIEW_REQUIRED":
		s += faint(" · ") + paint(cYellow, "needs review")
	}
	return s
}

// issueBadge is whether an issue is open or closed, and how it closed.
func issueBadge(i *ghpr.Issue) string {
	if i.State != "CLOSED" {
		return paint(cGreen, "◎ open")
	}
	switch i.Reason {
	case "NOT_PLANNED":
		return dim("◎ not planned")
	case "DUPLICATE":
		return dim("◎ duplicate")
	}
	return paint(cBlue, "◎ closed")
}
