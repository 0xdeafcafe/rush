package convo

import (
	"fmt"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

const loaderAnswer = "The lane is done.\n\nDecisions:\n" +
	"1. **Committing the loader work:** rush's permission check stopped the lane's commit, so this needs your go-ahead. (a) yes: I commit it on perf/loader as one commit per fix, nothing pushed; (b) no: leave it uncommitted for you to look at first.\n" +
	"My picks: 1a."

func decisionsSession(text string) *Session {
	s := New()
	s.Apply(host.Sent{Text: "go"}, at(0))
	s.Apply(event.Message{Role: "assistant", ID: "a", Parts: []event.Part{{Kind: event.Text, Text: text}}}, at(1))
	s.Apply(event.TurnEnd{Reason: "done"}, at(2))
	return s
}

// answer has a turn of yours start with text, and end.
func answer(s *Session, text string) {
	n := len(s.Turns)
	s.Apply(host.Sent{Text: text}, at(10*n))
	s.Apply(event.Message{Role: "assistant", ID: fmt.Sprint("r", n), Parts: []event.Part{{Kind: event.Text, Text: "On it."}}}, at(10*n+1))
	s.Apply(event.TurnEnd{Reason: "done"}, at(10*n+2))
}

// The block's picks are chosen only once a message of yours answers it:
// before that, and when something else came instead, they're suggestions,
// in every view alike, whatever happened to the card. News rush sent
// answers nothing.
func TestDecisionsAnsweredByWhatCameAfter(t *testing.T) {
	text := func(s *Session) string {
		var b strings.Builder
		for _, l := range s.Render(Options{Width: 140}) {
			b.WriteString(strings.TrimRight(stripANSI(l.Text), " ") + "\n")
		}
		return b.String()
	}
	open := func(got string) bool {
		return strings.Contains(got, "Decisions · not answered") && strings.Contains(got, "☆ yes") && !strings.Contains(got, "its pick")
	}
	s := decisionsSession(loaderAnswer)
	if got := text(s); !open(got) {
		t.Fatalf("nothing sent yet:\n%s", got)
	}
	ref := DecisionsRef(s.TurnRef(0))
	if got := strings.Join(func() (r []string) {
		for _, l := range s.Render(Options{Width: 140, Open: map[string]bool{DecisionsSent(ref): true}}) {
			r = append(r, stripANSI(l.Text))
		}
		return
	}(), "\n"); !strings.Contains(got, "★ yes") {
		t.Fatalf("an answer on its way:\n%s", got)
	}
	answer(s, "<rush-pr-news pr=\"o/r#1\">\nrush can't reach GitHub\n</rush-pr-news>")
	if got := text(s); !open(got) {
		t.Fatalf("rush's news answered it:\n%s", got)
	}
	answer(s, "1a 2b please")
	if got := text(s); strings.Contains(got, "not answered") || !strings.Contains(got, "its pick") {
		t.Fatalf("answered:\n%s", got)
	}

	s = decisionsSession(loaderAnswer)
	answer(s, "something else first")
	if got := text(s); !open(got) {
		t.Fatalf("passed over:\n%s", got)
	}
	// An answer to a later block is that block's.
	answer(s, loaderAnswer)
	s.Turns[len(s.Turns)-1].Items = decisionsSession(loaderAnswer).Turns[0].Items
	answer(s, "1a. Committing the loader work: yes")
	if got := text(s); strings.Count(got, "not answered") != 1 {
		t.Fatalf("the later answer went to both:\n%s", got)
	}
}

// An answer's decisions block is a list: one option a line, its label then
// what it does, the agent's pick starred; the prose around it as ever.
func TestDecisionsDrawnAsList(t *testing.T) {
	s := decisionsSession(loaderAnswer)
	answer(s, "1a. Committing the loader work: yes")
	var rows []string
	refs := map[string]bool{}
	for _, l := range s.Render(Options{Width: 140}) {
		rows = append(rows, strings.TrimRight(stripANSI(l.Text), " "))
		if strings.HasSuffix(l.Ref, ":dec") {
			refs[l.Ref] = true
		}
	}
	got := strings.Join(rows, "\n")
	for _, want := range []string{
		"The lane is done.",
		"Decisions",
		"1. Committing the loader work",
		"rush's permission check stopped the lane's commit, so this needs your go-ahead.",
		"★ yes  I commit it on perf/loader as one commit per fix, nothing pushed  its pick",
		"• no   leave it uncommitted for you to look at first",
	} {
		if !strings.Contains(got, want) {
			t.Fatalf("missing %q:\n%s", want, got)
		}
	}
	for _, gone := range []string{"(a)", "(b)", "My picks", "Decisions:"} {
		if strings.Contains(got, gone) {
			t.Fatalf("%q still shows:\n%s", gone, got)
		}
	}
	if len(refs) != 1 || !refs[DecisionsRef(s.TurnRef(0))] {
		t.Fatalf("block refs %v", refs)
	}
	// Read once while the text stays the same.
	it := s.Turns[0].Items[len(s.Turns[0].Items)-1]
	if p := it.parts(); p != it.parts() || len(p.ds) != 1 {
		t.Fatal("parts read again")
	}
}

// Without picks nothing is starred; an answer without a block is prose.
func TestDecisionsDrawnWithoutPicks(t *testing.T) {
	s := decisionsSession("Decisions:\n1. **Ship:** (a) yes; (b) no.\n2. **Where:** (a) main: straight in; (b) a branch: for review.")
	var b strings.Builder
	for _, l := range s.Render(Options{Width: 120}) {
		b.WriteString(stripANSI(l.Text) + "\n")
	}
	if got := b.String(); strings.Contains(got, "★") || !strings.Contains(got, "• a branch  for review") || !strings.Contains(got, "2. Where") {
		t.Fatalf("got:\n%s", got)
	}
	s = decisionsSession("1. one\n2. two")
	for _, l := range s.Render(Options{Width: 120}) {
		if strings.HasSuffix(l.Ref, ":dec") {
			t.Fatal("a plain list drawn as decisions")
		}
	}
}

// With its card up the block is one row; the drawing kept for the turn is
// redrawn as the card opens and closes.
func TestDecisionsOnCardIsOneRow(t *testing.T) {
	s := decisionsSession(loaderAnswer)
	ref := DecisionsRef(s.TurnRef(0))
	text := func(open map[string]bool) string {
		var b strings.Builder
		for _, l := range s.Render(Options{Width: 120, Open: open}) {
			b.WriteString(stripANSI(l.Text) + "\n")
		}
		return b.String()
	}
	for i, open := range []bool{false, true, false, true} {
		got := text(map[string]bool{DecisionsOnCard(ref): open})
		if hidden := strings.Contains(got, "Decisions · 1 decision · on the card below"); hidden != open || strings.Contains(got, "yes  I commit") == open {
			t.Fatalf("%d: card up %v:\n%s", i, open, got)
		}
	}
}
