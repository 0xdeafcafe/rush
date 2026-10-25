package ui

import (
	"math"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Visual and expressive styles: a chart of columns or a line, a heatmap,
// a hero number drawn big, a callout, code and words.

func init() {
	blockStyles["chart"] = blockChart
	blockStyles["heatmap"] = blockHeatmap
	blockStyles["hero"] = blockHero
	blockStyles["callout"] = blockCallout
	blockStyles["code"] = blockCode
	blockStyles["text"] = blockText
}

const chartRows = 6

// blockChart is values as columns, or a line, chartRows tall, its most
// at its top left and its labels, first, middle and last, under it.
func blockChart(b *host.Block, w int, cx blockCtx) []viewLine {
	vs := b.Values
	if len(vs) == 0 {
		return nil
	}
	lo, hi := 0.0, vs[0]
	for _, v := range vs {
		lo, hi = min(lo, v), max(hi, v)
	}
	if b.Style == "line" {
		lo = vs[0]
		for _, v := range vs {
			lo = min(lo, v)
		}
	}
	col := accentOr(b.Tone, cx.accent)
	axis := max(cellw.String(num(round2(hi))), cellw.String(num(round2(lo))))
	plotW := w - axis - 2
	var rows []string
	if b.Style == "line" {
		rows = lineChart(vs, lo, hi, plotW, chartRows)
	} else {
		rows = columnChart(vs, lo, hi, plotW, chartRows)
	}
	var out []viewLine
	for y, r := range rows {
		label := ""
		switch y {
		case 0:
			label = num(round2(hi))
		case len(rows) - 1:
			label = num(round2(lo))
		}
		out = append(out, viewLine{s: dim(blanks(axis-cellw.String(label))+label) + " " + faint("┤") + paint(col, r)})
	}
	out = append(out, viewLine{s: blanks(axis+1) + faint("└"+strings.Repeat("─", plotW))})
	if ls := b.Labels; len(ls) > 0 {
		first, last := ls[0], ls[len(ls)-1]
		mid := ""
		if len(ls) > 2 {
			mid = ls[len(ls)/2]
		}
		line := spread(dim(first), dim(last), plotW)
		if mid != "" && plotW > cellw.String(first)+cellw.String(mid)+cellw.String(last)+4 {
			at := plotW/2 - cellw.String(mid)/2
			pre := dim(first) + blanks(at-cellw.String(first))
			line = pre + dim(mid) + blanks(plotW-at-cellw.String(mid)-cellw.String(last)) + dim(last)
		}
		out = append(out, viewLine{s: blanks(axis+2) + line})
	}
	return out
}

func round2(f float64) float64 { return math.Round(f*100) / 100 }

// columnChart is values as columns h rows tall in w cells, eighths of a
// cell at their tops, spread to fill w.
func columnChart(vs []float64, lo, hi float64, w, h int) []string {
	vs = vs[max(0, len(vs)-w):]
	n := len(vs)
	cw := max(1, w/n)
	gap := 0
	if cw >= 3 {
		gap = 1
	}
	cells := make([][]rune, h)
	for y := range cells {
		cells[y] = []rune(strings.Repeat(" ", w))
	}
	eighths := []rune(" ▁▂▃▄▅▆▇█")
	for i, v := range vs {
		frac := 0.0
		if hi > lo {
			frac = (v - lo) / (hi - lo)
		}
		units := int(frac*float64(h*8) + 0.5)
		if v > lo && units == 0 {
			units = 1
		}
		for y := range h {
			from := (h - 1 - y) * 8
			fill := min(8, max(0, units-from))
			for x := i * cw; x < i*cw+cw-gap && x < w; x++ {
				cells[y][x] = eighths[fill]
			}
		}
	}
	out := make([]string, h)
	for y := range cells {
		out[y] = string(cells[y])
	}
	return out
}

// lineChart is values as a line h rows tall in w cells, drawn in braille
// dots: two across and four down a cell.
func lineChart(vs []float64, lo, hi float64, w, h int) []string {
	dw, dh := w*2, h*4
	dots := make([][]bool, dh)
	for y := range dots {
		dots[y] = make([]bool, dw)
	}
	pt := func(i int) (int, int) {
		x := 0
		if len(vs) > 1 {
			x = i * (dw - 1) / (len(vs) - 1)
		}
		frac := 0.5
		if hi > lo {
			frac = (vs[i] - lo) / (hi - lo)
		}
		return x, dh - 1 - int(frac*float64(dh-1)+0.5)
	}
	for i := range vs {
		x0, y0 := pt(i)
		dots[y0][x0] = true
		if i == 0 {
			continue
		}
		x1, y1 := pt(i - 1)
		steps := max(abs(x0-x1), abs(y0-y1))
		for s := 1; s < steps; s++ {
			x := x1 + (x0-x1)*s/steps
			y := y1 + (y0-y1)*s/steps
			dots[y][x] = true
		}
	}
	bits := [4][2]rune{{0x01, 0x08}, {0x02, 0x10}, {0x04, 0x20}, {0x40, 0x80}}
	out := make([]string, h)
	for cy := range h {
		var b strings.Builder
		for cx := range w {
			r := rune(0)
			for dy := range 4 {
				for dx := range 2 {
					if dots[cy*4+dy][cx*2+dx] {
						r |= bits[dy][dx]
					}
				}
			}
			if r == 0 {
				b.WriteRune(' ')
			} else {
				b.WriteRune(0x2800 + r)
			}
		}
		out[cy] = b.String()
	}
	return out
}

// blockHeatmap is a grid: a row a thing, a cell a value, denser and
// brighter the more it is, its columns' labels over it when they fit and
// a key under it.
func blockHeatmap(b *host.Block, w int, cx blockCtx) []viewLine {
	lw, n, hi := 0, 0, 0.0
	for _, it := range b.Items {
		lw = max(lw, cellw.String(it.Text))
		n = max(n, len(it.Values))
		for _, v := range it.Values {
			hi = max(hi, v)
		}
	}
	if n == 0 {
		return nil
	}
	lw = min(lw, w/3)
	cw := 2
	if lw+1+n*3 <= w {
		cw = 3
	}
	n = min(n, (w-lw-1)/cw)
	col := accentOr(b.Tone, cx.accent)
	shades := []string{"░", "▒", "▓", "█"}
	var out []viewLine
	if len(b.Columns) > 0 && slicesAllFit(b.Columns[:min(n, len(b.Columns))], cw-1) {
		var h strings.Builder
		h.WriteString(blanks(lw + 1))
		for c := range n {
			name := ""
			if c < len(b.Columns) {
				name = b.Columns[c]
			}
			h.WriteString(fit(dim(name), cw))
		}
		out = append(out, viewLine{s: h.String()})
	}
	for _, it := range b.Items {
		var r strings.Builder
		r.WriteString(paint(cSub, fit(cellwTrunc(it.Text, lw), lw)) + " ")
		for c := range n {
			cell := faint(strings.Repeat("·", 1)) + blanks(cw-1)
			if c < len(it.Values) && it.Values[c] > 0 && hi > 0 {
				i := min(3, int(it.Values[c]/hi*4))
				cell = paint(col, strings.Repeat(shades[i], cw-1)) + " "
			}
			r.WriteString(cell)
		}
		out = append(out, viewLine{r.String(), viewAct{link: it.Link}})
	}
	out = append(out, viewLine{s: blanks(lw+1) + dim("less ") + paint(col, "░▒▓█") + dim(" more · most "+num(round2(hi)))})
	return out
}

func slicesAllFit(ss []string, w int) bool {
	for _, s := range ss {
		if cellw.String(s) > w {
			return false
		}
	}
	return true
}

// bigGlyphs are digits and a few marks three rows tall, for a hero.
var bigGlyphs = map[rune][3]string{
	'0': {"█▀█", "█ █", "█▄█"}, '1': {"▀█ ", " █ ", "▄█▄"}, '2': {"▀▀█", "█▀▀", "█▄▄"},
	'3': {"▀▀█", " ▀█", "▄▄█"}, '4': {"█ █", "▀▀█", "  █"}, '5': {"█▀▀", "▀▀█", "▄▄█"},
	'6': {"█▀▀", "█▀█", "█▄█"}, '7': {"▀▀█", "  █", "  █"}, '8': {"█▀█", "█▀█", "█▄█"},
	'9': {"█▀█", "▀▀█", "▄▄█"}, '.': {" ", " ", "▄"}, ',': {" ", " ", "▄"}, ':': {" ", "▀", "▀"},
	'%': {"█ ▄", " ▄▀", "▀ █"}, '+': {"   ", "▄█▄", " ▀ "}, '-': {"   ", "▀▀▀", "   "},
	'−': {"   ", "▀▀▀", "   "}, '$': {"▄█▀", "▀█▄", "▀█▀"}, '/': {"  █", " █ ", "█  "},
	' ': {" ", " ", " "},
}

// bigText is s three rows tall: digits and marks drawn big, letters
// small on its last row, as units are ("212ms"). ok false when it has
// nothing to draw big.
func bigText(s string) ([3]string, bool) {
	var rows [3]strings.Builder
	big := false
	wasBig := false
	for i, r := range s {
		g, ok := bigGlyphs[r]
		// A space between big glyphs, none between a unit's letters.
		if i > 0 && (ok || wasBig) {
			for y := range rows {
				rows[y].WriteString(" ")
			}
		}
		wasBig = ok
		if !ok {
			w := cellw.String(string(r))
			rows[0].WriteString(blanks(w))
			rows[1].WriteString(blanks(w))
			rows[2].WriteRune(r)
			continue
		}
		big = big || r >= '0' && r <= '9'
		for y := range rows {
			rows[y].WriteString(g[y])
		}
	}
	return [3]string{rows[0].String(), rows[1].String(), rows[2].String()}, big
}

// blockHero is one number drawn big in its tone, what it is beside it,
// and its trend under both.
func blockHero(b *host.Block, w int, cx blockCtx) []viewLine {
	col := accentOr(b.Tone, cx.accent)
	if b.Tone == "quiet" {
		col = cBright
	}
	big, ok := bigText(b.Value)
	var out []viewLine
	if ok && cellw.String(big[0]) <= w {
		bw := cellw.String(big[0])
		words := wrap(b.Text, max(8, w-bw-3))
		for y := range 3 {
			line := paint(col+bold, big[y])
			if y < len(words) && bw+3+cellw.String(words[y]) <= w {
				line = fit(line, bw) + "   " + paint(cSub, words[y])
			}
			out = append(out, viewLine{s: line})
		}
	} else {
		out = append(out, viewLine{s: paint(col+bold, cellwTrunc(b.Value, w))})
		for _, l := range wrap(b.Text, w) {
			out = append(out, viewLine{s: paint(cSub, l)})
		}
	}
	if len(b.Trend) > 1 {
		out = append(out, viewLine{s: spark(stretch(b.Trend, w), w, col)})
	}
	return out
}

// stretch is vs spread over n points, so a short trend fills the width.
func stretch(vs []float64, n int) []float64 {
	if len(vs) >= n || len(vs) < 2 {
		return vs
	}
	out := make([]float64, n)
	for i := range out {
		f := float64(i) * float64(len(vs)-1) / float64(n-1)
		j := int(f)
		if j >= len(vs)-1 {
			out[i] = vs[len(vs)-1]
			continue
		}
		out[i] = vs[j] + (vs[j+1]-vs[j])*(f-float64(j))
	}
	return out
}

// blockCallout is words to notice: a bar in its tone down their left and
// its mark before them.
func blockCallout(b *host.Block, w int, cx blockCtx) []viewLine {
	mark, col := toneMark(b.Tone)
	if b.Tone == "" {
		mark, col = "✦", cx.accent
	}
	var out []viewLine
	for i, l := range wrap(b.Text, w-4) {
		lead := "  "
		if i == 0 {
			lead = paint(col, mark) + " "
		}
		out = append(out, viewLine{s: paint(col, "▌") + " " + lead + paint(cText, l)})
	}
	return out
}

// blockCode is lines of code as they are, numbered quietly down a rule.
func blockCode(b *host.Block, w int, cx blockCtx) []viewLine {
	lines := strings.Split(b.Text, "\n")
	nw := len(num(float64(len(lines))))
	var out []viewLine
	for i, l := range lines {
		n := num(float64(i + 1))
		out = append(out, viewLine{s: faint(blanks(nw-len(n))+n+" │ ") + paint(cText, cellw.Truncate(l, w-nw-3, "…"))})
	}
	return out
}

// blockText is words, wrapped.
func blockText(b *host.Block, w int, cx blockCtx) []viewLine {
	var out []viewLine
	for _, l := range wrap(b.Text, w) {
		out = append(out, viewLine{s: paint(cText, l)})
	}
	return out
}
