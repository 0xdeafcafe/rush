package ghpr

import (
	"errors"
	"strings"
	"testing"
	"time"
)

func TestParse(t *testing.T) {
	b := []byte(`[{"number":7,"title":"t","state":"OPEN","author":{"login":"me"},"reviewDecision":"REVIEW_REQUIRED",
	"reviewRequests":[{"__typename":"User","login":"sam"},{"__typename":"Team","name":"Web","slug":"web-team"}],
	"latestReviews":[{"author":{"login":"kim"},"state":"APPROVED","body":"lgtm"}],
	"statusCheckRollup":[
	 {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"},
	 {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"FAILURE"},
	 {"__typename":"CheckRun","name":"e2e","status":"IN_PROGRESS","conclusion":""},
	 {"__typename":"CheckRun","name":"skip","status":"COMPLETED","conclusion":"SKIPPED"},
	 {"__typename":"StatusContext","context":"ci/old","state":"ERROR"}]}]`)
	prs, err := Parse(b)
	if err != nil || len(prs) != 1 {
		t.Fatal(prs, err)
	}
	p := prs[0]
	if ok, bad, wait := p.Counts(); ok != 2 || bad != 2 || wait != 1 {
		t.Errorf("counts = %d %d %d", ok, bad, wait)
	}
	if len(p.Waiting) != 2 || p.Waiting[0] != "sam" || p.Waiting[1] != "web-team" {
		t.Errorf("waiting = %v", p.Waiting)
	}
	if len(p.Reviews) != 1 || p.Reviews[0].Who != "kim" || p.Reviews[0].Body != "lgtm" {
		t.Errorf("reviews = %v", p.Reviews)
	}
}

func TestParseComments(t *testing.T) {
	cs, err := parseComments([]byte(`{"comments":[{"author":{"login":"kim"},"body":"nice","createdAt":"2026-10-14T10:00:00Z"}]}`))
	if err != nil || len(cs) != 1 || cs[0].Who != "kim" || cs[0].Body != "nice" || cs[0].At.IsZero() {
		t.Fatal(cs, err)
	}
}

// A refused merge says why in plain words, all of gh's lines read: the
// way past its base branch's rules is on the last.
func TestMergeErr(t *testing.T) {
	policy := "X Pull request o/r#9 is not mergeable: the base branch policy prohibits the merge.\nTo have the pull request merged after all the requirements have been met, add the `--auto` flag.\nTo use administrator privileges to immediately merge the pull request, add the `--admin` flag.\n"
	var me *MergeError
	if !errors.As(mergeErr(policy, errors.New("exit 1")), &me) || !me.Admin {
		t.Fatalf("policy: %v", me)
	}
	if !errors.As(mergeErr("GraphQL: API rate limit exceeded for user ID 1.", errors.New("exit 1")), &me) || me.Admin || !strings.Contains(me.Why, "rate limit") {
		t.Fatalf("rate limit: %v", me)
	}
}

// GitHub failing is asked again later each time in a row, and not for ten
// minutes once it says it's asked too much.
func TestRetryIn(t *testing.T) {
	for _, c := range []struct {
		fails int
		err   string
		want  time.Duration
	}{
		{0, "", BranchEvery},
		{1, "HTTP 504", 2 * BranchEvery},
		{3, "HTTP 502", 8 * BranchEvery},
		{9, "HTTP 504", 15 * time.Minute},
		{1, "HTTP 403: You have exceeded a secondary rate limit.", 10 * time.Minute},
	} {
		if got := retryIn(c.fails, c.err); got != c.want {
			t.Errorf("retryIn(%d, %q) = %v, want %v", c.fails, c.err, got, c.want)
		}
	}
}
