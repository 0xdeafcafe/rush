package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

// needsSession is two finished turns whose answers both say the quote.
func needsSession() *Session {
	s := New()
	s.Apply(host.Sent{Text: "merge it"}, at(0))
	s.Apply(say("Merged. I haven't pushed it yet."), at(1))
	s.Apply(headless.Result{Subtype: "success"}, at(2))
	s.Apply(host.Sent{Text: "and the other one"}, at(10))
	s.Apply(say("The Claude Code change is merged into main as `de90d0e3`. It merged cleanly, rush builds, "+
		"and the new test passes on main. I haven't pushed it. It takes effect for sessions started after rush is rebuilt."), at(11))
	s.Apply(headless.Result{Subtype: "success"}, at(12))
	return s
}

func needsRender(s *Session, n Needs) []string {
	lines := s.Render(Options{Width: 46, Now: at(20), Open: map[string]bool{"t1": true, "t2": true}, Needs: n})
	out := make([]string, len(lines))
	for i, l := range lines {
		out[i] = strings.TrimRight(stripANSI(l.Text), " ")
	}
	return out
}

// The last turn ends with a Needs-you section: what it wants, the rows
// laid out for it, and the words that asked; the answer itself isn't
// marked, and an earlier turn gets no section.
func TestNeedsSection(t *testing.T) {
	s := needsSession()
	before := strings.Join(needsRender(s, Needs{}), "\n") // drawn once without: the cache mustn't keep it
	if strings.Contains(before, "Needs you") {
		t.Fatalf("a section with nothing asked:\n%s", before)
	}
	n := Needs{Kind: "you", Reason: "push the merge", Quote: "I haven't pushed it.", Items: []NeedsItem{
		{Kind: "do", Text: "rebuild rush"}, {Kind: "run", Text: "git push origin main"}, {Kind: "reply", Text: "yes, push it"}}}
	lines := s.Render(Options{Width: 80, Now: at(20), Open: map[string]bool{"t1": true, "t2": true}, Needs: n})
	var got []string
	refs := map[string]bool{}
	for _, l := range lines {
		got = append(got, strings.TrimRight(stripANSI(l.Text), " "))
		refs[l.Ref] = true
	}
	all := strings.Join(got, "\n")
	t.Logf("\n%s", all)
	head, answer := -1, -1
	for i, l := range got {
		switch {
		case strings.Contains(l, "╭ ! Needs you · push the merge"):
			head = i
		case strings.Contains(l, "The Claude Code change"):
			answer = i
		}
	}
	if head < 0 || answer < 0 || head < answer || strings.Count(all, "Needs you") != 1 {
		t.Fatalf("section at %d, answer at %d:\n%s", head, answer, all)
	}
	for _, want := range []string{"☐ rebuild rush", "$ git push origin main", "click copies", "↩ yes, push it", "╰ it said “I haven't pushed it.”"} {
		if !strings.Contains(all, want) {
			t.Errorf("no %q in\n%s", want, all)
		}
	}
	if !refs[NeedsRef("run", "git push origin main")] || !refs[NeedsRef("reply", "yes, push it")] {
		t.Errorf("the command and reply rows aren't clickable")
	}
	if strings.Contains(all, "▌") {
		t.Errorf("the answer is still marked:\n%s", all)
	}
}

func TestNeedsRef(t *testing.T) {
	k, txt, ok := NeedsOf(NeedsRef("run", "a: b"))
	if !ok || k != "run" || txt != "a: b" {
		t.Fatalf("NeedsOf = %q %q %v", k, txt, ok)
	}
}

func TestHasQuote(t *testing.T) {
	for _, c := range []struct {
		text, q string
		want    bool
	}{
		{"merged as `de90d0e3`.\nI  haven't pushed it.", "merged as de90d0e3. I haven't pushed it", true},
		{"**Push** it now", "push it", true},
		{"push it", "", false},
		{"push it", "pull it", false},
	} {
		if got := HasQuote(c.text, c.q); got != c.want {
			t.Errorf("HasQuote(%q, %q) = %v", c.text, c.q, got)
		}
	}
}
