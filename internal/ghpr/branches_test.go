package ghpr

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"
)

const fakeList = `[
 {"number":12,"title":"old try","state":"CLOSED","headRefName":"rush/abc","headRefOid":"aaa","isDraft":false,"statusCheckRollup":[],"reviewDecision":"","mergedAt":null,"url":"u12"},
 {"number":14,"title":"the fix","state":"MERGED","headRefName":"rush/abc","headRefOid":"bbb","isDraft":false,
  "statusCheckRollup":[{"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"SUCCESS"}],
  "reviewDecision":"APPROVED","mergedAt":"2026-10-20T10:00:00Z","url":"u14"},
 {"number":15,"title":"wip","state":"OPEN","headRefName":"rush/def","headRefOid":"ccc","isDraft":true,
  "statusCheckRollup":[{"__typename":"CheckRun","name":"build","status":"IN_PROGRESS"},{"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE"}],
  "reviewDecision":"CHANGES_REQUESTED","mergedAt":null,"url":"u15"}
]`

// A session's pull request is found by its branch, through one gh pr list
// for the repository, asked in the background and kept.
func TestByBranchWithAFakeGH(t *testing.T) {
	Forget()
	calls := 0
	was := RunGH
	t.Cleanup(func() { RunGH = was; Forget() })
	RunGH = func(_ context.Context, dir string, args ...string) ([]byte, error) {
		calls++
		if dir != "/repo" || !strings.Contains(strings.Join(args, " "), "pr list --state all --limit 100") {
			t.Errorf("asked %s %v", dir, args)
		}
		return []byte(fakeList), nil
	}
	if _, ok := ByBranch("/repo"); ok {
		t.Fatal("known before GitHub answered")
	}
	var by map[string]Status
	for end := time.Now().Add(5 * time.Second); time.Now().Before(end); time.Sleep(5 * time.Millisecond) {
		var ok bool
		if by, ok = ByBranch("/repo"); ok {
			break
		}
	}
	merged, ok := by["rush/abc"]
	if !ok || merged.Number != 14 || !merged.Merged() || merged.Review != "approved" || merged.Checks != "pass" || merged.MergedAt.IsZero() || merged.HeadOid != "bbb" {
		t.Fatalf("rush/abc: %+v (the merged one beats the closed one)", merged)
	}
	draft, _ := For("/repo", "rush/def")
	if draft.State != "draft" || draft.Checks != "fail" || draft.Failing != "test" || draft.Review != "changes" || draft.URL != "u15" {
		t.Fatalf("rush/def: %+v", draft)
	}
	if _, ok := For("/repo", "nope"); ok {
		t.Fatal("a branch with no pull request has one")
	}
	if calls != 1 {
		t.Fatalf("asked %d times within a minute", calls)
	}
}

// gh missing says so, and isn't taken for "no pull requests".
func TestByBranchWithoutGH(t *testing.T) {
	Forget()
	was := RunGH
	t.Cleanup(func() { RunGH = was; Forget() })
	RunGH = func(context.Context, string, ...string) ([]byte, error) { return nil, errors.New("exec: not found") }
	if _, err := LookBranches(context.Background(), "/repo"); err == nil {
		t.Fatal("no error without gh")
	}
	if _, ok := ByBranch("/repo"); ok {
		t.Fatal("ok without gh")
	}
}
