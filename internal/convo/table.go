package convo

import (
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/charmbracelet/x/ansi"
)

var tableSep = lazyRe(`^:?-{2,}:?$`)

// How a column's cells sit in it, as its header's rule of dashes says:
// --: to the right, :-: in the middle.
const (
	alignLeft = iota
	alignRight
	alignCentre
)

// tableCells splits a markdown table's rows into their cells. head is the
// row over the rule of dashes, -1 when there's none; align is each
// column's, as that rule says.
func tableCells(rows []string) (cells [][]string, head int, align []int) {
	head = -1
	for _, r := range rows {
		r = strings.TrimSpace(r)
		r = strings.TrimSuffix(strings.TrimPrefix(r, "|"), "|")
		parts := strings.Split(r, "|")
		sep := true
		for i := range parts {
			parts[i] = strings.TrimSpace(parts[i])
			if !tableSep().MatchString(parts[i]) {
				sep = false
			}
		}
		if sep && head < 0 {
			head = len(cells) - 1
			for _, p := range parts {
				a := alignLeft
				switch {
				case strings.HasPrefix(p, ":") && strings.HasSuffix(p, ":"):
					a = alignCentre
				case strings.HasSuffix(p, ":"):
					a = alignRight
				}
				align = append(align, a)
			}
			continue
		}
		cells = append(cells, parts)
	}
	return cells, head, align
}

// table draws a markdown table in a frame of faint light lines, so the
// words lead and the lines only part them: the header bold over a double
// rule, and a rule between rows whenever one of them wraps, so you can see
// where each row ends. Cells wrap to fit, the widest column giving way
// first; when even narrow columns can't fit, each row is drawn as its
// cells one under the other, named by the header.
func (d *drawer) table(rows []string, pad string, w int, col string) {
	k := memoKey{text: strings.Join(rows, "\n"), style: "table:" + col, spine: pad, n: w, width: d.o.Width, cw: d.cw}
	if ls, ok := d.s.memoGet(k); ok {
		d.lines = append(d.lines, ls...)
		return
	}
	from := len(d.lines)
	defer func() { d.s.memoPut(k, d.lines[from:]) }()
	cells, head, align := tableCells(rows)
	cols := 0
	for _, r := range cells {
		cols = max(cols, len(r))
	}
	if cols == 0 {
		return
	}
	// Each cell as drawn: its **bold** and `code` styled, in its row's ink.
	// The header is brightest; a row's first cell, which names it, a
	// weight above the rest.
	styled := make([][]string, len(cells))
	for ri, r := range cells {
		styled[ri] = make([]string, cols)
		for i := range cols {
			v := ""
			if i < len(r) {
				v = r[i]
			}
			ink := col
			switch {
			case ri == head:
				ink = strong(col)
			case i == 0:
				ink = col + bold
			}
			styled[ri][i] = paint(ink, inline(v, ink))
		}
	}
	// A column is as wide as its widest cell, and gives way no narrower
	// than its longest word (up to a point), so words wrap whole. A frame
	// takes a rule either side of every column and a space inside each.
	width, least := make([]int, cols), make([]int, cols)
	for _, r := range styled {
		for i, c := range r {
			width[i] = max(width[i], cellw.String(c))
			for _, word := range strings.Fields(stripANSI(c)) {
				least[i] = max(least[i], min(cellw.String(word), 14))
			}
		}
	}
	for i := range least {
		least[i] = max(1, min(least[i], width[i]))
	}
	chrome := 3*cols + 1
	if sum(least)+chrome > w {
		d.tableRecords(styled, head, pad, w)
		return
	}
	// The widest column of those that can still give way, first.
	for total := sum(width) + chrome; total > w; total-- {
		i := -1
		for j := range width {
			if width[j] > least[j] && (i < 0 || width[j] > width[i]) {
				i = j
			}
		}
		width[i]--
	}
	// Every cell's lines, and whether any body row takes more than one.
	parts := make([][][]string, len(styled))
	tall := false
	for ri, r := range styled {
		parts[ri] = make([][]string, cols)
		for i, c := range r {
			parts[ri][i] = cellRows(c, width[i])
			if ri != head && len(parts[ri][i]) > 1 {
				tall = true
			}
		}
	}
	rule := func(l, m, r, fill string) {
		var b strings.Builder
		b.WriteString(l)
		for i, cw := range width {
			if i > 0 {
				b.WriteString(m)
			}
			b.WriteString(strings.Repeat(fill, cw+2))
		}
		b.WriteString(r)
		d.add("", "", pad+faint(b.String()), "")
	}
	bar := faint("│")
	rule("╭", "┬", "╮", "─")
	for ri := range styled {
		switch {
		case ri > 0 && ri-1 == head:
			rule("╞", "╪", "╡", "═")
		case ri > 0 && tall:
			rule("├", "┼", "┤", "─")
		}
		height := 1
		for _, p := range parts[ri] {
			height = max(height, len(p))
		}
		for line := range height {
			var b strings.Builder
			b.WriteString(bar)
			for i := range cols {
				v := ""
				if line < len(parts[ri][i]) {
					v = parts[ri][i][line]
				}
				gap := max(0, width[i]-cellw.String(v))
				a := alignLeft
				if i < len(align) && ri != head {
					a = align[i]
				}
				switch a {
				case alignRight:
					b.WriteString(" " + blanks(gap) + v + " ")
				case alignCentre:
					b.WriteString(" " + blanks(gap/2) + v + blanks(gap-gap/2) + " ")
				default:
					b.WriteString(" " + v + blanks(gap) + " ")
				}
				b.WriteString(bar)
			}
			d.add("", "", pad+b.String(), "")
		}
	}
	rule("╰", "┴", "╯", "─")
}

