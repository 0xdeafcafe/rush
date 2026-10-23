package ghpr

import (
	"context"
	"errors"
	"os/exec"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Comment is something said on a pull request's conversation.
type Comment struct {
	Who, Body string
	At        time.Time
}

// Comments are a pull request's conversation, oldest first.
func Comments(ctx context.Context, root string, n int) ([]Comment, error) {
	cmd := exec.CommandContext(ctx, "gh", "pr", "view", strconv.Itoa(n), "--json", "comments")
	cmd.Dir = root
	out, err := cmd.Output()
	if err != nil {
		return nil, ghErr(err)
	}
	return parseComments(out)
}

func parseComments(b []byte) ([]Comment, error) {
	var raw struct {
		Comments []struct {
			Author    who       `json:"author"`
			Body      string    `json:"body"`
			CreatedAt time.Time `json:"createdAt"`
		} `json:"comments"`
	}
	if err := jsonx.Unmarshal(b, &raw); err != nil {
		return nil, err
	}
	out := make([]Comment, 0, len(raw.Comments))
	for _, c := range raw.Comments {
		out = append(out, Comment{Who: c.Author.String(), Body: c.Body, At: c.CreatedAt})
	}
	return out, nil
}

// Approve approves pull request n, saying body with it when there is one.
func Approve(ctx context.Context, root string, n int, body string) error {
	args := []string{"pr", "review", strconv.Itoa(n), "--approve"}
	if body != "" {
		args = append(args, "--body", body)
	}
	return run(ctx, root, args...)
}

// RequestChanges asks for changes to pull request n, saying why.
func RequestChanges(ctx context.Context, root string, n int, body string) error {
	return run(ctx, root, "pr", "review", strconv.Itoa(n), "--request-changes", "--body", body)
}

// Say comments body on pull request n.
func Say(ctx context.Context, root string, n int, body string) error {
	return run(ctx, root, "pr", "comment", strconv.Itoa(n), "--body", body)
}

// Merge merges pull request n by how: squash, merge or rebase; admin
// merges past its base branch's rules, as gh pr merge --admin does.
func Merge(ctx context.Context, root string, n int, how string, admin bool) error {
	args := []string{"pr", "merge", strconv.Itoa(n), "--" + how}
	if admin {
		args = append(args, "--admin")
	}
	cmd := exec.CommandContext(ctx, "gh", args...)
	cmd.Dir = root
	_, err := cmd.Output()
	if ee, ok := errors.AsType[*exec.ExitError](err); ok {
		return mergeErr(string(ee.Stderr), err)
	}
	return ghErr(err)
}

// A MergeError is GitHub refusing a merge, said plainly. Admin is whether
// merging as an admin gets past it: the base branch's rules stand in its
// way, a review or checks it wants.
type MergeError struct {
	Why   string
	Admin bool
}

func (e *MergeError) Error() string { return e.Why }

// mergeErr reads what gh pr merge said when it failed, all of it: gh puts
// the way past on the lines after the first.
func mergeErr(stderr string, err error) error {
	low := strings.ToLower(stderr)
	switch {
	case strings.Contains(low, "rate limit"):
		return &MergeError{Why: "GitHub's rate limit is used up: try again once it resets"}
	case strings.Contains(low, "--admin") || strings.Contains(low, "base branch policy"):
		return &MergeError{Why: "the base branch's rules block it (a review or checks): an admin can merge past them", Admin: true}
	case strings.Contains(low, "merge conflict") || strings.Contains(low, "not mergeable") && strings.Contains(low, "conflict"):
		return &MergeError{Why: "it conflicts with its base: bring the base in first"}
	}
	return ghErr(err)
}

func run(ctx context.Context, root string, args ...string) error {
	cmd := exec.CommandContext(ctx, "gh", args...)
	cmd.Dir = root
	_, err := cmd.Output()
	return ghErr(err)
}
