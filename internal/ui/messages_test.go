package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func sayModel() *Model {
	a := &fleet.Agent{Key: "k", DisplayName: "fix the status bar", Workspace: &host.Workspace{Path: "/src/rush/.claude/worktrees/6016523d", Repo: "/src/rush"}}
	return &Model{snap: &fleet.Snapshot{At: time.Now(), Agents: []*fleet.Agent{a}}, store: &state.Store{}}
}

// A refusal is plain and goes; a failure is red and stays until a key.
func TestSayKinds(t *testing.T) {
	m := sayModel()
	m.flash("select an agent first", true)
	if m.statusErr || !m.statusShown() {
		t.Fatal("a refusal shows, and isn't a failure")
	}
	m.snap.At = time.Now().Add(flashFor + time.Second)
	if m.statusShown() {
		t.Fatal("a refusal goes after a few seconds")
	}
	m.fail("couldn't switch: no such account")
	m.snap.At = time.Now().Add(time.Hour)
	if !m.statusErr || !m.statusShown() {
		t.Fatal("a failure stays until a key")
	}
	m.keyClearsFailure()
	if m.statusShown() {
		t.Fatal("a key takes a failure off the line")
	}
	if len(m.said) != 2 || m.said[1].kind != sayFailed {
		t.Fatalf("the log keeps both: %+v", m.said)
	}
}

// News never takes the line under the box; it's in the header's count
// until #log has been opened.
func TestNewsStaysOffTheLine(t *testing.T) {
	m := sayModel()
	m.flash("copied", false)
	m.news("kept “fix the status bar”'s worktree: 1 commit not pushed or in main", newsRefProjects+"/src/rush")
	if m.status != "copied" {
		t.Fatalf("news took the line: %q", m.status)
	}
	if h := ansi.Strip(m.newsHead()); !strings.Contains(h, "1 from rush") {
		t.Fatalf("header: %q", h)
	}
	m.openLog()
	m.sheet.key(m, tea.KeyPressMsg{}, "esc")
	if len(m.unseenNews()) != 0 || m.newsHead() != "" {
		t.Fatal("closing #log sees the news")
	}
}

// The tidy-up names a kept worktree by its session, not its folder.
func TestKeptWorktreeNamed(t *testing.T) {
	m := sayModel()
	m.onTidied(tidiedMsg{kept: map[string]string{"/src/rush/.claude/worktrees/6016523d": "6016523d isn't safe to remove: 1 commit not pushed or in main"}})
	if m.status != "" {
		t.Fatalf("the tidy-up took the line: %q", m.status)
	}
	n := m.unseenNews()
	if len(n) != 1 {
		t.Fatal("no news")
	}
	if s := m.said[n[0]]; strings.Contains(s.text, "6016523d") || !strings.Contains(s.text, "“fix the status bar”") || s.ref != newsRefProjects+"/src/rush" {
		t.Fatalf("news: %+v", s)
	}
}

// #log opens the log, newest first; closing it sees the news.
func TestLogSheet(t *testing.T) {
	m := sayModel()
	m.w = 120
	m.flash("one", false)
	m.news("two: happened", "")
	m.openLog()
	l, ok := m.sheet.(*logSheet)
	if !ok {
		t.Fatal("no log sheet")
	}
	body := ansi.Strip(strings.Join(l.body(m, 100, 20), "\n"))
	if strings.Index(body, "two") > strings.Index(body, "one") {
		t.Fatalf("newest first:\n%s", body)
	}
	l.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != nil || len(m.unseenNews()) != 0 {
		t.Fatal("esc closes it and sees the news")
	}
	for i := 0; i < logKept+5; i++ {
		m.flash("x", false)
	}
	if len(m.said) != logKept {
		t.Fatalf("keeps %d, not %d", len(m.said), logKept)
	}
}

// What rush says shows with the Session on the whole screen too, and
// beside the list's prompt when that's there.
func TestSaidInEveryLayout(t *testing.T) {
	for _, full := range []bool{true, false} {
		m, _ := benchModel(220, 40)
		m.full = full
		m.View()
		m.flash("select an agent first", true)
		if out := ansi.Strip(m.View().Content); strings.Count(out, "select an agent first") != 1 {
			t.Fatalf("full %v: said %d times", full, strings.Count(out, "select an agent first"))
		}
	}
}
