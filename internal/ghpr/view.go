package ghpr

import (
	"context"
	"net/url"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// ParseURL is the repository, owner/name, and number of a pull request's
// address on GitHub: https://github.com/o/r/pull/7, with or without a tab
// (/files, /checks) or an anchor after it. ok is false for anything else,
// an issue, a commit or another site.
func ParseURL(u string) (repo string, n int, ok bool) { return parseNumbered(u, "pull") }

// ParseIssueURL is ParseURL for an issue's address: .../issues/7.
func ParseIssueURL(u string) (repo string, n int, ok bool) { return parseNumbered(u, "issues") }

// parseNumbered reads github.com/<owner>/<name>/<kind>/<number>[/...].
func parseNumbered(u, kind string) (repo string, n int, ok bool) {
	p, err := url.Parse(u)
	if err != nil || (p.Scheme != "https" && p.Scheme != "http") || !strings.EqualFold(p.Host, "github.com") && !strings.EqualFold(p.Host, "www.github.com") {
		return "", 0, false
	}
	parts := strings.Split(strings.Trim(p.Path, "/"), "/")
	if len(parts) < 4 || parts[2] != kind || parts[0] == "" || parts[1] == "" {
		return "", 0, false
	}
	n, err = strconv.Atoi(parts[3])
	if err != nil || n <= 0 {
		return "", 0, false
	}
	return parts[0] + "/" + parts[1], n, true
}

// PRURL is the canonical address of pull request n in repo, owner/name.
func PRURL(repo string, n int) string {
	return "https://github.com/" + repo + "/pull/" + strconv.Itoa(n)
}

// IssueURL is the canonical address of issue n in repo, owner/name.
func IssueURL(repo string, n int) string {
	return "https://github.com/" + repo + "/issues/" + strconv.Itoa(n)
}

// View asks GitHub for one pull request by its address, from any folder,
// through the same gate as Look so a page of links never asks too many at
// once.
func View(ctx context.Context, u string) (PR, error) {
	select {
	case branches.gate <- struct{}{}:
	case <-ctx.Done():
		return PR{}, ctx.Err()
	}
	defer func() { <-branches.gate }()
	out, err := RunGH(ctx, "", "pr", "view", u, "--json", fields)
	if err != nil {
		return PR{}, ghErr(err)
	}
	return ParseOne(out)
}

// ParseOne reads gh pr view's JSON.
func ParseOne(b []byte) (PR, error) {
	var raw rawPR
	if err := jsonx.Unmarshal(b, &raw); err != nil {
		return PR{}, err
	}
	return fromRaw(&raw), nil
}

// Issue is an issue as GitHub says it.
type Issue struct {
	Number     int
	Title, URL string
	State      string // OPEN or CLOSED
	// Reason is why it's closed: COMPLETED, NOT_PLANNED or DUPLICATE.
	Reason string
}

// ViewIssue asks GitHub for one issue by its address, from any folder,
// through the same gate as View.
func ViewIssue(ctx context.Context, u string) (Issue, error) {
	select {
	case branches.gate <- struct{}{}:
	case <-ctx.Done():
		return Issue{}, ctx.Err()
	}
	defer func() { <-branches.gate }()
	out, err := RunGH(ctx, "", "issue", "view", u, "--json", "number,title,url,state,stateReason")
	if err != nil {
		return Issue{}, ghErr(err)
	}
	var raw struct {
		Number      int    `json:"number"`
		Title       string `json:"title"`
		URL         string `json:"url"`
		State       string `json:"state"`
		StateReason string `json:"stateReason"`
	}
	if err := jsonx.Unmarshal(out, &raw); err != nil {
		return Issue{}, err
	}
	return Issue{Number: raw.Number, Title: raw.Title, URL: raw.URL, State: raw.State, Reason: raw.StateReason}, nil
}
