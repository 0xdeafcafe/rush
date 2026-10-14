package ghpr

import (
	"context"
	"os/exec"
	"strconv"
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

// Merge merges pull request n by how: squash, merge or rebase.
func Merge(ctx context.Context, root string, n int, how string) error {
	return run(ctx, root, "pr", "merge", strconv.Itoa(n), "--"+how)
}

func run(ctx context.Context, root string, args ...string) error {
	cmd := exec.CommandContext(ctx, "gh", args...)
	cmd.Dir = root
	_, err := cmd.Output()
	return ghErr(err)
}
