package convo

import (
	"strings"
	"unicode"
)

// Needs is what the session's last message asks of you, as the list read
// it: what kind of ask it is (Kind: "you", "offer", "ask" or "problem"),
// what it wants in a few words (Reason), the words in the message that
// ask it (Quote, as the message has them), and the rows the model laid
// out for it (Items). The last turn ends with it as a section of its own.
type Needs struct {
	Kind, Reason, Quote string
	Items               []NeedsItem
}

// NeedsItem is one row of the Needs-you section: Kind is
// "do" (a step only you can take), "run" (a command for you to run),
// "reply" (a reply you could send), "link" (a page to open) or "note"
// (a fact to decide by).
type NeedsItem struct {
	Kind string `json:"kind"`
	Text string `json:"text"`
}

// NeedsKinds are the kinds of row the section draws.
var NeedsKinds = []string{"do", "run", "reply", "link", "note"}

// NeedsRef is the ref of a row of the section a click acts on: a
// command copied, a reply put in your message box, a link opened.
func NeedsRef(kind, text string) string { return "needs:" + kind + ":" + text }

// NeedsOf is the kind and text of a NeedsRef.
func NeedsOf(ref string) (kind, text string, ok bool) {
	rest, ok := strings.CutPrefix(ref, "needs:")
	if !ok {
		return "", "", false
	}
	return strings.Cut(rest, ":")
}

// key is n for a cache key: "" when nothing is asked.
func (n Needs) key() string {
	if n.Reason == "" && len(n.Items) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteString(n.Kind + "\x00" + n.Reason + "\x00" + n.Quote)
	for _, it := range n.Items {
		b.WriteString("\x00" + it.Kind + ":" + it.Text)
	}
	return b.String()
}

// foldWords is s as a quote is compared: markdown's marks gone, every run
// of space one space, lower case. A quote copied from a message matches
// whether or not it kept the message's backticks, and across wrapped rows.
func foldWords(s string) string {
	var b strings.Builder
	space := false
	for _, r := range s {
		switch {
		case r == '`' || r == '*' || r == '_':
			continue
		case unicode.IsSpace(r):
			space = b.Len() > 0
			continue
		}
		if space {
			b.WriteByte(' ')
			space = false
		}
		b.WriteRune(unicode.ToLower(r))
	}
	return b.String()
}

// HasQuote reports whether text holds q, as foldWords compares them.
func HasQuote(text, q string) bool {
	q = foldWords(q)
	return q != "" && strings.Contains(foldWords(text), q)
}

// needsHere is whether the turn being drawn ends with the Needs-you
// section: the session's last, finished, with something asked.
func (d *drawer) needsHere() bool {
	return d.o.Needs.key() != "" && d.s != nil && len(d.s.Turns) > 0 && d.s.Turns[len(d.s.Turns)-1] == d.t && !d.t.Live
}

// needsSection draws what the last message asks of you as a section of
// its own under it: a heading saying what, then the rows the model laid
// out (steps to take, a command, replies, links, a note), then the words
// that asked it, quiet. A command, reply or link is clicked to act on it.
func (d *drawer) needsSection() {
	n := d.o.Needs
	frame, mark := cYellow, paint(cYellow+bold, "?")
	switch n.Kind {
	case "you":
		mark = paint(cYellow+bold, "!")
	case "problem":
		frame, mark = cRed, paint(cRed+bold, "!")
	}
	pad := d.spine() + blanks(gutter-1)
	w := max(16, min(d.cw, capRow)-gutter-6)
	rail := paint(frame, "│")
	d.gap()
	head := mark + " " + paint(strong(cText), "Needs you")
	if n.Reason != "" {
		head += dim(" · ") + paint(cText, n.Reason)
	}
	d.add("", "", pad+paint(frame, "╭")+" "+head, "")
	for _, it := range n.Items {
		lead, ink, hint, ref := dim("·"), cSub, "", ""
		switch it.Kind {
		case "do":
			lead, ink = paint(cText, "☐"), cText
		case "run":
			lead, ink, hint, ref = paint(cGreen, "$"), cWhite, "click copies", NeedsRef(it.Kind, it.Text)
		case "reply":
			lead, ink, hint, ref = paint(cBlue, "↩"), cText, "click to reply", NeedsRef(it.Kind, it.Text)
		case "link":
			lead, ink, hint, ref = paint(cBlue, "↗"), cBlue, "click opens", NeedsRef(it.Kind, it.Text)
		}
		rows := wrap(paint(ink, it.Text), w)
		for k, r := range rows {
			l := blanks(2)
			if k == 0 {
				l = lead + " "
			}
			right := ""
			if k == 0 && hint != "" {
				right = dim(hint) + "  "
			}
			d.add(ref, "", pad+rail+" "+l+r, right)
		}
	}
	if q := strings.TrimSpace(n.Quote); q != "" {
		for k, r := range wrap(faint("it said “"+q+"”"), w) {
			if k > 0 {
				r = "  " + r
			}
			d.add("", "", pad+rail+" "+r, "")
		}
	}
	// The last row closes the frame.
	last := &d.lines[len(d.lines)-1]
	if rest, ok := strings.CutPrefix(last.Text, pad+rail); ok {
		last.Text = pad + paint(frame, "╰") + rest
	} else if strings.HasPrefix(last.Text, pad+paint(frame, "╭")) {
		d.add("", "", pad+paint(frame, "╰"), "")
	}
}
