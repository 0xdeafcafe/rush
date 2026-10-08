package convo

import (
	"slices"
	"testing"
)

// A window draws only the turns holding the rows asked for, and they're
// drawn as a whole render draws them. Read bottom to top a window at a
// time, every turn ends up counted as drawn: the session's rows add up.
func TestRenderWindowDrawsOnlyItsTurns(t *testing.T) {
	ref, s := benchSession(40), benchSession(40)
	o := Options{Width: 120, Now: at(100000), Open: map[string]bool{}}
	whole := ref.Render(o)
	ref.RenderWindow(o, 0, 1<<30, nil) // every turn counted as drawn
	if _, total := ref.Rows(); total != len(whole) {
		t.Fatalf("drawn whole, the rows add up to %d, not %d", total, len(whole))
	}
	var out []Line
	for to := 1 << 30; ; { // from the end, which isn't counted yet
		from := max(0, to-60)
		out = s.RenderWindow(o, from, to, out)
		lo, hi := s.WindowTurns()
		want := whole[ref.TurnRow(lo):ref.TurnRow(hi)]
		if hi == len(s.Turns) {
			want = whole[ref.TurnRow(lo):]
		}
		if !slices.Equal(out, want) {
			t.Fatalf("turns %d to %d drew %d rows, not the %d a whole render has", lo, hi, len(out), len(want))
		}
		if base, _ := s.Rows(); lo > 0 && s.TurnAt(base) != lo || s.TurnOf(s.TurnRef(lo)+":s:x") != lo {
			t.Fatalf("turn %d isn't found at its row %d", lo, base)
		}
		if lo == 0 {
			break
		}
		to = s.TurnRow(lo)
	}
	if _, total := s.Rows(); total != len(whole) {
		t.Fatalf("read through, the rows add up to %d, not %d", total, len(whole))
	}
}

// However long the session, only so many turns' drawings are kept.
func TestRenderWindowKeepsBoundedDrawings(t *testing.T) {
	s := benchSession(1200)
	o := Options{Width: 120, Now: at(100000), Open: map[string]bool{}, History: HistoryOpen}
	var out []Line
	for to := 1 << 30; ; {
		out = s.RenderWindow(o, max(0, to-200), to, out)
		lo, _ := s.WindowTurns()
		if lo == 0 {
			break
		}
		to = s.TurnRow(lo)
	}
	if _, total := s.Rows(); total < keepRows*3/2 {
		t.Fatalf("the session is only %d rows: too short to show a bound", total)
	}
	if s.index.drawn.Rows() > keepRows+500 || len(s.cache) > s.index.drawn.Len() {
		t.Fatalf("%d rows of %d turns kept, past the bound of %d", s.index.drawn.Rows(), len(s.cache), keepRows)
	}
}
