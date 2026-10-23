package ui

import (
	"cmp"
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Tabular styles: a table whose header a click sorts by, keys and their
// values, a checklist, a log, and things before and after.

func init() {
	blockStyles["table"] = blockTable
	blockStyles["kv"] = blockKV
	blockStyles["checklist"] = blockChecklist
	blockStyles["log"] = blockLog
	blockStyles["compare"] = blockCompare
}

// numeric is whether a cell reads as a number, to sit right.
func numeric(s string) bool {
	s = strings.TrimSpace(strings.TrimLeft(s, "$€£+-−~"))
	s = strings.TrimRight(s, "%kKmMgGbBsx ")
	_, err := strconv.ParseFloat(strings.ReplaceAll(s, ",", ""), 64)
	return err == nil && s != ""
}

// cellNum is a cell's number, to sort by; ok false when it isn't one.
func cellNum(s string) (float64, bool) {
	if !numeric(s) {
		return 0, false
	}
	s = strings.TrimSpace(strings.TrimLeft(s, "$€£+~"))
	s = strings.Replace(s, "−", "-", 1)
	s = strings.TrimRight(s, "%kKmMgGbBsx ")
	f, err := strconv.ParseFloat(strings.ReplaceAll(s, ",", ""), 64)
	return f, err == nil
}

// blockTable is columns over rows, each column as wide as it needs, the
// widest cut to fit; numbers sit right. A click on the header sorts by
// the column under it, again the other way.
func blockTable(b *host.Block, w int, cx blockCtx) []viewLine {
	n := len(b.Columns)
	for _, it := range b.Items {
		n = max(n, len(it.Cells))
	}
	if n == 0 {
		return nil
	}
	gutter := 0
	if slices.ContainsFunc(b.Items, func(it host.Item) bool { return it.Tone != "" }) {
		gutter = 2
	}
	widths := make([]int, n)
	right := make([]bool, n)
	for c := range n {
		if c < len(b.Columns) {
			widths[c] = cellw.String(b.Columns[c])
		}
		right[c] = c > 0
		for _, it := range b.Items {
			if c < len(it.Cells) {
				widths[c] = max(widths[c], cellw.String(it.Cells[c]))
				if it.Cells[c] != "" && !numeric(it.Cells[c]) {
					right[c] = false
				}
			}
		}
	}
	// Cut the widest until the row fits, two spaces between columns.
	for room := w - gutter - 2*(n-1); sum(widths) > room; {
		i := 0
		for c := range widths {
			if widths[c] > widths[i] {
				i = c
			}
		}
		if widths[i] <= 3 {
			break
		}
		widths[i]--
	}
	cell := func(s string, c int) string {
		s = cellwTrunc(s, widths[c])
		if right[c] {
			return blanks(widths[c]-cellw.String(s)) + s
		}
		return s + blanks(widths[c]-cellw.String(s))
	}
	starts := make([]int, n)
	at := 2 + gutter // the panel's edge and its space
	for c := range n {
		starts[c] = at
		at += widths[c] + 2
	}
	rows := slices.Clone(b.Items)
	sortBy := cx.st.sorts[cx.key]
	if sortBy != 0 {
		c, desc := abs(sortBy)-1, sortBy < 0
		slices.SortStableFunc(rows, func(x, y host.Item) int {
			a, bb := cellAt(x.Cells, c), cellAt(y.Cells, c)
			r := 0
			if fa, ok := cellNum(a); ok {
				if fb, ok := cellNum(bb); ok {
					r = cmp.Compare(fa, fb)
				}
			} else {
				r = strings.Compare(strings.ToLower(a), strings.ToLower(bb))
			}
			if desc {
				r = -r
			}
			return r
		})
	}
	var out []viewLine
	if len(b.Columns) > 0 {
		var h strings.Builder
		h.WriteString(blanks(gutter))
		for c := range n {
			if c > 0 {
				h.WriteString("  ")
			}
			name := ""
			if c < len(b.Columns) {
				name = b.Columns[c]
			}
			col := cSub
			if abs(sortBy)-1 == c && sortBy != 0 {
				col = cBright
				arrow := "↑"
				if sortBy < 0 {
					arrow = "↓"
				}
				name = cellwTrunc(name, widths[c]-1) + arrow
			}
			h.WriteString(paint(col+bold, cell(name, c)))
		}
		out = append(out, viewLine{h.String(), viewAct{sort: cx.key, cols: starts}})
		out = append(out, viewLine{s: faint(strings.Repeat("─", min(w, at-4)))})
	}
	for _, it := range rows {
		var r strings.Builder
		if gutter > 0 {
			mark, col := toneMark(it.Tone)
			if it.Tone == "" {
				mark = " "
			}
			r.WriteString(paint(col, mark) + " ")
		}
		for c := range n {
			if c > 0 {
				r.WriteString("  ")
			}
			col := cSub
			if c == 0 {
				col = cText
			}
			r.WriteString(paint(col, cell(cellAt(it.Cells, c), c)))
		}
		s := r.String()
		if it.Link != "" {
			s = link(it.Link, s)
		}
		out = append(out, viewLine{s, viewAct{link: it.Link}})
	}
	return out
}

func cellAt(cells []string, c int) string {
	if c < len(cells) {
		return cells[c]
	}
	return ""
}

func sum(xs []int) int {
	t := 0
	for _, x := range xs {
		t += x
	}
	return t
}

// blockKV is keys down the left, quiet, and their values beside them,
// in their tone.
func blockKV(b *host.Block, w int, cx blockCtx) []viewLine {
	keyW := 0
	for _, it := range b.Items {
		keyW = max(keyW, cellw.String(it.Text))
	}
	keyW = min(keyW, w*2/5)
	var out []viewLine
	for _, it := range b.Items {
		col := cText
		if it.Tone != "" {
			col = toneEdge(it.Tone)
		}
		v := paint(col, cellwTrunc(it.Meta, w-keyW-2))
		if it.Link != "" {
			v = link(it.Link, v) + paint(cSub, " ↗")
		}
		out = append(out, viewLine{paint(cSub, fit(cellwTrunc(it.Text, keyW), keyW)) + "  " + v, viewAct{link: it.Link}})
	}
	return out
}

// blockChecklist is things to do, ticked when done and quieter for it,
// with how many are done along its foot.
func blockChecklist(b *host.Block, w int, cx blockCtx) []viewLine {
	var out []viewLine
	done := 0
	for _, it := range b.Items {
		mark, text := paint(cSub, "○"), paint(cText, it.Text)
		if it.Done != nil && *it.Done {
			done++
			mark, text = paint(cGreen, "✓"), dim(it.Text)
		} else if it.Tone != "" {
			m, col := toneMark(it.Tone)
			mark = paint(col, m)
		}
		meta := dim(it.Meta)
		room := w - 2 - cellw.String(meta) - 1
		line := mark + " " + fit(cellwTrunc(text, room), room) + " " + meta
		out = append(out, viewLine{line, viewAct{link: it.Link}})
	}
	if len(b.Items) > 2 {
		frac := float64(done) / float64(len(b.Items))
		words := " " + strconv.Itoa(done) + " of " + strconv.Itoa(len(b.Items))
		out = append(out, viewLine{s: progressBar(frac, w-cellw.String(words), cGreen) + dim(words)})
	}
	return out
}

// blockLog is lines as a program wrote them, each after its time, the
// bad and the warnings in their colour.
func blockLog(b *host.Block, w int, cx blockCtx) []viewLine {
	tw := 0
	for _, it := range b.Items {
		tw = max(tw, cellw.String(it.Time))
	}
	var out []viewLine
	for _, it := range b.Items {
		col := cSub
		switch it.Tone {
		case "bad", "warn", "good", "busy":
			col = toneEdge(it.Tone)
		case "":
			col = cText
		}
		line, room := "", w
		if tw > 0 {
			line, room = faint(fit(it.Time, tw))+" ", w-tw-1
		}
		line += paint(col, cellwTrunc(it.Text, room))
		out = append(out, viewLine{line, viewAct{link: it.Link}})
	}
	return out
}

// blockCompare is things before and after: what it is, then the old
// quiet and the new bright in its tone.
func blockCompare(b *host.Block, w int, cx blockCtx) []viewLine {
	fromW, toW := 0, 0
	for _, it := range b.Items {
		fromW, toW = max(fromW, cellw.String(it.From)), max(toW, cellw.String(it.To))
	}
	var out []viewLine
	for _, it := range b.Items {
		col := cBright
		if it.Tone != "" {
			col = toneEdge(it.Tone)
		}
		right := dim(blanks(fromW-cellw.String(it.From))+it.From) + faint(" → ") + paint(col+bold, blanks(toW-cellw.String(it.To))+it.To)
		room := w - cellw.String(right) - 1
		out = append(out, viewLine{paint(cText, fit(cellwTrunc(it.Text, room), room)) + " " + right, viewAct{link: it.Link}})
	}
	return out
}
