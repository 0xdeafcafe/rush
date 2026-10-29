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

// The last answer marks the line the quote is on, every row it wraps
// to, and says why straight under it; the earlier answer saying the same
// isn't marked.
func TestNeedsMarksTheQuote(t *testing.T) {
	s := needsSession()
	before := needsRender(s, Needs{}) // drawn once without: the cache mustn't keep it
	if strings.Contains(strings.Join(before, "\n"), "▌") {
		t.Fatalf("marked with nothing asked:\n%s", strings.Join(before, "\n"))
	}
	got := needsRender(s, Needs{Quote: "main. I haven't pushed it.", Reason: "push the merge, then rebuild rush"})
	all := strings.Join(got, "\n")
	var marked []string
	for _, l := range got {
		if strings.Contains(l, "▌") {
			marked = append(marked, l)
		}
	}
	if len(marked) < 3 || !strings.Contains(marked[len(marked)-1], "▌needs you · push the merge") ||
		!strings.Contains(marked[0], "The Claude Code change") || !strings.Contains(marked[len(marked)-2], "rebuilt.") {
		t.Fatalf("marked rows %q in\n%s", marked, all)
	}
	if strings.Contains(all, "▌Merged") {
		t.Fatalf("the earlier answer is marked:\n%s", all)
	}
}

// A quote late in a paragraph marks that paragraph from its first row, not
// from the row the quote starts on, and why goes under it, before what
// follows; the paragraphs around it aren't marked.
func TestNeedsMarksItsParagraph(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "fix it"}, at(0))
	s.Apply(say("Fixed the panel.\n\nThis is commit b0cca013, built on the latest main. The panel tests pass. "+
		"I've only seen it drawn in that test, not in a running rush.\n\nEverything is on the branch, not pushed."), at(1))
	s.Apply(headless.Result{Subtype: "success"}, at(2))
	got := needsRender(s, Needs{Quote: "I've only seen it drawn in that test, not in a running rush.", Reason: "try it in rush"})
	all := strings.Join(got, "\n")
	first, why, next := -1, -1, -1
	for i, l := range got {
		switch {
		case strings.Contains(l, "This is commit"):
			first = i
		case strings.Contains(l, "needs you · try it in rush"):
			why = i
		case strings.Contains(l, "Everything is on"):
			next = i
		}
	}
	if first < 0 || why < 0 || next < 0 || !(first < why && why < next) {
		t.Fatalf("paragraph at %d, why at %d, next at %d:\n%s", first, why, next, all)
	}
	for i := first; i <= why; i++ {
		if !strings.Contains(got[i], "▌") {
			t.Fatalf("row %d of the paragraph isn't marked:\n%s", i, all)
		}
	}
	if strings.Contains(got[next], "▌") || strings.Contains(all, "▌Fixed") {
		t.Fatalf("a paragraph around it is marked:\n%s", all)
	}
}

// A quote the answer doesn't hold marks nothing.
func TestNeedsMissingQuoteMarksNothing(t *testing.T) {
	got := strings.Join(needsRender(needsSession(), Needs{Quote: "approve PR #41", Reason: "approve it"}), "\n")
	if strings.Contains(got, "▌") {
		t.Fatalf("marked:\n%s", got)
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
