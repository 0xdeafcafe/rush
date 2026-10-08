package convo

import (
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

// attachRe finds the chips for what you attached: images and pastes.
var attachRe = regexp.MustCompile(`\[Image #\d+[^\]]*\]|\[image: [^\]]+\]|` + PasteChipRe.String())

// askRows wraps your words, one paragraph a line, to w cells, each line
// styled by style. A word longer than a row, a pasted URL say, is broken
// across rows. A running turn draws it every frame, so it's kept as drawn.
func (d *drawer) askRows(ask string, w int, style func(string) string, key string) []string {
	return d.cachedRows(ask, key, w, func() []string {
		var rows []string
		for i, l := range strings.Split(strings.TrimSpace(ask), "\n") {
			if l = strings.TrimSpace(l); l == "" {
				if i > 0 && rows[len(rows)-1] != "" {
					rows = append(rows, "")
				}
				continue
			}
			rows = append(rows, wrap(style(oneLine(l)), w)...)
		}
		return rows
	})
}

// cachedRows is build's rows, kept for the text, key and width: a running
// turn draws what you said every frame.
func (d *drawer) cachedRows(text, key string, w int, build func() []string) []string {
	mk := memoKey{text: text, style: key, n: w, pal: palette}
	if ls, ok := d.s.memoGet(mk); ok {
		rows := make([]string, len(ls))
		for i, l := range ls {
			rows[i] = l.Text
		}
		return rows
	}
	rows := build()
	ls := make([]Line, len(rows))
	for i, r := range rows {
		ls[i].Text = r
	}
	d.s.memoPut(mk, ls)
	return rows
}

// boxPadTop is the rows of fill above what you said inside it.
const boxPadTop = 0

// Pastes up to pasteFull lines show whole; longer ones show their first
// pasteHead and a line that opens the message.
const (
	pasteFull = 15
	pasteHead = 12
)

// userBox is what you said: a fill one step off the ground, the turn's
// rule its left edge, the words a column in. Attachments are one dim
// line; a paste shows its own text. Clicking it opens or closes the turn.
// Its images show as thumbnails in place of their chips once they're made.
func (d *drawer) userBox(ref, title, ask string, imgs []string, pics []*event.ImageData, multi bool) {
	if parts := queuedMessages(ask); len(parts) > 0 && !multi {
		for i, part := range parts {
			if i > 0 {
				// Queue entries are independent user messages. d.gap deliberately
				// leaves a filled user box alone, so add this separator as part of
				// that box rather than relying on its usual between-section gap.
				d.add(ref, bgUser, paint(cOrange, "▌"), "")
			}
			d.userBox(ref, "", part, nil, nil, false)
			d.named = true // one "you" for the lot
		}
		// Numbered markers already carry their original image links. When
		// every attachment has one, don't add a second filename chip (or a
		// second thumbnail) that points at the same image. A partial match is
		// left alone: there is no safe way to renumber the remaining images.
		if (len(imgs) > 0 || len(pics) > 0) && !numberedImagesCover(ask, max(len(imgs), len(pics))) {
			d.userBox(ref, "", "", imgs, pics, false)
		}
		d.named = false
		return
	}
	var chips []chip
	inner := max(10, d.cw-4)
	var rows []string
	if multi {
		rows = []string{paint(cWhite+bold, "$ ") + dim("shell command")}
	} else {
		for i, part := range pasteParts(ask) {
			if part.paste {
				rows = append(rows, d.cachedRows(part.text, "paste:"+ref+":"+strconv.Itoa(i), inner, func() []string { return pasteRows(ref, part.text, inner) })...)
				continue
			}
			text := attachRe.ReplaceAllStringFunc(part.text, func(m string) string {
				chips = append(chips, chipOf(m))
				return ""
			})
			rows = append(rows, d.askRows(text, inner, func(s string) string { return styledAsk(messageLinks(s, ref), cWhite) }, "box:"+ref+":"+strconv.Itoa(i))...)
		}
	}
	for i, name := range imgs {
		chips = append(chips, chip{shown: "▣ " + name, href: "/image/" + strconv.Itoa(i+1)})
	}
	thumbs := thumbRows(ref, pics, inner)
	if thumbs != nil {
		chips = slices.DeleteFunc(chips, func(c chip) bool { return strings.HasPrefix(c.shown, "▣ ") })
	}
	rows = append(rows[:len(rows):len(rows)], chipRows(ref, chips, inner)...)
	if thumbs != nil {
		rows = append(rows, thumbs...)
	}
	if title != "" {
		rows = append([]string{faint(title)}, rows...)
	}
	// Its left edge is a rail in your colour, the heaviest on the page; the
	// cursor takes it when the turn's picked.
	edge := paint(cOrange, "▌")
	if ref != "" && ref == d.o.Selected {
		edge = d.spine()
	}
	fill := func(s, right string) { d.add(ref, bgUser, edge+"  "+s, right) }
	for range boxPadTop {
		fill("", "")
	}
	for i, r := range rows {
		who := ""
		if i == 0 && !d.named {
			who = dim("you") + " " // who said it, as the agent's answer is named under it
		}
		fill(r, who)
		if i > 0 {
			d.wrapped()
		}
	}
}

// pastePart is a stretch of a message: its words, or one paste.
type pastePart struct {
	text  string
	paste bool
}

// pasteParts splits a message at the pastes Claude Code marks in it.
func pasteParts(s string) []pastePart {
	var out []pastePart
	at := 0
	for _, m := range pastedRe.FindAllStringSubmatchIndex(s, -1) {
		if m[0] > at {
			out = append(out, pastePart{text: s[at:m[0]]})
		}
		out = append(out, pastePart{text: s[m[2]:m[3]], paste: true})
		at = m[1]
	}
	if at < len(s) || len(out) == 0 {
		out = append(out, pastePart{text: s[at:]})
	}
	return out
}

// pasteLines is a paste's lines as they were meant: the border a copy from
// a terminal box brings along stripped when most lines carry it.
func pasteLines(text string) []string {
	lines := strings.Split(strings.TrimSpace(strings.ReplaceAll(text, "\r", "")), "\n")
	bordered := 0
	for _, l := range lines {
		if strings.HasPrefix(strings.TrimLeft(l, " "), "│") {
			bordered++
		}
	}
	for i, l := range lines {
		if bordered*2 > len(lines) {
			l = strings.TrimRight(strings.TrimLeft(l, " │"), " │")
		}
		lines[i] = strings.TrimRight(expandTabs(l), " ")
	}
	return lines
}

// pasteRows is a paste as text: a dim label, then its lines, all of them
// when they're few, else the first and a link that opens the message.
func pasteRows(ref, text string, w int) []string {
	lines := pasteLines(text)
	more := 0
	if len(lines) > pasteFull {
		lines, more = lines[:pasteHead], len(lines)-pasteHead
	}
	open := func(s string) string { return "\x1b]8;;rush:message/" + ref + "\x1b\\" + dim(s) + "\x1b]8;;\x1b\\" }
	label := "▤ pasted"
	if more > 0 {
		label += " · " + plural(len(lines)+more, "line")
	}
	rows := []string{dim(label)}
	for _, l := range lines {
		if l == "" {
			rows = append(rows, "")
			continue
		}
		rows = append(rows, wrap(paint(cSub, l), w)...)
	}
	if more > 0 {
		rows = append(rows, open(fmt.Sprintf("… %d more lines · click to show all", more)))
	}
	return rows
}

// otherAsk is a turn you didn't start, from another agent, a wakeup or a
// rush switch, as a quiet line of the turn's own, inset like the steps.
func (d *drawer) otherAsk(ask, noun, how string, wake bool) {
	t := d.t
	style, label := sub, dim("◌ "+t.From)
	if t.From == "" && strings.TrimSpace(t.Prompt) == "" {
		label = dim("◌")
	}
	if wake {
		style, label = text, dim(noun)
		if ask != "" {
			ask = `"` + ask + `"`
		}
	}
	if switched(t) {
		style, label, ask = func(s string) string { return paint(cOrange, s) }, dim("↻ rush"), switchedAsk
	}
	lw := cellw.String(label)
	rows := d.askRows(ask, max(10, min(d.cw-gutter-lw-3, capProse)), style, "ask:"+t.From+":"+d.ref)
	if strings.TrimSpace(t.Prompt) == "" && t.From == "" {
		rows = []string{dim(unasked(t))}
	}
	if wake && len(rows) > 0 {
		rows[len(rows)-1] += how // a copy: the memo keeps the words alone
	}
	for i, r := range rows {
		if i == 0 {
			d.add(d.ref, "", d.spine()+blanks(gutter-1)+label+"  "+r, "")
		} else {
			d.add(d.ref, "", d.spine()+blanks(gutter-1+lw+2)+r, "")
			d.wrapped()
		}
	}
}

// chip is an attachment as one dim line shows it, and where clicking it goes
// under the message: "" for the message itself.
type chip struct{ shown, href string }

// chipOf names a chip you attached: images and pastes, without the brackets
// a transcript wraps them in.
func chipOf(m string) chip {
	switch {
	case strings.HasPrefix(m, "[image: "):
		return chip{shown: "▣ " + ImageLabel(strings.TrimSuffix(m[len("[image: "):], "]"))}
	case strings.HasPrefix(m, "[Image"):
		return chip{shown: "▣ " + strings.Trim(m, "[]"), href: "/image/" + messageImageRe.FindStringSubmatch(m)[1]}
	}
	return chip{shown: "▤ " + strings.Trim(m, "[]")}
}

// chipRows lays chips out dim, as many to a row as fit in w.
func chipRows(ref string, chips []chip, w int) []string {
	var rows []string
	row, n := "", 0
	for _, c := range chips {
		cw := cellw.String(c.shown)
		if n > 0 && n+3+cw > w {
			rows, row, n = append(rows, row), "", 0
		}
		if n > 0 {
			row, n = row+"   ", n+3
		}
		shown := cellw.Truncate(c.shown, w, "…")
		row, n = row+"\x1b]8;;rush:message/"+ref+c.href+"\x1b\\"+dim(shown)+"\x1b]8;;\x1b\\", n+min(cw, w)
	}
	if n > 0 {
		rows = append(rows, row)
	}
	return rows
}

// Only unwrap rush's exact delivery envelope; ordinary user text stays intact.
func queuedMessages(text string) []string {
	head, rest, ok := strings.Cut(text, "\n\n")
	if !ok {
		return nil
	}
	countText, suffix, ok := strings.Cut(head, " messages, ")
	if !ok || suffix != "queued while you worked. Each is its own message; take them in order." {
		return nil
	}
	n, err := strconv.Atoi(countText)
	if err != nil || n < 2 || n > 10000 {
		return nil
	}
	var parts []string
	for i := 1; i <= n; i++ {
		marker := fmt.Sprintf("[Message %d of %d]\n", i, n)
		if !strings.HasPrefix(rest, marker) {
			return nil
		}
		rest = strings.TrimPrefix(rest, marker)
		if i == n {
			parts = append(parts, rest)
			break
		}
		part, next, found := strings.Cut(rest, fmt.Sprintf("\n\n[Message %d of %d]\n", i+1, n))
		if !found {
			return nil
		}
		parts = append(parts, part)
		rest = fmt.Sprintf("[Message %d of %d]\n", i+1, n) + next
	}
	if host.JoinQueue(parts) != text {
		return nil
	}
	return parts
}

func numberedImagesCover(text string, count int) bool {
	if count == 0 {
		return false
	}
	seen := make(map[int]bool, count)
	for _, m := range messageImageRe.FindAllStringSubmatch(text, -1) {
		n, err := strconv.Atoi(m[1])
		if err == nil {
			seen[n] = true
		}
	}
	for n := 1; n <= count; n++ {
		if !seen[n] {
			return false
		}
	}
	return true
}
