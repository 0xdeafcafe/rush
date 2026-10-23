package convo

import (
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

// An answer's decisions block is a list: one option a line, its label then
// what it does, the agent's pick starred; the prose around it as ever.
func TestDecisionsDrawnAsList(t *testing.T) {
	s := decisionsSession(loaderAnswer)
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
