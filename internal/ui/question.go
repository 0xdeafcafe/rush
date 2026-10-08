package ui

import (
	"fmt"
	"strconv"
	"strings"
	"unicode"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// The question card's grounds: the card itself, the option under the
// cursor, and the keycaps that number the options.
var qCard, qSel, qCap string

// A lit chip or keycap is in rush's own colours on any ground, with its
// own dark text.
const (
	qCapOn = "\x1b[48;2;217;119;87m"
	qChip  = "\x1b[48;2;229;181;103m"
	qInk   = "\x1b[38;2;33;28;22m" // dark text on a lit chip or keycap
)

// keycap is a key drawn as a key: " 1 " on its own ground, lit when it's
// the one under the cursor.
func keycap(k string, on bool) string {
	k = convo.KeyWord(k)
	if on {
		return qCapOn + qInk + bold + " " + k + " " + reset
	}
	return qCap + cSub + " " + k + " " + reset
}

// chip is a lit label, the card's heading.
func chip(s string) string { return qChip + qInk + bold + " " + s + " " + reset }

// questionCard draws Claude's questions as one form: a heading, the
// question as prose with the ask itself set apart, each option as a block
// (a numbered keycap, its label, and the whole of its description), and
// beside them the preview of the option under the cursor when Claude gave
// one. With more than one question, a strip names them all (answered ones
// with their answer, so you can go back and change one) and a last step
// lists your answers before they go. It keeps to maxH rows by shortening
// the options not under the cursor first, then the question's lead-in.
func (m *Model) questionCard(c *hostConn, req *event.Question, w, maxH int) []string {
	c.syncQuestion(req)
	title, qs := questions(req)
	if len(qs) == 0 {
		return nil
	}
	var out []string
	for fold := 0; fold <= 3; fold++ {
		out = drawQuestion(c, title, qs, w, fold, maxH)
		if maxH <= 0 || len(out) <= maxH {
			break
		}
	}
	c.qRows = out
	if c.inModal {
		c.qRows, c.qHit = nil, nil
	}
	return out
}

// drawQuestion draws the card, folded: 0 shows everything, and each step up
// shows less of what isn't under the cursor. In full view (v) it's the
// option under the cursor alone, its preview as tall as maxH allows. It
// notes in c.qHit what a click on each row presses.
func drawQuestion(c *hostConn, title string, qs []question, w, fold, maxH int) []string {
	var out []string
	c.qHit = map[int]string{}
	edge := paint(cYellow, "▍")
	if c.cardFocus {
		edge = paint(cOrange, "▍")
	}
	cl := func(txt string) { out = append(out, onBg(qCard, edge+txt, w)) }
	if c.inModal {
		// The modal's frame is the card.
		cl = func(txt string) { out = append(out, fit(" "+txt, w)) }
	}
	review := c.qIdx >= len(qs)
	textW := max(20, min(w-8, 108))

	// The heading: what it's about, and where you are among several.
	head, right := "", ""
	switch {
	case len(qs) == 1 && qs[0].Header != "":
		head = chip("? "+oneLine(qs[0].Header)) + "  " + paint(cSub, "Claude asks")
	default:
		head = chip("?") + " " + paint(cYellow+bold, "Claude asks")
	}
	if title != "" {
		head += "  " + dim("·") + "  " + paint(cText, oneLine(title))
	}
	if len(qs) > 1 {
		right = fmt.Sprintf("%d of %d answered", len(c.qAnswer), len(qs))
	}
	cl(spread(" "+head, paint(cSub, right)+"  ", w-1))
	if len(qs) > 1 {
		cl("")
		cl("  " + questionStrip(c, qs, w-4))
	}
	cl("")

	if review {
		labelW := 0
		for i, q := range qs {
			labelW = max(labelW, cellw.String(firstNonEmpty(q.Header, fmt.Sprintf("Question %d", i+1))))
		}
		labelW = min(labelW, 24)
		cl("  " + paint(cText+bold, "Send these answers?"))
		cl("")
		for i, q := range qs {
			name := ansi.Truncate(firstNonEmpty(q.Header, fmt.Sprintf("Question %d", i+1)), labelW, "…")
			ans, ok := c.qAnswer[q.Question]
			a := paint(cGreen, "✓ ") + paint(cText+bold, ansi.Truncate(shownAnswer(ans), max(10, w-labelW-16), "…"))
			if !ok {
				a = paint(cYellow, "○ not answered")
			}
			c.qHit[len(out)] = fmt.Sprint(i + 1)
			cl("  " + keycap(fmt.Sprint(i+1), false) + "  " + paint(cSub, name) + strings.Repeat(" ", labelW-cellw.String(name)+3) + a)
		}
		cl("")
		// The button enter presses: lit while the card has the keys.
		c.qHit[len(out)] = "enter"
		if c.cardFocus {
			cl("  " + tabOn + " ⏎ Send answers " + reset)
		} else {
			cl("  " + tabOff + " ⏎ Send answers " + reset)
		}
		cl("")
		if c.cardFocus {
			cl("  " + keysFit(w-6, "enter", "sends your answers", "← · 1–"+fmt.Sprint(len(qs)), "changes one", "s", "skips them all", "esc", escWord(c)))
		} else {
			cl("  " + dim("↑ to send or change your answers"))
		}
		return out
	}

	// The question: a lead-in as prose, then the ask itself in bold.
	q := qs[c.qIdx]
	lead, ask := splitAsk(q.Question)
	if lead != "" {
		lines := wrap(convo.Inline(lead, cText), textW)
		if fold >= 3 && len(lines) > 4 {
			lines = append(lines[:3:3], lines[3]+" …")
		}
		for _, l := range lines {
			cl("  " + paint(cText, l))
		}
		cl("")
	}
	for _, l := range wrap(convo.Inline(ask, cText+bold), textW) {
		cl("  " + paint(cText+bold, l))
	}
	cl("")

	if c.qFull && c.cardFocus && c.qCursor < len(q.Options) {
		o := q.Options[c.qCursor]
		label, rec := optionLabel(o.Label)
		name := keycap(fmt.Sprint(c.qCursor+1), true) + " " + paint(cText+bold, label)
		if rec {
			name += "  " + paint(cGreen, "★ recommended")
		}
		c.qHit[len(out)] = fmt.Sprint("@", c.qCursor)
		cl(spread(" "+paint(cOrange, "▌")+" "+name, dim(fmt.Sprintf("%d of %d", c.qCursor+1, len(q.Options)))+"  ", w-1))
		if d := strings.TrimSpace(o.Description); d != "" {
			for _, l := range wrap(convo.Inline(d, cText), textW) {
				cl("     " + paint(cText, l))
			}
		}
		cl("")
		if o.Preview == "" {
			cl("     " + dim("no preview for this option"))
		} else {
			rows := 1 << 20
			if maxH > 0 {
				rows = max(6, maxH-len(out)-3)
			}
			for _, r := range previewBox(label, o.Preview, w-6, rows) {
				cl("     " + r)
			}
		}
		cl("")
		cl("  " + keysFit(w-6, "↑↓", "other options", "enter", "picks", "v", "back to all", "esc", "back to all"))
		return out
	}

	// The options, then the preview beside them when there's room, or
	// under them when there isn't.
	cursor := -1
	if c.cardFocus {
		cursor = c.qCursor
	}
	var preview, previewOf string
	if at := c.qCursor; at < len(q.Options) {
		preview, previewOf = q.Options[at].Preview, q.Options[at].Label
	}
	side := preview != "" && w >= 96
	lw := w - 1
	if side {
		lw = min(60, (w-1)/2)
	}
	type orow struct {
		text string
		sel  bool
		hit  string // what a click on it presses
	}
	var rows []orow
	descW := max(16, min(lw-10, 104))
	picked := c.picks(c.qIdx)
	prev := c.qAnswer[q.Question]
	for i, o := range q.Options {
		on := cursor == i
		label, rec := optionLabel(o.Label)
		bar := " "
		if on {
			bar = paint(cOrange, "▌")
		}
		mark := ""
		switch {
		case q.MultiSelect && picked[i]:
			mark = paint(cGreen, "☑ ")
		case q.MultiSelect:
			mark = dim("☐ ")
		}
		name := paint(cText+bold, label)
		if !on {
			name = paint(cText, label)
		}
		name = keycap(fmt.Sprint(i+1), on) + " " + mark + name
		if rec {
			name += "  " + paint(cGreen, "★ recommended")
		}
		if !q.MultiSelect && prev == o.Label {
			name += "  " + paint(cGreen, "✓ your answer")
		}
		if o.Preview != "" && !side && o.Preview != preview {
			name += "  " + dim("◇ preview")
		}
		hit := fmt.Sprint("@", i)
		rows = append(rows, orow{" " + bar + " " + name, on, hit})
		if d := strings.TrimSpace(o.Description); d != "" {
			ink := cSub
			if on {
				ink = cText
			}
			lines := wrap(convo.Inline(d, ink), descW)
			keep := [...]int{6, 2, 1, 0}[fold]
			if on {
				keep = 12
			}
			if len(lines) > keep {
				lines = lines[:keep]
				if keep > 0 {
					lines[keep-1] += " …"
				}
			}
			for _, l := range lines {
				rows = append(rows, orow{" " + bar + "     " + paint(ink, l), on, hit})
			}
		}
		if fold < 2 {
			rows = append(rows, orow{"", false, ""})
		}
	}
	own := cursor == len(q.Options)
	bar := " "
	if own {
		bar = paint(cOrange, "▌")
	}
	ownText := paint(cText, "Something else") + "  " + dim("type your own answer below")
	if own {
		ownText = paint(cText+bold, "Something else") + "  " + paint(cSub, "enter, then type it below")
	}
	if ownAnswer(q, prev) {
		ownText += "  " + paint(cGreen, "✓ ") + paint(cText, ansi.Truncate(shownAnswer(prev), max(8, lw-40), "…"))
	}
	rows = append(rows, orow{" " + bar + " " + keycap("✎", own) + " " + ownText, own, fmt.Sprint("@", len(q.Options))})
	if q.MultiSelect {
		// Enter on an option only ticks it; moving on is this button.
		btn := cursor == len(q.Options)+1
		bar, face := " ", tabOff
		if btn {
			bar, face = paint(cOrange, "▌"), tabOn
		}
		n := 0
		for _, v := range picked {
			if v {
				n++
			}
		}
		note := dim("tick at least one first")
		if n > 0 {
			note = paint(cSub, fmt.Sprintf("with %d ticked", n))
		}
		rows = append(rows, orow{"", false, ""}, orow{" " + bar + " " + face + " ⏎ Continue " + reset + "  " + note, btn, fmt.Sprint("@", len(q.Options)+1)})
	}

	var pv []string
	if preview != "" {
		pw := w - 1 - 5
		if side {
			pw = w - 1 - lw - 3
		}
		name, _ := optionLabel(previewOf)
		pv = previewBox(name, preview, pw, 14)
	}
	n := len(rows)
	if side {
		n = max(n, len(pv))
	}
	for i := 0; i < n; i++ {
		left, sel := "", false
		if i < len(rows) {
			left, sel = rows[i].text, rows[i].sel
			if rows[i].hit != "" {
				c.qHit[len(out)] = rows[i].hit
			}
		}
		bg := qCard
		if sel {
			bg = qSel
		}
		if !side {
			cl(onBg(bg, left, w-3) + "  ")
			continue
		}
		r := ""
		if i < len(pv) {
			r = pv[i]
		}
		cl(onBg(bg, left, lw) + "  " + r)
	}
	if !side && len(pv) > 0 {
		cl("")
		for _, r := range pv {
			cl("     " + r)
		}
	}
	cl("")
	if c.cardFocus {
		pairs := []string{"↑↓", "choose", "enter", "picks", "1–" + fmt.Sprint(len(q.Options)), "pick one"}
		if q.MultiSelect {
			pairs = []string{"↑↓", "choose", "enter", "ticks", "1–" + fmt.Sprint(len(q.Options)), "tick one"}
		}
		if len(qs) > 1 {
			pairs = append(pairs, "←→", "questions")
		}
		pairs = append(pairs, "v", "full view", "s", "skips", "esc", escWord(c))
		cl("  " + keysFit(w-6, pairs...))
	} else {
		cl("  " + paint(cSub, "↑") + dim(" to choose   ·   or type your own answer below and press ") + paint(cSub, "enter"))
	}
	return out
}

// ownAnswer is whether a is an answer in your own words, not an option.
func ownAnswer(q question, a string) bool {
	if a == "" || q.MultiSelect {
		return false
	}
	for _, o := range q.Options {
		if o.Label == a {
			return false
		}
	}
	return true
}

// splitAsk parts a question into its lead-in and the ask: Claude often
// explains first and asks last ("… so I need your go-ahead. Proceed?").
// The ask is the last sentence when it's a question and there's something
// before it; otherwise it's the whole text.
func splitAsk(q string) (lead, ask string) {
	q = strings.TrimSpace(q)
	if !strings.HasSuffix(q, "?") {
		return "", q
	}
	if i := strings.LastIndex(q, "\n\n"); i > 0 {
		return strings.TrimSpace(q[:i]), strings.TrimSpace(q[i+2:])
	}
	for i := len(q) - 2; i > 0; i-- {
		if q[i] != ' ' && q[i] != '\n' || !strings.ContainsRune(".!?", rune(q[i-1])) {
			continue
		}
		rest := strings.TrimSpace(q[i:])
		if r := []rune(rest); len(r) > 0 && unicode.IsUpper(r[0]) {
			return strings.TrimSpace(q[:i]), rest
		}
		break
	}
	return "", q
}

// questionStrip is a row naming every question: the one at hand lit, ✓
// and its answer once answered, ○ the rest, then send.
func questionStrip(c *hostConn, qs []question, w int) string {
	var parts []string
	for i, q := range qs {
		name := firstNonEmpty(q.Header, fmt.Sprintf("Question %d", i+1))
		ans, ok := c.qAnswer[q.Question]
		switch {
		case i == c.qIdx:
			parts = append(parts, chip(name))
		case ok:
			parts = append(parts, paint(cGreen, "✓ ")+paint(cSub, name)+" "+dim(ansi.Truncate(shownAnswer(ans), 18, "…")))
		default:
			parts = append(parts, dim("○ "+name))
		}
	}
	send := dim("○ send")
	if c.qIdx >= len(qs) {
		send = chip("send")
	}
	parts = append(parts, send)
	return ansi.Truncate(strings.Join(parts, faint("  ›  ")), w, "…")
}

// previewBox frames an option's preview under its name: Claude writes it as markdown, most
// often a mockup or code, so its lines are kept as they are (fences
// dropped), cut to the width and to maxRows rows.
func previewBox(name, md string, w, maxRows int) []string {
	var body []string
	for _, l := range strings.Split(strings.ReplaceAll(md, "\t", "    "), "\n") {
		if strings.HasPrefix(strings.TrimSpace(l), "```") {
			continue
		}
		body = append(body, ansi.Strip(strings.TrimRight(l, " \r")))
	}
	for len(body) > 0 && strings.TrimSpace(body[len(body)-1]) == "" {
		body = body[:len(body)-1]
	}
	more := 0
	if len(body) > maxRows {
		more = len(body) - maxRows + 1
		body = body[:maxRows-1]
	}
	inner := 8
	for _, l := range body {
		inner = max(inner, cellw.String(l))
	}
	inner = min(inner, max(8, w-4))
	label := ansi.Truncate(oneLine(name), max(4, inner-2), "…")
	top := faint("╭─") + " " + paint(cSub, label) + " " + faint(strings.Repeat("─", max(0, inner-cellw.String(label)-1))+"╮")
	out := []string{top}
	for _, l := range body {
		if cellw.String(l) > inner {
			l = ansi.Truncate(l, inner-1, "") + faint("›")
		}
		out = append(out, faint("│")+" "+paint(cText, l)+strings.Repeat(" ", inner-cellw.String(l))+" "+faint("│"))
	}
	if more > 0 {
		t := fmt.Sprintf("… %d more lines", more)
		out = append(out, faint("│")+" "+dim(t)+strings.Repeat(" ", max(0, inner-cellw.String(t)))+" "+faint("│"))
	}
	return append(out, faint("╰"+strings.Repeat("─", inner+2)+"╯"))
}

// escWord is what esc does on a question: in its modal, sets it aside.
func escWord(c *hostConn) string {
	if c.inModal {
		return "later"
	}
	return "type instead"
}

// heldBox is the message box set aside while Claude's questions wait.
type heldBox struct {
	input    []rune
	back     int
	undo     undoStack
	editQ    int
	editWas  string
	editHeld bool
}

// holdForQuestion keeps the box to the question while one waits: whatever
// was being typed when it came is set aside, so the
// box answers the question and nothing else, and it comes back once the
// question is answered, for the conversation to go on in the order Claude
// expects.
func (m *Model) holdForQuestion(c *hostConn) {
	asking := isQuestion(c.sess.Pending())
	switch {
	case asking && c.qHeld == nil:
		// Held even when empty, or what's typed as the answer would be
		// taken for what was there on the next pass and set aside too.
		c.qHeld = &heldBox{input: c.input, back: c.back, undo: c.undo, editQ: c.editQ, editWas: c.editWas, editHeld: c.editHeld}
		c.input, c.back, c.anchor, c.undo = nil, 0, 0, undoStack{}
		c.editQ, c.editWas, c.editHeld = 0, "", false
		c.slashSel = 0
	case !asking && c.qHeld != nil && c.editQ == 0 && len(c.input) == 0:
		// A half-written answer holds it back: it stays in the box, for
		// the next question if one comes, or to send.
		h := c.qHeld
		c.qHeld = nil
		c.input, c.back, c.anchor, c.undo = h.input, h.back, 0, h.undo
		c.editQ, c.editWas, c.editHeld = h.editQ, h.editWas, h.editHeld
	}
}

// clickQuestion is a click on the question card: on an option it puts the
// cursor there, and picks it once it's there; on a key in its hints, it
// presses that key. It reports whether the click was the card's.
func (m *Model) clickQuestion(c *hostConn, x, y int) (tea.Cmd, bool) {
	if c.panelClip[1] > 0 && (y < c.panelClip[0] || y >= c.panelClip[1]) || !isQuestion(c.sess.Pending()) {
		return nil, false
	}
	i := y - c.dockY - c.cardTop - c.qHitTop
	if i < 0 || i >= len(c.qRows) {
		return nil, false
	}
	k := c.qHit[i]
	if n, ok := strings.CutPrefix(k, "@"); ok {
		at, _ := strconv.Atoi(n)
		if !c.cardFocus || c.qCursor != at {
			c.cardFocus, c.qCursor = true, at
			return nil, true
		}
		k = "enter"
	}
	if k == "" {
		k = hintKey(c.qRows[i], x-m.paneX())
	}
	if k == "" {
		return nil, false
	}
	c.cardFocus = true
	return m.cardKey(c, k, true)
}
