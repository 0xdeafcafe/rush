package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
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

// The sheet reviews, comments and merges as you, each asked or typed
// first; your own pull request it doesn't offer to review.
func TestPRSheetActs(t *testing.T) {
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: time.Now()}, w: 160, h: 50}
	theirs := ghpr.PR{Number: 9, Title: "Their fix", State: "OPEN", Author: "sam", Head: "fix", Base: "main"}
	m.prs.byRoot = map[string]ghpr.Repo{"/src/alpha": {Root: "/src/alpha", At: time.Now(), PRs: []ghpr.PR{theirs, {Number: 41, State: "OPEN", Mine: true}}}}
	s := &prSheet{root: "/src/alpha", num: 9, last: theirs, read: true, comments: []ghpr.Comment{{Who: "kim", Body: "what about tests?", At: time.Now()}}}
	m.sheet = s
	body := ansi.Strip(strings.Join(s.body(m, 120, 60), "\n"))
	for _, want := range []string{"a approve", "x ask for changes", "m merge", "CONVERSATION  1", "kim", "what about tests?"} {
		if !strings.Contains(body, want) {
			t.Fatalf("%q missing:\n%s", want, body)
		}
	}

	s.key(m, tea.KeyPressMsg{}, "a")
	if m.confirm == nil || !strings.Contains(m.confirm.question, "Approve #9") {
		t.Fatalf("a asks first: %+v", m.confirm)
	}
	m.confirm = nil
	s.key(m, tea.KeyPressMsg{}, "m")
	if m.confirm == nil || m.confirm.yesText != "squash" || len(m.confirm.more) != 2 {
		t.Fatalf("m asks how: %+v", m.confirm)
	}
	m.confirm = nil

	s.key(m, tea.KeyPressMsg{}, "c")
	for _, r := range "lgtm" {
		s.key(m, tea.KeyPressMsg{Code: r, Text: string(r)}, string(r))
	}
	if s.writing != "comment" || string(s.input) != "lgtm" {
		t.Fatalf("typing a comment: %q %q", s.writing, string(s.input))
	}
	if cmd := s.key(m, tea.KeyPressMsg{}, "enter"); cmd == nil || s.sending != "commenting" || s.writing != "" {
		t.Fatalf("enter posts it: sending %q", s.sending)
	}

	mine := &prSheet{root: "/src/alpha", num: 41, last: ghpr.PR{Number: 41, State: "OPEN", Mine: true}, read: true}
	if b := ansi.Strip(strings.Join(mine.body(m, 120, 60), "\n")); strings.Contains(b, "approve") {
		t.Fatalf("offers to approve your own:\n%s", b)
	}
}
