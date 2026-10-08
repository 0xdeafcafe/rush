package main

import (
	"github.com/0xdeafcafe/photon/jsonx"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const links = `{"links": [
  {"id": "card_aaa111", "name": "Fix login", "column": "in_progress", "projectPath": "/src/app",
   "sessionLink": {"sessionId": "s-1"}, "worktreeLink": {"path": "/src/app/.claude/worktrees/fix-login", "branch": "fix-login"},
   "prLinks": [{"number": 7, "status": "failing", "unresolvedThreads": 1, "firstUnresolvedThreadURL": "https://gh/t/1",
     "checkRuns": [{"name": "lint", "status": "completed", "conclusion": "failure"}, {"name": "test", "status": "completed", "conclusion": "success"}]}],
   "manualOverrides": {}, "manuallyArchived": false, "source": "manual", "isRemote": false, "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-01T00:00:00Z"},
  {"id": "card_bbb222", "column": "backlog", "projectPath": "/src/app", "issueLink": {"number": 12, "title": "Dark mode", "body": "Please."},
   "manualOverrides": {}, "manuallyArchived": false, "source": "githubIssue", "isRemote": false, "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-02T00:00:00Z"},
  {"id": "card_ccc333", "name": "Old", "column": "done", "manuallyArchived": true, "manualOverrides": {}, "source": "manual", "isRemote": false, "createdAt": "x", "updatedAt": "x"},
  {"id": "card_ddd444", "name": "A subagent", "column": "in_progress", "parentCardId": "card_aaa111", "manualOverrides": {}, "manuallyArchived": false, "source": "manual", "isRemote": false, "createdAt": "x", "updatedAt": "x"},
  {"id": "card_eee555", "name": "Discovered", "column": "all_sessions", "manualOverrides": {}, "manuallyArchived": false, "source": "discovered", "isRemote": false, "createdAt": "x", "updatedAt": "x"}
]}`

func testBoard(t *testing.T) []card {
	t.Helper()
	home := t.TempDir()
	t.Setenv("KANBAN_CODE_HOME", home)
	if err := os.WriteFile(filepath.Join(home, "links.json"), []byte(links), 0o600); err != nil {
		t.Fatal(err)
	}
	cards, err := readCards()
	if err != nil {
		t.Fatal(err)
	}
	return cards
}

func TestReadCards(t *testing.T) {
	cards := testBoard(t)
	if len(cards) != 2 {
		t.Fatalf("archived, subagent and unfiled cards should be left off: %+v", cards)
	}
	b := board(cards, "")
	if !strings.Contains(b, "## In Progress\n- card_aaa111  Fix login  [fix-login]  PR #7 failing") ||
		!strings.Contains(b, "## Backlog\n- card_bbb222  Dark mode") {
		t.Fatalf("board =\n%s", b)
	}
	if b := board(cards, "backlog"); strings.Contains(b, "Fix login") {
		t.Fatalf("one column shows others:\n%s", b)
	}
}

func TestCardOf(t *testing.T) {
	cards := testBoard(t)
	for name, c := range map[string]struct {
		s    session
		want string
	}{
		"meta":     {session{Meta: map[string]string{"card": "card_bbb222"}, SessionID: "s-1"}, "card_bbb222"},
		"session":  {session{SessionID: "s-1"}, "card_aaa111"},
		"worktree": {session{Cwd: "/src/app/.claude/worktrees/fix-login/web"}, "card_aaa111"},
		"none":     {session{Cwd: "/src/app/.claude/worktrees/fix-login-2"}, ""},
	} {
		got, ok := cardOf(cards, c.s)
		if c.want == "" && ok || c.want != "" && got.ID != c.want {
			t.Errorf("%s: got %q, %v", name, got.ID, ok)
		}
	}
	for q, want := range map[string]string{"card_bbb222": "card_bbb222", "card_a": "card_aaa111", "dark MODE": "card_bbb222"} {
		if c, err := findCard(cards, q); err != nil || c.ID != want {
			t.Errorf("findCard(%q) = %q, %v", q, c.ID, err)
		}
	}
	if _, err := findCard(cards, "card_"); err == nil {
		t.Error("an ambiguous prefix found a card")
	}
}

func TestDescribeAndTask(t *testing.T) {
	cards := testBoard(t)
	d := describe(cards[0])
	for _, want := range []string{"Column: In Progress", "PR #7", "Failing checks: lint", "Unresolved review threads: 1 (first: https://gh/t/1)"} {
		if !strings.Contains(d, want) {
			t.Errorf("describe lacks %q:\n%s", want, d)
		}
	}
	if tk := task(cards[1]); !strings.HasPrefix(tk, "Resolve issue #12: Dark mode") || !strings.Contains(tk, "Please.") || !strings.Contains(tk, "card_bbb222") {
		t.Errorf("task = %q", tk)
	}
}

func TestPRNews(t *testing.T) {
	var p []pr
	_ = jsonx.Unmarshal([]byte(`[{"number": 7, "unresolvedThreads": 1, "firstUnresolvedThreadURL": "u",
		"checkRuns": [{"name": "lint", "conclusion": "failure"}]}]`), &p)
	seen, news := prNews(p, nil, false)
	if len(news) != 0 {
		t.Fatalf("the first look should only note how it is: %v", news)
	}
	if _, news = prNews(p, seen, true); len(news) != 0 {
		t.Fatalf("nothing changed, yet: %v", news)
	}
	p[0].UnresolvedThreads = 3
	p[0].CheckRuns = append(p[0].CheckRuns, p[0].CheckRuns[0])
	p[0].CheckRuns[1].Name = "test"
	_, news = prNews(p, seen, true)
	if len(news) != 2 || !strings.Contains(news[0], "checks failing: test.") || !strings.Contains(news[1], "3 unresolved review thread(s), 2 new") {
		t.Fatalf("news = %q", news)
	}
}
