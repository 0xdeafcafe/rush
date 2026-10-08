package convo

import (
	"fmt"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
)

func historyFixture() *Session {
	s := New()
	for i := 1; i <= 4; i++ {
		s.Turns = append(s.Turns, &Turn{N: i, Prompt: fmt.Sprintf("Please investigate issue %d.", i), Items: []*Item{
			{Kind: KText, Text: fmt.Sprintf("Progress note %d", i)},
			{Kind: KText, Answer: true, Text: "Fixed the issue.\n\nThe second paragraph explains what changed."},
		}})
	}
	return s
}

// A collapsed turn keeps its message exactly as it is open, in full, and
// one dim line stands where the rest was: nothing above it moves.
func TestCollapsedTurnKeepsItsMessage(t *testing.T) {
	for _, width := range []int{20, 40, 80, 180} {
		t.Run(fmt.Sprint(width), func(t *testing.T) {
			s := historyFixture()
			s.Turns = s.Turns[:1]
			s.Turns[0].Prompt = strings.Repeat("Investigate 界🙂 carefully. ", 30)
			s.Turns[0].Items[1].Text = "The important result.\n\nEND OF ANSWER"
			shut := s.Render(Options{Width: width, Open: map[string]bool{"t1": false}})
			open := s.Render(Options{Width: width, Open: map[string]bool{"t1": true}})
			for _, l := range shut {
				if cellw.String(l.Text) > width {
					t.Fatalf("row wider than pane: %q", l.Text)
				}
			}
			out := plain(shut)
			if strings.Count(out, "carefully.") != 30 || !strings.Contains(out, "▸ show") || strings.Contains(out, "END OF ANSWER") || strings.Contains(out, "Progress note") {
				t.Fatalf("a collapsed turn is its message and one line:\n%s", out)
			}
			// Everything down to that line is as it is open.
			n := len(shut) - 1
			if got, want := plain(shut[:n]), plain(open[:n]); got != want {
				t.Fatalf("collapsing moved the message:\n%s\n---\n%s", got, want)
			}
		})
	}
}

// A turn something else started collapses the same way, to the line that
// says what woke it and the show line.
func TestCollapsedWakeup(t *testing.T) {
	s := historyFixture()
	s.Turns[0].From = "background shell · completed"
	s.Turns[0].Items = nil
	s.Turns[0].touch()
	var out []string
	for _, l := range s.Render(Options{Width: 100, Open: map[string]bool{"t1": false}}) {
		if l.Ref == "t1" {
			out = append(out, strings.TrimSpace(plain([]Line{l})))
		}
	}
	if len(out) != 2 || !strings.Contains(out[0], "background shell · completed") || !strings.HasSuffix(out[1], "show") {
		t.Fatalf("a wakeup is its line and the show line: %q", out)
	}
}

func TestHistoryDefaultsToEveryTurnOpen(t *testing.T) {
	s := historyFixture()
	s.Turns[0].Items[1].Text = "start\n\n" + strings.Repeat("More useful details.\n", 200) + "END OF ANSWER"
	out := plain(s.Render(Options{Width: 100}))
	if !strings.Contains(out, "END OF ANSWER") || strings.Contains(out, "open turn for full") {
		t.Fatalf("the default must draw older turns whole: %s", out)
	}
}

func TestManualCloseHidesATurn(t *testing.T) {
	s := historyFixture()
	o := Options{Width: 100, Open: map[string]bool{"t1": false}}
	if strings.Contains(plain(s.Render(o)), "Progress note 1") {
		t.Fatal("a turn closed by hand shows its body")
	}
}

func BenchmarkHistoryPreviewLongReply(b *testing.B) {
	s := historyFixture()
	s.Turns[0].Items[1].Text = strings.Repeat("This is a long answer.\n", 10000)
	o := Options{Width: 100}
	buf := s.Render(o)
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		s.Turns[0].touch()
		buf = s.RenderInto(o, buf)
	}
}
