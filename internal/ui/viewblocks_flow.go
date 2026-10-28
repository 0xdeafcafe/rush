package ui

import (
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Flow styles: steps of a pipeline left to right, a timeline down, a
// board of columns of cards, and a tree whose branches a click folds.

func init() {
	blockStyles["steps"] = blockSteps
	blockStyles["timeline"] = blockTimeline
	blockStyles["board"] = blockBoard
	blockStyles["tree"] = blockTree
}

// stepMark is a step's mark and colour: done, now, failed, to come.
func stepMark(tone string) (string, string) {
	switch tone {
	case "good":
		return "●", cGreen
	case "busy":
		return "◉", cBlue
	case "bad":
		return "✗", cRed
	case "warn":
		return "▲", cYellow
	}
	return "○", cDim
}

// blockSteps is a pipeline: its steps left to right joined by a line,
// solid as far as they've gone, each one's word under it; down the panel
// when they don't fit across.
func blockSteps(b *host.Block, w int, cx blockCtx) []viewLine {
	its := b.Items
	if len(its) == 0 {
		return nil
	}
	const join = 4 // " ━━ "
	across := 0
	for _, it := range its {
		across += 2 + cellw.String(it.Text)
	}
	across += join * (len(its) - 1)
	if across <= w {
		var top, under strings.Builder
		used := 0
		for i, it := range its {
			mark, col := stepMark(it.Tone)
			if i > 0 {
				line := faint(" ── ")
				if its[i-1].Tone == "good" {
					line = " " + paint(cGreen, "━━") + " "
				}
				top.WriteString(line)
				under.WriteString(blanks(join))
				used += join
			}
			name := paint(col, mark) + " " + paint(cText, it.Text)
			if it.Tone == "" {
				name = paint(col, mark) + " " + dim(it.Text)
			}
			top.WriteString(name)
			nw := 2 + cellw.String(it.Text)
			// A step's word goes under it, as far as the next one starts.
			room := nw + join
			if i == len(its)-1 {
				room = w - used
			}
			under.WriteString(fit(dim(cellwTrunc(it.Meta, room-1)), nw))
			used += nw
		}
		out := []viewLine{{s: top.String()}}
		if strings.TrimSpace(under.String()) != "" {
			out = append(out, viewLine{s: under.String()})
		}
		return out
	}
	var out []viewLine
	under := slices.ContainsFunc(its, func(it host.Item) bool { return it.Meta != "" && !metaFits(it.Text, it.Meta, false, w) })
	for i, it := range its {
		mark, col := stepMark(it.Tone)
		lines := []string{paint(col, mark) + " " + paint(cText, cellwTrunc(it.Text, w-2))}
		if under {
			lines = append(lines, metaLines(it.Meta, w)...)
		} else if it.Meta != "" {
			lines[0] = spread(lines[0], dim(it.Meta), w)
		}
		out = append(out, cx.rowLines(openRow(cx.prefix, &its[i]), w, lines...)...)
		if i < len(its)-1 {
			edge := faint("│")
			if it.Tone == "good" {
				edge = paint(cGreen, "┃")
			}
			out = append(out, viewLine{s: edge})
		}
	}
	return out
}

// blockTimeline is events down a line, each at its time with its mark,
// its word under it beside the line.
func blockTimeline(b *host.Block, w int, cx blockCtx) []viewLine {
	tw := 0
	for _, it := range b.Items {
		tw = max(tw, cellw.String(it.Time))
	}
	var out []viewLine
	for i, it := range b.Items {
		mark, col := toneMark(it.Tone)
		if it.Tone == "" {
			mark, col = "●", cx.accent
		}
		room := w - tw - 3
		text := paint(cText, cellwTrunc(it.Text, room))
		if it.Link != "" {
			text = link(it.Link, text)
		}
		out = append(out, cx.rowLines(openRow(cx.prefix, &b.Items[i]), w, dim(fit(it.Time, tw))+" "+paint(col, mark)+" "+text)...)
		last := i == len(b.Items)-1
		if it.Meta != "" {
			edge := faint("│")
			if last {
				edge = " "
			}
			out = append(out, viewLine{s: blanks(tw) + " " + edge + " " + dim(cellwTrunc(it.Meta, room))})
		}
		if !last {
			out = append(out, viewLine{s: blanks(tw) + " " + faint("│")})
		}
	}
	return out
}

// blockBoard is columns of cards side by side, each column's title over
// a rule in its tone, each card its mark, its words and a quiet line;
// one column under another when four won't fit across.
func blockBoard(b *host.Block, w int, cx blockCtx) []viewLine {
	n := len(b.Items)
	if n == 0 {
		return nil
	}
	cw := (w - (n - 1)) / n
	if n > 4 || cw < 12 {
		var out []viewLine
		for i := range b.Items {
			col := &b.Items[i]
			if i > 0 {
				out = append(out, viewLine{})
			}
			for _, l := range boardHead(col, w) {
				out = append(out, viewLine{s: l})
			}
			for j := range col.Items {
				card := &col.Items[j]
				var ls []string
				for _, l := range boardCard(*card, w) {
					ls = append(ls, l.s)
				}
				o := openRow(host.ItemKey(cx.prefix, col.ID, col.Text, nil), card)
				o.cut = len(wrap(card.Text, max(8, w-2))) > 2
				out = append(out, cx.rowLines(o, w, ls...)...)
			}
		}
		return out
	}
	cols := make([][]string, n)
	for i := range b.Items {
		col := &b.Items[i]
		width := cw
		if i == n-1 {
			width = w - (cw+1)*(n-1)
		}
		cols[i] = append(cols[i], boardHead(col, width)...)
		for j, card := range col.Items {
			if j > 0 {
				cols[i] = append(cols[i], "")
			}
			for _, l := range boardCard(card, width) {
				cols[i] = append(cols[i], l.s)
			}
		}
	}
	h := 0
	for _, c := range cols {
		h = max(h, len(c))
	}
	out := make([]viewLine, h)
	for y := range h {
		var line strings.Builder
		for i, c := range cols {
			width := cw
			if i == n-1 {
				width = w - (cw+1)*(n-1)
			}
			if i > 0 {
				line.WriteString(" ")
			}
			s := ""
			if y < len(c) {
				s = c[y]
			}
			line.WriteString(fit(s, width))
		}
		out[y] = viewLine{s: line.String()}
	}
	return out
}

// boardHead is a board column's title and count over a rule in its tone.
func boardHead(col *host.Item, w int) []string {
	c := cSub
	if col.Tone != "" {
		c = toneEdge(col.Tone)
	}
	count := " " + strconv.Itoa(len(col.Items))
	return []string{paint(c+bold, cellwTrunc(col.Text, w-len(count))) + dim(count), paint(c, strings.Repeat("▔", w))}
}

// boardCard is a card w wide: its mark and up to two lines of its words,
// then its quiet line.
func boardCard(card host.Item, w int) []viewLine {
	mark, col := toneMark(card.Tone)
	lines := wrap(card.Text, max(8, w-2))
	if len(lines) > 2 {
		lines = append(lines[:1], cellwTrunc(strings.Join(lines[1:], " "), w-2))
	}
	var out []viewLine
	for i, l := range lines {
		lead := "  "
		if i == 0 {
			lead = paint(col, mark) + " "
		}
		out = append(out, viewLine{lead + paint(cText, l), viewAct{link: card.Link}})
	}
	if card.Meta != "" {
		out = append(out, viewLine{"  " + dim(cellwTrunc(card.Meta, w-2)), viewAct{link: card.Link}})
	}
	return out
}

// blockTree is nodes under their parents along guides, each with its
// mark and its word at the end; a click on a node with children folds
// them, and a node can start folded (tone quiet does).
func blockTree(b *host.Block, w int, cx blockCtx) []viewLine {
	var out []viewLine
	var walk func(items []host.Item, lead, path, prefix string)
	walk = func(items []host.Item, lead, path, prefix string) {
		for i := range items {
			it := &items[i]
			last := i == len(items)-1
			branch, next := "├─ ", "│  "
			if last {
				branch, next = "└─ ", "   "
			}
			if path == "" { // the top: no guide
				branch, next = "", ""
			}
			key := cx.key + "/t" + path + strconv.Itoa(i)
			folded := cx.st.fold[key]
			mark, col := toneMark(it.Tone)
			act := viewAct{link: it.Link}
			if len(it.Items) > 0 {
				mark, col = "▾", cSub
				if folded {
					mark = "▸"
				}
				if it.Tone != "" {
					col = toneEdge(it.Tone)
				}
				act = viewAct{fold: key, folded: folded}
			}
			o := openRow(prefix, it)
			meta := dim(it.Meta)
			pre := faint(lead+branch) + paint(col, mark) + " "
			room := w - cellw.String(pre) - cellw.String(meta) - 1
			text := cText
			if len(it.Items) > 0 {
				text = cText + bold
			}
			line := pre + fit(paint(text, cellwTrunc(it.Text, room)), room) + " " + meta
			if act.fold != "" {
				out = append(out, viewLine{line, act}) // a click folds it
			} else {
				out = append(out, cx.rowLines(o, w, line)...)
			}
			if len(it.Items) > 0 && !folded {
				walk(it.Items, lead+next, path+strconv.Itoa(i)+".", o.key)
			}
		}
	}
	walk(b.Items, "", "", cx.prefix)
	return out
}
