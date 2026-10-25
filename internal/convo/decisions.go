package convo

import (
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
)

// An answer that ends with a decisions block (host.DecisionsIn) draws it as
// a list rather than prose: each decision's topic and what's at stake, then
// its options one a line, the agent's pick marked.

// DecisionsRef is the ref of the decisions block in turn ref's answer: its
// rows are one thing to pick.
func DecisionsRef(turn string) string { return turn + ":dec" }

// DecisionsTurn is the turn a decisions block's ref is in.
func DecisionsTurn(ref string) (string, bool) { return strings.CutSuffix(ref, ":dec") }

// DecisionsOnCard is the Open key saying block ref's card is up: the block
// is then one quiet row, and being in Open it's in what the turn's drawing
// is kept by.
func DecisionsOnCard(ref string) string { return ref + ":card" }

// DecisionsPassed is the Open key saying block ref's card closed without
// an answer: a message went instead, or it was set aside. Its picks are
// then the agent's suggestions, not what was chosen.
func DecisionsPassed(ref string) string { return ref + ":passed" }

// answerParts is an answer split around its decisions block.
type answerParts struct {
	text          string
	before, after string
	ds            []host.Decision
}

// parts is its text split around its decisions block, read once while
// the text stays the same.
func (it *Item) parts() *answerParts {
	if p := it.split; p != nil && p.text == it.Text {
		return p
	}
	p := &answerParts{text: it.Text}
	p.before, p.ds, p.after = host.DecisionsIn(it.Text)
	it.split = p
	return p
}

// answerItem is answer for an item, its decisions block drawn as a list.
func (d *drawer) answerItem(it *Item) {
	p := it.parts()
	if len(p.ds) == 0 {
		d.answer(it.Text)
		return
	}
	d.gap()
	if b := strings.TrimSpace(p.before); b != "" {
		d.markdown(b, 2, cText, true)
		d.gap()
	}
	d.decisions(p.ds)
	if a := strings.TrimSpace(p.after); a != "" {
		d.gap()
		d.markdown(a, 2, cText, true)
	}
}

// decisions draws ds: a heading, then each decision's number and topic,
// its context, and its options as bullets, the pick ★ and green.
func (d *drawer) decisions(ds []host.Decision) {
	ref := DecisionsRef(d.ref)
	pad := d.spine() + " "
	w := min(d.cw-3, capProse)
	if d.o.Open[DecisionsOnCard(ref)] {
		n := strconv.Itoa(len(ds)) + " decision"
		if len(ds) > 1 {
			n += "s"
		}
		d.add(ref, "", pad+paint(cSub, "Decisions")+dim(" · "+n+" · on the card below"), "")
		return
	}
	passed := d.o.Open[DecisionsPassed(ref)]
	if passed {
		d.add(ref, "", pad+paint(strong(cText), "Decisions")+dim(" · not answered · d opens the card"), "")
	} else {
		d.add(ref, "", pad+paint(strong(cText), "Decisions"), "")
	}
	for i, dc := range ds {
		if i > 0 {
			d.add(ref, "", pad, "") // a row between decisions, still the block's
		}
		d.add(ref, "", pad+dim(dc.Ref+".")+" "+paint(cWhite+bold, dc.Topic), "")
		in := blanks(len(dc.Ref) + 2)
		if dc.Context != "" {
			for _, l := range wrap(paint(cSub, inline(dc.Context, cSub)), max(8, w-len(in))) {
				d.add(ref, "", pad+in+l, "")
			}
		}
		// Labels line up, unless one is long: then each detail follows its own.
		lw := 0
		for _, o := range dc.Options {
			lw = max(lw, cellw.String(o.Label))
		}
		if lw > 12 {
			lw = 0
		}
		for _, o := range dc.Options {
			mark, name, ink := dim("•"), paint(cText+bold, o.Label), cSub
			switch {
			case o.Recommended && passed:
				mark = dim("☆") // only suggested: nothing was picked
			case o.Recommended:
				mark, name, ink = paint(cGreen, "★"), paint(cGreen+bold, o.Label), cText
			}
			lead := mark + " " + name + blanks(max(0, lw-cellw.String(o.Label)))
			leadW := 2 + max(lw, cellw.String(o.Label)) + 2
			rows := []string{""}
			if o.Detail != "" {
				rows = wrap(paint(ink, inline(o.Detail, ink)), max(8, w-len(in)-leadW))
			}
			switch {
			case o.Recommended && passed:
				rows[len(rows)-1] = strings.TrimRight(rows[len(rows)-1], " ") + "  " + dim("its suggestion")
			case o.Recommended:
				rows[len(rows)-1] = strings.TrimRight(rows[len(rows)-1], " ") + "  " + paint(cGreen, "its pick")
			}
			for k, r := range rows {
				if k == 0 {
					d.add(ref, "", pad+in+lead+"  "+r, "")
					continue
				}
				d.add(ref, "", pad+in+blanks(leadW)+r, "")
				d.wrapped()
			}
		}
	}
}
