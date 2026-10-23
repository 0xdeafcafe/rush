package ui

import (
	"os"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/ghpr"
	"github.com/charmbracelet/x/ansi"
)

func finishPR() ghpr.PR {
	return ghpr.PR{Number: 118, Title: "fix the flaky tax rounding test", State: "OPEN", URL: "https://github.com/o/r/pull/118",
		Checks: []ghpr.Check{
			{Name: "lint", State: "passed"}, {Name: "typecheck", State: "passed"}, {Name: "build", State: "passed"},
			{Name: "e2e", State: "passed"}, {Name: "unit", Workflow: "ci", State: "failed"},
		}}
}

// The card says first what stands between the pull request and merged.
func TestFinishHeadline(t *testing.T) {
	for _, tc := range []struct {
		edit func(p *ghpr.PR)
		want string
	}{
		{func(p *ghpr.PR) {}, "PR #118 can't merge yet: the unit check fails"},
		{func(p *ghpr.PR) { p.Checks[0].State = "failed" }, "PR #118 can't merge yet: 2 checks fail"},
		{func(p *ghpr.PR) { p.Checks[4].State = "pending" }, "PR #118 is waiting on its checks: 4 of 5 done"},
		{func(p *ghpr.PR) { p.Checks[4].State = "passed"; p.Decision = "REVIEW_REQUIRED" }, "PR #118 passes its checks and waits for a review"},
		{func(p *ghpr.PR) { p.Checks[4].State = "passed"; p.Decision = "APPROVED" }, "PR #118 is ready to merge"},
		{func(p *ghpr.PR) { p.State = "MERGED" }, "PR #118 is merged"},
	} {
		f := &finishCard{pr: finishPR()}
		tc.edit(&f.pr)
		if got, _ := f.headline(); got != tc.want {
			t.Errorf("headline %q, want %q", got, tc.want)
		}
	}
}

// Drawn: the headline, the failing check, the steps with each check under
// them, and f only while there's something to fix; w sets it aside.
func TestFinishCard(t *testing.T) {
	m := &Model{}
	c := &hostConn{finish: &finishCard{id: "finish:118", root: "/src/r", pr: finishPR()}}
	rows := m.finishRows(c, 100)
	if p := os.Getenv("RUSH_FINISH_DUMP"); p != "" {
		_ = os.WriteFile(p, []byte(strings.Join(rows, "\n")+"\n"), 0o644)
	}
	text := ansi.Strip(strings.Join(rows, "\n"))
	for _, want := range []string{"can't merge yet: the unit check fails", "✗ unit", "✓ pushed", "✗ checks 4 of 5", "└─ ✗ unit", "f fix it with this agent", "m merge anyway", "↑ gives it the keys"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	if _, used := m.finishKey(c, "w"); used {
		t.Fatal("w answered the card without its keys")
	}
	c.cardFocus = true
	if _, used := m.finishKey(c, "w"); !used || c.finishGone != "finish:118" || c.finish != nil {
		t.Fatalf("w didn't set it aside: gone %q", c.finishGone)
	}
	pr := finishPR()
	if got := fixText(&pr); !strings.Contains(got, "The unit check fails") || !strings.Contains(got, "PR #118") {
		t.Errorf("f asks %q", got)
	}
}
