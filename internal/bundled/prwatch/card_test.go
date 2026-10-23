package prwatch

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/watch"
)

func text(ss []convo.Seg) string {
	var b strings.Builder
	for _, s := range ss {
		b.WriteString(s.Text)
	}
	return b.String()
}

func TestTheCardSaysAFailureTheWatchHasntReported(t *testing.T) {
	now := time.Unix(10_000, 0)
	l := watch.Live{
		Number: 8982, Title: "fix(security): close the alerts", Until: watch.UntilDone, Timeout: 110 * time.Minute,
		Start: now.Add(-66 * time.Minute), Asked: now.Add(-20 * time.Second), Next: now.Add(40 * time.Second),
		Passed: 41, Failed: 1, Pending: 2, FailedChecks: []string{"test"}, Running: []string{"codeql-complete", "sdk-python-complete"},
	}
	c := card(l, now)
	body := text(c.Body)
	for _, want := range []string{"#8982", "✓41", "✗1", "◌2", "test failed", "says so once 2 more finish"} {
		if !strings.Contains(body, want) {
			t.Errorf("body lacks %q: %s", want, body)
		}
	}
	if c.Tone != convo.ToneBad || c.Spin {
		t.Errorf("a watch holding a failure reads as waiting: %+v", c)
	}
	if right := text(c.Right); right != "1h06m of 1h50m · next look 40s" {
		t.Errorf("right %q", right)
	}
	if d := text(c.Detail[2]); !strings.Contains(d, "--until done") || !strings.Contains(d, "in 44m at its timeout") {
		t.Errorf("ends line %q", d)
	}
}

func TestTheCardWaitsWhenNothingHasFailed(t *testing.T) {
	now := time.Unix(10_000, 0)
	c := card(watch.Live{Number: 7, Until: watch.UntilFail, Start: now, Asked: now, Passed: 3, Pending: 1, Running: []string{"e2e"}}, now)
	if !c.Spin || !strings.Contains(text(c.Body), "waiting on e2e") {
		t.Errorf("%+v", c)
	}
	if c := card(watch.Live{Number: 7, Start: now}, now); !strings.Contains(text(c.Body), "asking GitHub") {
		t.Errorf("before an answer: %s", text(c.Body))
	}
}