// cellRows is a styled cell wrapped to w cells, each row carrying the
// cell's style on.
func cellRows(s string, w int) []string {
	if cellw.String(s) <= w {
		return []string{s}
	}
	rows := strings.Split(strings.TrimRight(ansi.Wrap(s, max(1, w), ""), "\n"), "\n")
	for len(rows) > 1 && strings.TrimSpace(ansi.Strip(rows[len(rows)-1])) == "" {
		rows[len(rows)-2] += rows[len(rows)-1] // a row of style codes alone
		rows = rows[:len(rows)-1]
	}
	for i, r := range rows {
		rows[i] = strings.TrimRight(r, " ")
	}
	return CarryStyle(rows)
}

// tableRecords draws a table too wide for any frame: each row its cells
// one under the other, each after its column's name from the header, the
// names lined up when they're short, a faint rule between rows. A table
// without a header has each row's cells on a line, · between.
func (d *drawer) tableRecords(styled [][]string, head int, pad string, w int) {
	var names []string
	nameW := 0
	if head >= 0 {
		for _, c := range styled[head] {
			n := strings.TrimSpace(stripANSI(c))
			names = append(names, n)
			nameW = max(nameW, cellw.String(n))
		}
	}
	lined := nameW <= w/3
	first := true
	for ri, r := range styled {
		if ri == head {
			continue
		}
		if !first {
			d.add("", "", pad+faint(strings.Repeat("─", min(w, 12))), "")
		}
		first = false
		if head < 0 {
			for _, part := range wrap(strings.Join(r, faint(" · ")), max(1, w)) {
				d.add("", "", pad+part, "")
			}
			continue
		}
		for i, c := range r {
			name := ""
			if i < len(names) {
				name = names[i]
			}
			lead, hang := "", "  "
			switch {
			case lined:
				lead, hang = dim(name)+blanks(nameW-cellw.String(name)+2), blanks(nameW+2)
			case name != "":
				lead = dim(name) + " "
			}
			rows := wrap(c, max(1, w-cellw.String(hang)))
			if !lined {
				rows = wrap(lead+c, max(1, w-2))
				lead = ""
			}
			for j, part := range rows {
				if j == 0 {
					part = lead + part
				} else {
					part = hang + part
				}
				d.add("", "", pad+part, "")
				if j > 0 {
					d.wrapped()
				}
			}
		}
	}
}
