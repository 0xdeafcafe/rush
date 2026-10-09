package convo

import (
	"slices"
	"strconv"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
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

// Before a turn gone past is drawn it's counted at what it will draw: a
// turn that fanned out to 40 agents keeps a row for each, so the session's
// count, which places Home, the minimap and the pill, is near the rows drawn.
func TestCountBeforeDrawingNearsTheRows(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	s.Apply(host.Sent{Text: "fan out: one lane per module"}, at(0))
	for i := range 40 {
		id := "x" + strconv.Itoa(i)
		s.Apply(toolUse(id, "Agent", map[string]any{"description": "lane " + strconv.Itoa(i), "prompt": "review it", "subagent_type": "general-purpose", "run_in_background": true}), at(1))
		s.Apply(toolResult(id, "Async agent launched successfully.\nagentId: a"+strconv.Itoa(i), false, nil), at(1))
	}
	s.Apply(say("Launched 40 lanes."), at(2))
	s.Apply(headless.Result{Subtype: "success"}, at(2))
	for i := range 40 {
		s.Apply(headless.Message{Role: "user", Blocks: []headless.Block{{Type: "text", Text: "<task-notification><task-id>a" + strconv.Itoa(i) + "</task-id><status>completed</status><summary>Agent finished</summary></task-notification>"}}}, at(3+i))
		s.Apply(say("Lane done."), at(3+i))
		s.Apply(headless.Result{Subtype: "success"}, at(3+i))
	}
	o := Options{Width: 120, Now: at(100), Agent: "@lead"}
	counted := s.Count(o)
	drawn := len(s.RenderWindow(o, 0, 1<<30, nil))
	if d := counted - drawn; d*10 > drawn || -d*10 > drawn {
		t.Fatalf("counted %d rows before drawing, %d drawn", counted, drawn)
	}
}
