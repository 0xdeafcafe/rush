package ui

import (
	"os/exec"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A view's rows open on a click: under the row its words in full, a
// table row's cells each by its column, its detail, and its buttons: the
// agent's, then open ↗ for its link, tick for a checklist's item, and
// copy. A button does what the agent wrote on it (host.Action); a click on
// a checklist item's mark ticks it.

// viewBtn is a button on a line of the panel: the columns it takes, and
// what it does.
type viewBtn struct {
	from, to int
	a        host.Action
}

// rowOpen is a row a click opens, and what shows under it then.
type rowOpen struct {
	key                      string // the row's, in its view (host.ItemKey)
	text, meta, detail, link string
	cols, cells              []string
	actions                  []host.Action
	tick                     *bool // a checklist item's: done or not
	cut                      bool  // its words didn't fit on its line
}

// openRow is the row of an item under prefix.
func openRow(prefix string, it *host.Item) rowOpen {
	return rowOpen{key: host.ItemKey(prefix, it.ID, it.Text, it.Cells), text: it.Text, meta: it.Meta,
		detail: it.Detail, link: it.Link, cells: it.Cells, actions: it.Actions}
}

// rowLines are a row's drawn lines with the click that opens it, and when
// it's open, what shows under it.
func (cx blockCtx) rowLines(r rowOpen, w int, lines ...string) []viewLine {
	if r.key == "" {
		out := make([]viewLine, len(lines))
		for i, s := range lines {
			out[i] = viewLine{s, viewAct{link: r.link}}
		}
		return out
	}
	key := cx.view + "|" + r.key
	open := cx.st.open[key]
	act := viewAct{open: key, opened: open, row: r.key}
	if r.tick != nil {
		act.tick, act.done = true, *r.tick
	}
	out := make([]viewLine, 0, len(lines)+4)
	for _, s := range lines {
		out = append(out, viewLine{s, act})
	}
	if open {
		out = append(out, cx.opened(r, w)...)
	}
	return out
}

// opened is what shows under an open row, along a quiet rule: its words
// in full when they were cut, its cells by their columns, its word, its
// detail, then its buttons.
func (cx blockCtx) opened(r rowOpen, w int) []viewLine {
	const lead = 2 // "│ "
	in := max(8, w-lead)
	var out []viewLine
	add := func(s string) { out = append(out, viewLine{s: faint("│ ") + s}) }
	switch {
	case len(r.cells) > 0:
		kw := 0
		for c := range r.cells {
			if c < len(r.cols) {
				kw = max(kw, cellw.String(r.cols[c]))
			}
		}
		kw = min(kw, in/3)
		for c, cell := range r.cells {
			if cell == "" {
				continue
			}
			name := ""
			if c < len(r.cols) {
				name = r.cols[c]
			}
			for j, l := range wrap(cell, max(4, in-kw-2)) {
				k := blanks(kw)
				if j == 0 {
					k = fit(cellwTrunc(name, kw), kw)
				}
				if kw == 0 {
					add(paint(cText, l))
				} else {
					add(paint(cSub, k) + "  " + paint(cText, l))
				}
			}
		}
	case r.cut || cellw.String(r.text) > in:
		for _, l := range wrap(r.text, in) {
			add(paint(cText, l))
		}
	}
	if len(r.cells) == 0 && r.meta != "" && (r.cut || cellw.String(r.meta) > in/3) {
		for _, l := range wrap(r.meta, in) {
			add(dim(l))
		}
	}
	if r.detail != "" {
		for _, para := range strings.Split(r.detail, "\n") {
			if para == "" {
				add("")
				continue
			}
			for _, l := range wrap(para, in) {
				add(paint(cText, l))
			}
		}
	}
	btns := append([]host.Action(nil), r.actions...)
	if r.tick != nil {
		done := !*r.tick
		label := "✓ tick off"
		if *r.tick {
			label = "untick"
		}
		btns = append(btns, host.Action{Label: label, Set: &host.Change{Done: &done}})
	}
	if r.link != "" && !opensAlready(r.actions, r.link) {
		btns = append(btns, host.Action{Label: "open ↗", Open: r.link})
	}
	words := r.text
	if len(r.cells) > 0 {
		words = strings.Join(filledCells(r.cells), " · ")
	}
	if words != "" {
		btns = append(btns, host.Action{Label: "copy", Copy: words})
	}
	for _, l := range btnLines(btns, in, 2+lead, cx.accent, r.key) {
		l.s = faint("│ ") + l.s
		inner := l.act.lit
		l.act.lit = func(col int) string { return faint("│ ") + inner(col) }
		out = append(out, l)
	}
	return out
}

func opensAlready(as []host.Action, link string) bool {
	for _, a := range as {
		if a.Open == link {
			return true
		}
	}
	return false
}

func filledCells(ss []string) []string {
	var out []string
	for _, s := range ss {
		if s != "" {
			out = append(out, s)
		}
	}
	return out
}

// btnLines are buttons side by side, onto another line when they don't
// fit w, the first starting at column at of the panel: each its label on
// a raised ground, in its tone or the view's accent, lit under the
// pointer. key is the row they change, "" for the view's own.
func btnLines(as []host.Action, w, at int, accent, key string) []viewLine {
	var out []viewLine
	var line []viewBtn
	used := 0
	flush := func() {
		if len(line) == 0 {
			return
		}
		bs := line
		draw := func(col int) string {
			var b strings.Builder
			x := at
			for i, bt := range bs {
				if i > 0 {
					b.WriteString(" ")
					x++
				}
				b.WriteString(blanks(bt.from - x))
				b.WriteString(viewChip(bt.a, accent, col >= bt.from && col < bt.to))
				x = bt.to
			}
			return b.String()
		}
		out = append(out, viewLine{s: draw(-1), act: viewAct{btns: bs, row: key, lit: draw}})
		line, used = nil, 0
	}
	for _, a := range as {
		cw := cellw.String(a.Label) + 2
		if used > 0 && used+1+cw > w {
			flush()
		}
		if used > 0 {
			used++
		}
		line = append(line, viewBtn{from: at + used, to: at + used + cw, a: a})
		used += cw
	}
	flush()
	return out
}

// viewChip is a button: its label on a raised ground, brighter under the
// pointer.
func viewChip(a host.Action, accent string, lit bool) string {
	col := accentOr(a.Tone, accent)
	bg := bgBtw
	if lit {
		bg = bgMark
	}
	return bg + col + bold + " " + a.Label + " " + reset
}

// viewAction does what a button says, on the row of key in the view of
// name: changes the row, opens its link, copies its words, sends the
// agent its message.
func (m *Model) viewAction(c *hostConn, name, key string, a host.Action) tea.Cmd {
	if c.sess == nil {
		return nil
	}
	id := c.sess.Info.ID
	var cmds []tea.Cmd
	if a.Set != nil && key != "" {
		if _, err := host.ChangeRow(id, name, key, *a.Set); err != nil {
			m.fail("couldn't change it: " + err.Error())
		} else {
			m.refreshViews(id)
		}
	}
	if a.Open != "" {
		go func() { _ = exec.Command("open", a.Open).Run() }()
		m.flash("opened "+strings.TrimPrefix(a.Open, "https://"), false)
	}
	if a.Copy != "" {
		cmds = append(cmds, setClipboard(a.Copy))
		m.flash("copied "+cellwTrunc(a.Copy, 40), false)
	}
	if a.Send != "" {
		cmds = append(cmds, m.sendAs(c, a.Send))
	}
	return tea.Batch(cmds...)
}
