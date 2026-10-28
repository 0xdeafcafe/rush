package convo

import (
	"context"
	"errors"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/ghpr"
)

// testLinks turns lookups on for one test, forgetting every answer after.
func testLinks(t *testing.T) {
	t.Helper()
	LookLinks(true)
	t.Cleanup(func() {
		LookLinks(false)
		linkAnswers.Lock()
		linkAnswers.by = map[string]*linkAnswer{}
		linkAnswers.Unlock()
	})
}

// waitLink waits for a link's lookup to answer.
func waitLink(t *testing.T, u string) any {
	t.Helper()
	for end := time.Now().Add(5 * time.Second); time.Now().Before(end); time.Sleep(time.Millisecond) {
		if _, _, v, ok := LinkInfo(u); ok {
			return v
		}
	}
	t.Fatalf("%s never answered", u)
	return nil
}

// A kind registered once draws its badge after every link it knows,
// bare links and markdown ones alike, once its lookup has answered: never
// while the frame that first draws it is drawn.
func TestLinkKindBadge(t *testing.T) {
	testLinks(t)
	var asked atomic.Int32
	RegisterLink(LinkKind[string]{
		Name: "test-ticket",
		Key: func(u string) string {
			if k, ok := strings.CutPrefix(u, "https://tickets.test/"); ok {
				return strings.SplitN(k, "#", 2)[0]
			}
			return ""
		},
		Look:  func(_ context.Context, key string) (string, error) { asked.Add(1); return "state of " + key, nil },
		Badge: func(v *string) string { return "[" + *v + "]" },
	})
	u := "https://tickets.test/T-1"
	if got := ansi.Strip(Inline("see "+u, cText)); strings.Contains(got, "[") {
		t.Fatalf("drawn with a badge before its lookup answered: %q", got)
	}
	waitLink(t, u)
	if got := ansi.Strip(Inline("see "+u+" now", cText)); got != "see "+u+" [state of T-1] now" {
		t.Errorf("bare link = %q", got)
	}
	if got := ansi.Strip(Inline("see [the ticket]("+u+"#top)", cText)); got != "see the ticket [state of T-1]" {
		t.Errorf("markdown link = %q", got)
	}
	if n := asked.Load(); n != 1 {
		t.Errorf("asked %d times for one key, want once", n)
	}
	// The badge opens the same address the link does.
	if out := Inline(u, cText); strings.Count(out, "\x1b]8;;"+u+"\x1b\\") != 2 {
		t.Errorf("badge not linked: %q", out)
	}
}

// With lookups off, as in every other test, nothing is asked or drawn.
func TestLinkKindOff(t *testing.T) {
	LookLinks(false)
	u := "https://github.com/acme/terrafied/pull/33"
	if _, _, _, ok := LinkInfo(u); ok {
		t.Fatal("answered with lookups off")
	}
	if got := ansi.Strip(Inline(u, cText)); got != u {
		t.Errorf("drawn %q", got)
	}
}

// A failed lookup draws the link bare and isn't asked again at once.
func TestLinkKindFailed(t *testing.T) {
	testLinks(t)
	var asked atomic.Int32
	RegisterLink(LinkKind[int]{
		Name:  "test-broken",
		Key:   func(u string) string { s, _ := strings.CutPrefix(u, "https://broken.test/"); return s },
		Look:  func(context.Context, string) (int, error) { asked.Add(1); return 0, errors.New("signed out") },
		Badge: func(*int) string { return "never" },
	})
	u := "https://broken.test/x"
	LinkInfo(u)
	for end := time.Now().Add(5 * time.Second); asked.Load() == 0 && time.Now().Before(end); time.Sleep(time.Millisecond) {
	}
	time.Sleep(10 * time.Millisecond)
	for range 3 {
		if _, _, _, ok := LinkInfo(u); ok {
			t.Fatal("a failure answered")
		}
	}
	if n := asked.Load(); n != 1 {
		t.Errorf("asked %d times, want once", n)
	}
}

func TestPRBadge(t *testing.T) {
	for _, c := range []struct {
		pr   ghpr.PR
		want string
	}{
		{ghpr.PR{State: "MERGED"}, "⇡ merged"},
		{ghpr.PR{State: "CLOSED"}, "⇡ closed"},
		{ghpr.PR{State: "OPEN", Draft: true}, "⇡ draft"},
		{ghpr.PR{State: "OPEN", Decision: "APPROVED", Checks: []ghpr.Check{{State: "passed"}}}, "⇡ open · ✓ checks · approved"},
		{ghpr.PR{State: "OPEN", Decision: "REVIEW_REQUIRED", Checks: []ghpr.Check{{State: "passed"}, {State: "failed"}, {State: "failed"}}}, "⇡ open · ✗ 2 failing · needs review"},
		{ghpr.PR{State: "OPEN", Checks: []ghpr.Check{{State: "pending"}}}, "⇡ open · 1 running"},
	} {
		if got := ansi.Strip(prBadge(&c.pr)); got != c.want {
			t.Errorf("prBadge(%+v) = %q, want %q", c.pr, got, c.want)
		}
	}
	if got := ansi.Strip(issueBadge(&ghpr.Issue{State: "CLOSED", Reason: "NOT_PLANNED"})); got != "◎ not planned" {
		t.Errorf("issueBadge = %q", got)
	}
}
