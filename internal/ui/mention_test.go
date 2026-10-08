package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

func TestMention(t *testing.T) {
	fix := &fleet.Agent{Key: "a", DisplayName: "fix login bug", Branch: "fix/login"}
	docs := &fleet.Agent{Key: "b", DisplayName: "docs"}
	term := &fleet.Agent{Key: "c", DisplayName: "login page", Interactive: true}
	m := &Model{order: []*fleet.Agent{docs, fix, term}, groupOf: map[string]string{"a": "Working", "b": "Done"}}

	got := m.mentionMatches([]rune("@LOG"), 0)
	if len(got) != 1 || got[0].Name != "fix-login-bug" || got[0].Description != "fix login bug · Working · fix/login" {
		t.Fatalf("search by name, skipping what can't be messaged: %+v", got)
	}
	if got := m.mentionMatches([]rune("@"), 0); len(got) != 2 || got[0].Name != "docs" {
		t.Fatalf("@ alone offers every agent in the list's order: %+v", got)
	}
	if m.mentionMatches([]rune("@docs hi"), 0) != nil {
		t.Fatal("the picker closes once the cursor has left the tag")
	}
	if a, rest := m.mentioned("@Fix-Login-Bug  try again\nplease"); a != fix || rest != "try again\nplease" {
		t.Fatalf("got %v %q", a, rest)
	}
	if a, rest := m.mentioned("@" + nickname("a") + " hi"); a != fix || rest != "hi" || nickname("a") != nickname("a") || !strings.Contains(nickname("a"), "-") {
		t.Fatalf("the nickname tags it too: %v %q", a, rest)
	}
	if titleTag("Can you fix the flaky login test, please?") != "fix-flaky-login" || titleTag("") != "" {
		t.Fatalf("title tag %q", titleTag("Can you fix the flaky login test, please?"))
	}
	if a, _ := m.mentioned("@src/main.go explain"); a != nil {
		t.Fatal("an @ that tags no agent is left for the agent")
	}
}

// Mid-message, a tag is offered and completed where it's typed, and the
// Session's agent is told who it is and how to reach it.
func TestMentionMidMessage(t *testing.T) {
	docs := &fleet.Agent{Key: "b", ID: "b1b2c3d4", DisplayName: "docs", Rush: true, Cwd: "/src/site"}
	past := &fleet.Agent{Key: "p", DisplayName: "old run"}
	m := &Model{order: []*fleet.Agent{docs, past}, groupOf: map[string]string{}}

	in, back := []rune("ask @do and tell me"), len(" and tell me")
	if got := m.mentionMatches(in, back); len(got) != 1 || got[0].Name != mentionName(docs) || !strings.Contains(got[0].Name, "-") {
		t.Fatalf("mid-message tag: %+v", got)
	}
	if agentHandle(docs) != (community.Author{SessionID: docs.ID}).Username() || !tags(docs, "docs") {
		t.Fatalf("a rush session's handle is the one it chirps under, and its title still tags it: %s", agentHandle(docs))
	}
	out, b := completeMention(in, back, "docs")
	if string(out) != "ask @docs and tell me" || b != len("and tell me") {
		t.Fatalf("completed %q, cursor %d from the end", string(out), b)
	}
	if m.mentionMatches([]rune("mail me@do"), 0) != nil {
		t.Fatal("an @ inside a word (an email) isn't a tag")
	}

	sent := m.withMentions("ask @docs and @old-run what changed.", "self")
	if !strings.HasPrefix(sent, "ask @docs and @old-run what changed.\n\n") ||
		!strings.Contains(sent, "session send b1b2c3d4") || !strings.Contains(sent, "in /src/site") ||
		!strings.Contains(sent, "@old-run is old run, another agent rush runs; it isn't a rush session") {
		t.Fatalf("told: %q", sent)
	}
	if got := m.withMentions("@docs first", "self"); got != "@docs first" {
		t.Fatalf("a tag at the start sends straight to them, no note: %q", got)
	}
	if got := m.withMentions("me, @docs", "b"); got != "me, @docs" {
		t.Fatalf("a session isn't told about itself: %q", got)
	}
}
