package codex

import (
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A reading keeps the account's earned resets: how many, and each one
// still to be used, as the backend describes it.
func TestReadsResets(t *testing.T) {
	var r rateLimitsResponse
	err := jsonx.Unmarshal([]byte(`{"rateLimits":{"limitId":"codex","primary":{"usedPercent":100}},
		"rateLimitResetCredits":{"availableCount":2,"credits":[
			{"id":"c1","title":"Limit reset","description":"Resets your 5h and weekly limits","status":"available","grantedAt":1790000000,"expiresAt":1800000000,"resetType":"codexRateLimits"},
			{"id":"c0","status":"redeemed","grantedAt":1780000000,"resetType":"codexRateLimits"}]}}`), &r)
	if err != nil {
		t.Fatal(err)
	}
	q := quotaFromResponse(r)
	if q.Resets == nil || q.Resets.Available != 2 || len(q.Resets.Credits) != 1 {
		t.Fatalf("resets: %+v", q.Resets)
	}
	if c := q.Resets.Credits[0]; c.ID != "c1" || c.About == "" || c.Expires.IsZero() {
		t.Errorf("credit: %+v", c)
	}
}
