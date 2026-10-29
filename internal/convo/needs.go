package convo

import (
	"strings"
	"unicode"
)

// Needs is what the session's last message asks of you, as the list read
// it: the words in it that do (Quote, as the message has them) and what
// it wants, in a few words (Reason). The answer in the last turn that
// holds Quote marks the rows they're on, and says why under them.
type Needs struct{ Quote, Reason string }

// key is n for a cache key: "" when nothing is asked.
func (n Needs) key() string {
	if n.Quote == "" {
		return ""
	}
	return n.Quote + "\x00" + n.Reason
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

// needsHere is whether answer text s is the one o.Needs points into: the
// last turn's, holding the quote.
func (d *drawer) needsHere(s string) bool {
	n := d.o.Needs
	if n.Quote == "" || d.s == nil || len(d.s.Turns) == 0 || d.s.Turns[len(d.s.Turns)-1] != d.t {
		return false
	}
	return HasQuote(s, n.Quote)
}

// markNeeds marks the rows from on that hold the quote, wherever it
// wraps, with the Needs-you edge beside the text, and adds why under them.
func (d *drawer) markNeeds(from int) {
	n := d.o.Needs
	q := foldWords(n.Quote)
	sp := d.spine()
	var joined strings.Builder
	starts := make([]int, 0, len(d.lines)-from)
	for _, l := range d.lines[from:] {
		if joined.Len() > 0 {
			joined.WriteByte(' ')
		}
		starts = append(starts, joined.Len())
		joined.WriteString(foldWords(strings.TrimPrefix(stripANSI(l.Text), stripANSI(sp))))
	}
	at := strings.Index(joined.String(), q)
	if at < 0 {
		return
	}
	end := at + len(q)
	edge := paint(cYellow, "▌")
	for i := range starts {
		next := joined.Len()
		if i+1 < len(starts) {
			next = starts[i+1]
		}
		if next <= at || starts[i] >= end {
			continue
		}
		l := &d.lines[from+i]
		if rest, ok := strings.CutPrefix(l.Text, sp+" "); ok {
			l.Text = sp + edge + rest
		}
	}
	if n.Reason != "" {
		d.add("", "", sp+edge+paint(cYellow+bold, "needs you")+dim(" · ")+paint(cText, n.Reason), "")
	}
}
