package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A model's one line becomes what the turn wants; anything else is no
// answer, so the agent stays where it was.
func TestParseTurnWants(t *testing.T) {
	for in, want := range map[string]turnWants{
		"OFFER: check it in rush":                         {Wants: wantsOffer, Line: "check it in rush"},
		"**YOU:** approve PR #41.":                        {Wants: wantsYou, Line: "approve PR #41"},
		"DONE: merged into main\nmore text":               {Wants: wantsDone, Line: "merged into main"},
		"problem: tests fail on main":                     {Wants: wantsProblem, Line: "tests fail on main"},
		"YOU: push the merge\nQUOTE: I haven't pushed it": {Wants: wantsYou, Line: "push the merge", Quote: "I haven't pushed it"},
		"ASK: which theme\n**QUOTE:** \"Which theme?\"":   {Wants: wantsAsk, Line: "which theme", Quote: "Which theme?"},
		"DONE: merged\nQUOTE: it's merged":                {Wants: wantsDone, Line: "merged"},
	} {
		got, ok := parseTurnWants(in)
		if !ok || got != want {
			t.Errorf("parseTurnWants(%q) = %+v, %v; want %+v", in, got, ok, want)
		}
	}
	for _, in := range []string{"", "I think it's done", "MAYBE: who knows"} {
		if _, ok := parseTurnWants(in); ok {
			t.Errorf("parseTurnWants(%q) took it as an answer", in)
		}
	}
}

// A finished turn whose last message offers, asks, hands you a step or
// says it couldn't goes under Needs you, the reason on its second line;
// one that's done, or not read yet, or read for an older turn, stays
// under Finished.
func TestFinishedTurnsThatWantYouNeedYou(t *testing.T) {
	now := time.Now()
	agent := func(key string) *fleet.Agent {
		a := &fleet.Agent{Key: key, DisplayName: key, PID: 7, Root: "/src/rush", Branch: "rush/" + key}
		a.State, a.UpdatedAt = "done", now.Add(-5*time.Minute)
		return a
	}
	offer, done, unread, older := agent("offer"), agent("done"), agent("unread"), agent("older")
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 120, h: 40, lastState: map[string]string{}}
	m.turns.got = map[string]turnWants{
		"offer": {At: offer.UpdatedAt, Wants: wantsOffer, Line: "check it in rush"},
		"done":  {At: done.UpdatedAt, Wants: wantsDone, Line: "merged into main"},
		"older": {At: older.UpdatedAt.Add(-time.Hour), Wants: wantsAsk, Line: "which theme"},
	}
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{offer, done, unread, older}}
	m.rebuild()
	want := map[string]string{"offer": needsSection, "done": finishedSection, "unread": finishedSection, "older": finishedSection}
	for k, g := range want {
		if m.groupOf[k] != g {
			t.Errorf("%s in %q, want %q", k, m.groupOf[k], g)
		}
	}
	if got := ansi.Strip(m.agentWhere(offer, 80)); !strings.Contains(got, "check it in rush") {
		t.Errorf("offer's second line = %q, want its offer", got)
	}
	if got := ansi.Strip(m.agentWhere(done, 80)); !strings.Contains(got, "✓ merged into main") {
		t.Errorf("done's second line = %q, want that it's done", got)
	}
	offer.Seen = true // looked at: it asks nothing more
	m.rebuild()
	if m.groupOf["offer"] == needsSection || m.turnLabel(offer) != "" {
		t.Errorf("a looked-at offer still needs you: in %q", m.groupOf["offer"])
	}
}

// A turn that wants you says what on its Wall tile, in plain words.
func TestTurnWantsOnWall(t *testing.T) {
	now := time.Now()
	a := &fleet.Agent{Key: "k", DisplayName: "fix auth", PID: 7, State: "done", UpdatedAt: now.Add(-time.Minute)}
	m := &Model{store: &state.Store{}, previews: map[string]previewEntry{}, w: 120, h: 40, lastState: map[string]string{}}
	m.snap = &fleet.Snapshot{At: now, Agents: []*fleet.Agent{a}}
	m.turns.got = map[string]turnWants{"k": {At: a.UpdatedAt, Wants: wantsYou, Line: "approve PR #52"}}
	if got := ansi.Strip(strings.Join(m.wallAgentTile(a, 60, 8, false), "\n")); !strings.Contains(got, "needs you · approve PR #52") {
		t.Errorf("Wall tile doesn't say what it wants:\n%s", got)
	}
}
