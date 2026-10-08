package ui

import (
	"fmt"
	"math/rand"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"

	uv "github.com/charmbracelet/ultraviolet"
	"github.com/mattn/go-runewidth"
)

// oldWrapSegs is the first wrapSegs, which measured every row's remainder
// again: the oracle for the one that measures once.
func oldWrapSegs(text []rune, w int) []seg {
	w = max(4, w)
	width := func(r []rune) int {
		n := 0
		for _, c := range r {
			n += runewidth.RuneWidth(c)
		}
		return n
	}
	var out []seg
	start := 0
	for start <= len(text) {
		end := start
		for end < len(text) && text[end] != '\n' {
			end++
		}
		line := text[start:end]
		from := 0
		for {
			if width(line[from:]) <= w {
				out = append(out, seg{start + from, start + len(line)})
				break
			}
			cut, wd := from, 0
			for cut < len(line) && wd+runewidth.RuneWidth(line[cut]) <= w {
				wd += runewidth.RuneWidth(line[cut])
				cut++
			}
			brk := cut
			for i := cut; i > from; i-- {
				if line[i-1] == ' ' {
					brk = i
					break
				}
			}
			out = append(out, seg{start + from, start + brk})
			from = brk
		}
		if end >= len(text) {
			break
		}
		start = end + 1
	}
	return out
}

// oldContent drew the box's rows with the colour before every character.
func oldContent(b box, w int) []string {
	lw := b.leadW()
	segs := b.segs()
	start, end := b.window(segs)
	from, to := -1, -1
	if b.anchor >= 0 && b.anchor != b.cursor {
		from, to = min(b.anchor, b.cursor), max(b.anchor, b.cursor)
	}
	var rows []string
	for i := start; i < end; i++ {
		sg := segs[i]
		var sb strings.Builder
		for p := sg.from; p < sg.to; p++ {
			ch := string(b.text[p])
			switch {
			case b.focused && p == b.cursor:
				sb.WriteString(reverse(ch))
			case p >= from && p < to:
				sb.WriteString(bgMark + cText + ch + reset + bgInput)
			default:
				sb.WriteString(cText + ch)
			}
		}
		last := i == len(segs)-1 || segs[i+1].from > sg.to
		if b.focused && b.cursor == sg.to && last {
			sb.WriteString(reverse(" "))
		}
		lead := strings.Repeat(" ", lw)
		if i == 0 {
			lead = b.lead
		}
		rows = append(rows, lead+sb.String()+reset)
	}
	return rows
}

// cellsOf draws styled text into a screen, for comparing what shows.
func cellsOf(s string, w int) string {
	buf := uv.NewScreenBuffer(w, 1)
	uv.NewStyledString(s).Draw(buf, uv.Rect(0, 0, w, 1))
	var b strings.Builder
	for x := 0; x < w; x++ {
		if c := buf.CellAt(x, 0); c != nil {
			b.WriteString(c.Content + "|" + c.Style.String() + "\n")
		}
	}
	return b.String()
}

func TestBoxDrawsTheSame(t *testing.T) {
	pieces := []string{"a", "word ", " ", "  ", "\n", "中文", "é", "👍", "\t", "longwordwithoutspaces", "-", "x"}
	r := rand.New(rand.NewSource(3))
	for n := 0; n < 20000; n++ {
		var sb strings.Builder
		for k := r.Intn(40); k >= 0; k-- {
			sb.WriteString(pieces[r.Intn(len(pieces))])
		}
		text := []rune(sb.String())
		w := 4 + r.Intn(60)
		got, want := wrapSegs(text, w), oldWrapSegs(text, w)
		if len(got) != len(want) {
			t.Fatalf("%q at %d: %v, want %v", string(text), w, got, want)
		}
		for i := range got {
			if got[i] != want[i] {
				t.Fatalf("%q at %d: %v, want %v", string(text), w, got, want)
			}
		}
		if len(text) == 0 {
			continue
		}
		b := box{w: w + 8, focused: r.Intn(2) == 0, text: text, cursor: r.Intn(len(text) + 1), anchor: r.Intn(len(text)+2) - 1,
			lead: paint(cOrange, "❯ "), maxRows: 1 + r.Intn(6)}
		inner := max(20, b.w) - 4
		g, o := b.content(inner), oldContent(b, inner)
		if len(g) != len(o) {
			t.Fatalf("rows %d, want %d", len(g), len(o))
		}
		for i := range g {
			if cellsOf(g[i], inner+2) != cellsOf(o[i], inner+2) {
				t.Fatalf("row %d of %q differs:\n%q\n%q", i, string(text), g[i], o[i])
			}
		}
	}
}

// A paste chip in a box is marked rune for rune, and nothing else is.
func TestChipMask(t *testing.T) {
	text := []rune("héllo [Pasted text #1 +30 lines] bye")
	m := chipMask(text)
	start := len([]rune("héllo "))
	for i := range text {
		want := i >= start && i < start+len("[Pasted text #1 +30 lines]")
		if m[i] != want {
			t.Fatalf("mask[%d] (%q) = %v, want %v", i, text[i], m[i], want)
		}
	}
	if chipMask([]rune("no chips here")) != nil {
		t.Fatal("a box without chips has no mask")
	}
}

// Every line of a box is exactly its width, with the cursor after a row
// that fills it and edge labels too long to fit.
func TestBoxStaysInItsEdge(t *testing.T) {
	for n := 0; n < 90; n++ {
		for _, lead := range []string{"", "❯ "} {
			text := []rune(strings.Repeat("x", n))
			b := box{w: 40, focused: true, text: text, cursor: len(text), anchor: -1, lead: lead, maxRows: 5,
				topL: strings.Repeat("label ", n/3), topR: "right", footR: strings.Repeat("r", n/2), holder: strings.Repeat("hold ", n)}
			for i, l := range b.lines() {
				if w := cellw.String(l); w != 40 {
					t.Fatalf("%d x's, lead %q: line %d is %d wide: %q", n, lead, i, w, l)
				}
			}
		}
	}
}

// The box scrolls like any text area: the cursor moves within the rows on
// screen, and the rows only move when the cursor would leave them.
func TestBoxScrollsOnlyToKeepTheCursorInView(t *testing.T) {
	var lines []string
	for i := 1; i <= 12; i++ {
		lines = append(lines, fmt.Sprintf("row%02d", i))
	}
	text := []rune(strings.Join(lines, "\n"))
	b := box{w: 80, text: text, cursor: len(text), maxRows: 6}
	row := func() int { // the cursor's line
		return strings.Count(string(text[:b.cursor]), "\n")
	}
	move := func(d int) {
		n := row() + d
		b.cursor = strings.Index(string(text), lines[n])
		b = b.scrolled()
	}
	want := func(step string, first int) {
		t.Helper()
		if start, end := b.window(b.segs()); start != first || end != first+6 {
			t.Fatalf("%s: rows %d..%d on screen, want %d..%d", step, start, end, first, first+6)
		}
	}
	b = b.scrolled()
	want("typed", 6)
	for i := 1; i <= 5; i++ {
		move(-1)
		want(fmt.Sprintf("up %d", i), 6)
	}
	move(-1)
	want("up past the top row", 5)
	for i := 1; i <= 5; i++ {
		move(1)
		want(fmt.Sprintf("down %d", i), 5)
	}
	move(1)
	want("down past the bottom row", 6)
	b.cursor = 0
	b = b.scrolled()
	want("to the start", 0)
}
