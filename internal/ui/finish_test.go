package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/ghpr"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

func finishPR() ghpr.PR {
	return ghpr.PR{Number: 118, Title: "fix the flaky tax rounding test", State: "OPEN", URL: "https://github.com/o/r/pull/118",
		Added: 18, Removed: 6, Files: 2, Comments: 3,
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
	for _, row := range rows {
		if cellw.String(row) > 100 {
			t.Errorf("row wider than the card: %q", ansi.Strip(row))
		}
	}
	for _, want := range []string{"#118 ↗", "fix the flaky tax rounding test", "+18 −6", "2 files", "3 comments", "can't merge yet: the unit check fails", "✗ unit", "✓ pushed", "✗ checks 4 of 5", "└─ ✗ unit", "f fix it with this agent", "m merge anyway", "↑ gives it the keys"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	if _, used := m.finishKey(c, "w"); used {
		t.Fatal("w answered the card without its keys")
	}
	c.cardFocus = true
	if _, used := m.finishKey(c, "w"); !used || !c.finishGone["finish:118"] || c.finish != nil {
		t.Fatalf("w didn't set it aside: gone %v", c.finishGone)
	}
	pr := finishPR()
	if got := fixText(&pr); !strings.Contains(got, "The unit check fails") || !strings.Contains(got, "PR #118") {
		t.Errorf("f asks %q", got)
	}
}

// The steps tell done (✓) from not needed (–) and mark where the pull
// request is now (▸); a merge under way says so.
func TestFinishStepsSayWhereItIs(t *testing.T) {
	m := &Model{}
	p := ghpr.PR{Number: 8, Title: "tags", State: "OPEN", Head: "rush/x", Base: "main", Added: 3, Removed: 1, Files: 2}
	c := &hostConn{finish: &finishCard{id: "finish:8", root: "/src/r", pr: p}}
	text := ansi.Strip(strings.Join(m.finishRows(c, 100), "\n"))
	for _, want := range []string{"– no checks", "– no review needed", "▸ merge"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	c.finish.pr.Decision = "REVIEW_REQUIRED"
	if text := ansi.Strip(strings.Join(m.finishRows(c, 100), "\n")); !strings.Contains(text, "▸ review") || !strings.Contains(text, "○ merge") {
		t.Errorf("a review needed is where it is, merge after it:\n%s", text)
	}
	c.finish.merging, c.finish.asking = "squash", true
	if text := ansi.Strip(strings.Join(m.finishRows(c, 100), "\n")); !strings.Contains(text, "is merging…") || !strings.Contains(text, "◌ merging (squash)…") ||
		!strings.Contains(text, "◌ asking GitHub") || strings.Contains(text, "m merge") {
		t.Errorf("a merge under way says so, and can't be asked again:\n%s", text)
	}
	d := mergeDetail(&p)
	for _, want := range []string{"rush/x → main", "+3 −1 · 2 files", "checks: none run on it · review: none needed", `squash: one commit on main, titled "tags"`} {
		if !strings.Contains(d, want) {
			t.Errorf("merge detail has no %q:\n%s", want, d)
		}
	}
}

// With several pull requests the card has a tab each, opens on the one
// that most needs you, and ← → or a click moves between them; w sets
// aside only the one shown.
func TestFinishTabs(t *testing.T) {
	now := time.Now()
	a := &fleet.Agent{Key: "a1", PID: 1, Root: "/src/alpha", Repo: "/src/alpha", UpdatedAt: now}
	a.PRs = []agent.PR{{Number: 15}, {Number: 16}, {Number: 17}}
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now, Agents: []*fleet.Agent{a}}, w: 160, h: 50}
	failing := finishPR()
	failing.Number = 16
	m.prs.byRoot = map[string]ghpr.Repo{"/src/alpha": {Root: "/src/alpha", At: now, PRs: []ghpr.PR{
		{Number: 15, Title: "A calmer conversation", State: "MERGED"},
		failing,
		{Number: 17, Title: "Pending one", State: "OPEN", Checks: []ghpr.Check{{Name: "ci", State: "pending"}, {Name: "lint", State: "passed"}}},
	}}}
	c := &hostConn{key: "a1", kind: "claude", sess: convo.New(), open: map[string]bool{}}
	c.finish = m.finishFor(c)
	f := c.finish
	if f == nil || len(f.tabs) != 3 || f.pr.Number != 16 {
		t.Fatalf("card = %+v", f)
	}
	text := ansi.Strip(strings.Join(m.finishRows(c, 120), "\n"))
	for _, want := range []string{"#15 merged", "#16 ✗ 1 failing", "#17 ◌ 1/2", "PR #16 can't merge yet", "← → the others"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	if len(c.finishBtns) != 3 {
		t.Fatalf("tabs to click: %+v", c.finishBtns)
	}
	c.cardFocus = true
	m.finishKey(c, "right")
	if c.finish.pr.Number != 17 || c.finishAt != 17 {
		t.Fatalf("→ shows #%d", c.finish.pr.Number)
	}
	m.finishKey(c, "pr:15")
	if c.finish.pr.Number != 15 {
		t.Fatalf("click shows #%d", c.finish.pr.Number)
	}
	// Kept on the next draw.
	if c.finish = m.finishFor(c); c.finish.pr.Number != 15 {
		t.Fatalf("redrawn on #%d", c.finish.pr.Number)
	}
	m.finishKey(c, "w")
	if c.finish == nil || len(c.finish.tabs) != 2 || c.finish.pr.Number == 15 {
		t.Fatalf("w set aside more than the one: %+v", c.finish)
	}
}
