package ghpr

import "testing"

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
