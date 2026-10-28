package ui

import (
	"strconv"

	"github.com/0xdeafcafe/rush/internal/host"
)

// A view's blocks are its parts in styles of their own — tabular (a
// table, keys and values, a checklist, a log), visual (bars, a chart, a
// heatmap, before and after), flow (a pipeline of steps, a timeline, a
// board, a tree) and expressive (a hero number, a callout, code) — each
// under its title, a rule a click folds.

// blockCtx is what a block is drawn with: the view's accent, for what has
// no tone, and the panel's state, for what's folded.
type blockCtx struct {
	accent string // a colour
	st     *viewsState
	key    string // the block's, for its folds: view/b<index>
	view   string // the view's name
	prefix string // its rows' keys start so (host.BlockKey)
}

// blockDraw draws one style of block w wide.
type blockDraw func(b *host.Block, w int, cx blockCtx) []viewLine

// blockStyles are the styles drawn in a file of their own each.
var blockStyles = map[string]blockDraw{}

// viewBlock is a block w wide: its title as a group's rule, a click
// folding it, then its body in its style.
func viewBlock(v *host.View, bi int, w int, st *viewsState, accent string) []viewLine {
	b := &v.Blocks[bi]
	key := v.Name + "/b" + strconv.Itoa(bi)
	var out []viewLine
	folded := b.Folded
	if f, ok := st.fold[key]; ok {
		folded = f
	}
	if b.Title != "" {
		n := len(b.Items)
		if b.Kind == "code" || b.Kind == "callout" || b.Kind == "text" || b.Kind == "hero" || b.Kind == "chart" {
			n = -1 // no count
		}
		out = append(out, viewLine{viewRuleHead(b.Title, b.Tone, n, folded, w), viewAct{fold: key, folded: folded}})
		if folded {
			return out
		}
	}
	cx := blockCtx{accent: accent, st: st, key: key, view: v.Name, prefix: host.BlockKey(bi)}
	switch b.Kind {
	case "list":
		for i := range b.Items {
			r := itemRow(&b.Items[i])
			out = append(out, cx.rowLines(openRow(cx.prefix, &b.Items[i]), w, viewRow(&r, w)...)...)
		}
	case "bars":
		rows := make([]host.Row, len(b.Items))
		for i := range b.Items {
			rows[i] = itemRow(&b.Items[i])
		}
		bars := viewBars(rows, w, accent)
		for i := range rows {
			if rows[i].Value != nil {
				out = append(out, cx.rowLines(openRow(cx.prefix, &b.Items[i]), w, bars(&rows[i]))...)
			}
		}
	case "stats":
		for _, s := range viewTiles(b.Stats, w) {
			out = append(out, viewLine{s: s})
		}
	case "progress":
		for _, s := range viewHead(&host.View{Progress: b.Progress}, w, accent) {
			out = append(out, viewLine{s: s})
		}
	default:
		if draw := blockStyles[b.Kind]; draw != nil {
			out = append(out, draw(b, w, cx)...)
		}
	}
	return out
}

// itemRow is an item as a group's row.
func itemRow(it *host.Item) host.Row {
	return host.Row{Text: it.Text, Meta: it.Meta, Tone: it.Tone, Link: it.Link, Progress: it.Progress, Value: it.Value}
}
