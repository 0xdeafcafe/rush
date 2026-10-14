package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/ghpr"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The list's Pull requests row has every project's, project by project;
// enter goes into it, and enter on one opens it whole: who it waits on,
// its checks and its body.
func TestProjectsPullRequests(t *testing.T) {
	now := time.Now()
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now}, w: 160, h: 50}
	a := &fleet.Agent{Key: "a1", PID: 1, Root: "/src/alpha", Repo: "/src/alpha", UpdatedAt: now}
	m.snap.Agents = append(m.snap.Agents, a)
	m.prs.byRoot = map[string]ghpr.Repo{"/src/alpha": {Root: "/src/alpha", At: now, PRs: []ghpr.PR{
		{Number: 41, Title: "Add the gate", Mine: true, Author: "me", Head: "gate", Base: "main", Decision: "REVIEW_REQUIRED",
			Waiting: []string{"sam", "web-team"}, Reviews: []ghpr.Review{{Who: "kim", State: "APPROVED", Body: "looks good"}},
			Checks: []ghpr.Check{{Name: "test", State: "passed"}, {Name: "lint", State: "failed"}},
			Body:   "## Why\n\nThe gate shares the machine."},
		{Number: 9, Title: "Their fix", AskedYou: true, Author: "sam"},
	}}}

	m.pickInProjects(panePRs)
	page := ansi.Strip(strings.Join(m.projectsBody(), "\n"))
	for _, s := range []string{"⇡ Pull requests", "2 want you", "ALPHA", "#41", "1/2 ✗1", "waiting on sam, web-team", "#9", "your review", "@sam"} {
		if !strings.Contains(page, s) {
			t.Fatalf("%q missing:\n%s", s, page)
		}
	}

	m.projectsKey("enter")
	if !m.work.projIn || m.work.inSel != "r/src/alpha#41" {
		t.Fatalf("enter: in %v on %q", m.work.projIn, m.work.inSel)
	}
	m.projectsKey("enter")
	sh, ok := m.sheet.(*prSheet)
	if !ok {
		t.Fatalf("enter on a pull request opened %T", m.sheet)
	}
	body := ansi.Strip(strings.Join(sh.body(m, 120, 60), "\n"))
	for _, s := range []string{"#41 Add the gate", "gate into main", "✓ kim approved", "looks good", "◌ sam asked", "✗ lint", "✓ test", "Why", "The gate shares the machine."} {
		if !strings.Contains(body, s) {
			t.Fatalf("sheet: %q missing:\n%s", s, body)
		}
	}

	// The project's own page has them too.
	m.sheet, m.work.projIn = nil, false
	m.pickInProjects("p/src/alpha")
	page = ansi.Strip(strings.Join(m.projectsBody(), "\n"))
	if !strings.Contains(page, "PULL REQUESTS  2") || !strings.Contains(page, "#41") {
		t.Fatalf("project page lacks its pull requests:\n%s", page)
	}
}


// An agent knows the pull requests it opened and GitHub's for its branch;
// its Overview lists them, and the edge under its box names one or two,
// or says where more are.
func TestAgentPullRequests(t *testing.T) {
	now := time.Now()
	a := &fleet.Agent{Key: "a1", PID: 1, Root: "/src/alpha", Repo: "/src/alpha", Branch: "gate", UpdatedAt: now}
	a.PRs = []agent.PR{{Number: 3, Title: "Old merged one", State: "MERGED", URL: "https://x/3"}}
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: now, Agents: []*fleet.Agent{a}}, w: 160, h: 50}
	m.prs.byRoot = map[string]ghpr.Repo{"/src/alpha": {Root: "/src/alpha", At: now, PRs: []ghpr.PR{
		{Number: 41, Title: "Add the gate", Mine: true, Head: "gate", Base: "main", State: "OPEN", Decision: "APPROVED",
			Checks: []ghpr.Check{{Name: "test", State: "passed"}}},
		{Number: 9, Title: "Someone else's", AskedYou: true, Head: "other", State: "OPEN"},
	}}}

	prs := m.agentPRs(a)
	if len(prs) != 2 || prs[0].pr.Number != 41 || prs[1].pr.Number != 3 || prs[1].root != "" {
		t.Fatalf("agent's PRs = %+v", prs)
	}
	note := ansi.Strip(m.prBoxNote(a, 120))
	if note != "⇡ #41 ✓ approved · #3 merged" {
		t.Errorf("two by number: %q", note)
	}
	a.PRs = nil
	if note := ansi.Strip(m.prBoxNote(a, 120)); !strings.HasPrefix(note, "⇡ #41 ✓ approved · Add the gate") {
		t.Errorf("one with its title: %q", note)
	}
	a.PRs = []agent.PR{{Number: 3}, {Number: 4}}
	if note := ansi.Strip(m.prBoxNote(a, 120)); note != "⇡ 3 pull requests · Overview has them" {
		t.Errorf("more: %q", note)
	}

	c := &hostConn{key: "a1", kind: "claude", sess: convo.New(), open: map[string]bool{}}
	var refs []string
	var b strings.Builder
	for _, l := range m.overviewSections(c, convo.Options{Width: 100, Now: now}) {
		if l.Ref != "" {
			refs = append(refs, l.Ref)
		}
		b.WriteString(ansi.Strip(l.Text) + "\n")
	}
	text := b.String()
	if !strings.HasPrefix(strings.Join(refs, " "), "pr:41 pr:3 pr:4") || !strings.Contains(text, "Pull requests") {
		t.Fatalf("overview: %v\n%s", refs, text)
	}
	m.openAgentPR(a, 41)
	if sh, ok := m.sheet.(*prSheet); !ok || sh.num != 41 {
		t.Fatalf("space on #41 opened %T", m.sheet)
	}
}
