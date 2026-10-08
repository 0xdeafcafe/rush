package ui

import (
	"regexp"
	"sort"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/charmbracelet/x/ansi"
	"github.com/mattn/go-runewidth"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// The input box sits on the brightest surface on screen, so where you type
// is never in doubt; its edge turns orange while your keys go there.
var (
	bgInput string
	bgMark  string // selected text
	bgChip  string // a paste folded into a chip, an image
	cEdge   string
)

// box describes one input box: the edge labels and what's inside.
type box struct {
	w       int
	focused bool
	topL    string // who the text goes to, and what enter will do
	topR    string
	footL   string
	footR   string
	text    []rune
	cursor  int
	anchor  int    // selection start, -1 for none
	holder  string // shown faint while the box is empty
	lead    string // before the text on its first line, e.g. ❯
	maxRows int
	top     int  // the first wrapped row shown last draw, kept while the cursor stays in view
	hot     seg  // a paste chip lit, under the pointer
	idle    bool // focused but not being typed in: the edge and fill stay, the cursor doesn't
	// pos maps each drawn position to the text's where an image in the
	// text is drawn as its name; nil when they are the same.
	pos []int
}

// lines draws the box: a top edge carrying its labels, the text wrapped
// with the cursor in place, and a bottom edge.
func (b box) lines() []string {
	w := max(20, b.w)
	edge := cEdge
	if b.focused {
		edge = cOrange
	}
	inner := w - 4 // "│ " … " │"
	border := func(l, r, left, right string) string {
		head := paint(edge, l+"─")
		if left != "" {
			head += " " + left + " "
		}
		tail := paint(edge, "─"+r)
		if right != "" {
			tail = " " + right + " " + tail
		}
		if cellw.String(head)+cellw.String(tail) > w {
			tail = paint(edge, "─"+r)
		}
		if over := cellw.String(head) + cellw.String(tail) - w; over > 0 {
			head = ansi.Truncate(head, cellw.String(head)-over-1, "…")
		}
		fill := w - cellw.String(head) - cellw.String(tail)
		return head + paint(edge, strings.Repeat("─", max(0, fill))) + tail
	}
	out := []string{border("╭", "╮", b.topL, b.topR)}
	for _, row := range b.content(inner) {
		pad := inner - cellw.String(row)
		body := row + strings.Repeat(" ", max(0, pad))
		body = bgInput + strings.ReplaceAll(body, reset, reset+bgInput) + reset
		out = append(out, paint(edge, "│")+bgInput+" "+reset+body+bgInput+" "+reset+paint(edge, "│"))
	}
	return append(out, border("╰", "╯", b.footL, b.footR))
}

// rows is how many text rows lines() draws.
func (b box) rows() int {
	if len(b.text) == 0 {
		return 1
	}
	start, end := b.window(b.segs())
	return end - start
}

// segs is the text wrapped as lines() draws it. A row leaves a cell spare,
// so the cursor after a full row sits inside the edge rather than on it.
func (b box) segs() []seg { return wrapSegs(b.text, b.textW()) }

func (b box) textW() int { return max(20, b.w) - 4 - b.leadW() - 1 }

// seg is one wrapped row of the text, as rune offsets [from, to).
type seg struct{ from, to int }

// wrapSegs word-wraps text to w cells a row, breaking long words, and keeps
// each row's offsets so a click or the cursor maps to an exact character.
func wrapSegs(text []rune, w int) []seg {
	w = max(4, w)
	// at[i] is the width of text[:i], so any stretch's width is a subtraction.
	at := make([]int, len(text)+1)
	for i, r := range text {
		at[i+1] = at[i] + runeW(r)
	}
	var out []seg
	start := 0
	for start <= len(text) {
		end := start
		for end < len(text) && text[end] != '\n' {
			end++
		}
		from := start
		for {
			if at[end]-at[from] <= w {
				out = append(out, seg{from, end})
				break
			}
			cut := from
			for cut < end && at[cut+1]-at[from] <= w {
				cut++
			}
			brk := cut
			for i := cut; i > from; i-- {
				if text[i-1] == ' ' {
					brk = i
					break
				}
			}
			out = append(out, seg{from, brk})
			from = brk
		}
		if end >= len(text) {
			break
		}
		start = end + 1
	}
	return out
}

// runeW is runewidth.RuneWidth, with printable ASCII answered directly.
func runeW(r rune) int {
	if r >= 0x20 && r < 0x7f {
		return 1
	}
	return runewidth.RuneWidth(r)
}

// window is which wrapped rows are on screen: the rows from top, moved
// only as far as it takes to keep the cursor's row among them.
func (b box) window(segs []seg) (start, end int) {
	limit := max(1, b.maxRows)
	if len(segs) <= limit {
		return 0, len(segs)
	}
	at := len(segs) - 1
	for i, sg := range segs {
		if b.cursor >= sg.from && b.cursor <= sg.to {
			at = i
			break
		}
	}
	start = min(max(b.top, at-limit+1), at)
	start = max(0, min(start, len(segs)-limit))
	return start, start + limit
}

// scrolled is b with top set to the first row it shows, for the caller to
// keep and hand back on the next draw.
func (b box) scrolled() box {
	b.top, _ = b.window(b.segs())
	return b
}

func (b box) leadW() int { return cellw.String(b.lead) }

func (b box) content(w int) []string {
	lw := b.leadW()
	if len(b.text) == 0 {
		cur := ""
		if b.focused && !b.idle {
			cur = reverse(" ")
		}
		return []string{b.lead + cur + faint(ansi.Truncate(b.holder, max(1, w-lw-1), "…"))}
	}
	segs := b.segs()
	start, end := b.window(segs)
	inChip := chipMask(b.text)
	from, to := -1, -1
	if b.anchor >= 0 && b.anchor != b.cursor {
		from, to = min(b.anchor, b.cursor), max(b.anchor, b.cursor)
	}
	var rows []string
	for i := start; i < end; i++ {
		sg := segs[i]
		var sb strings.Builder
		sb.Grow(len(b.lead) + (sg.to-sg.from)*2 + 64)
		lead := strings.Repeat(" ", lw)
		if i == 0 {
			lead = b.lead
		}
		sb.WriteString(lead)
		// The colour is set once per run rather than per character.
		const plain, text, chip, lit = 0, 1, 2, 3
		style := plain
		for p := sg.from; p < sg.to; p++ {
			switch {
			case b.focused && p == b.cursor:
				sb.WriteString("\x1b[7m")
				sb.WriteRune(b.text[p])
				sb.WriteString("\x1b[27m")
			case p >= from && p < to:
				sb.WriteString(bgMark + cText)
				sb.WriteRune(b.text[p])
				sb.WriteString(reset + bgInput)
				style = plain
			case inChip != nil && inChip[p]:
				st, col := chip, cBlue
				if p >= b.hot.from && p < b.hot.to {
					st, col = lit, cOrange
				}
				if style != st {
					sb.WriteString(bgChip + col)
					style = st
				}
				sb.WriteRune(b.text[p])
			default:
				if style == chip || style == lit {
					sb.WriteString(reset + bgInput)
				}
				if style != text {
					sb.WriteString(cText)
					style = text
				}
				sb.WriteRune(b.text[p])
			}
		}
		// The cursor at the end of a row shows as a block after it.
		last := i == len(segs)-1 || segs[i+1].from > sg.to
		if b.focused && b.cursor == sg.to && last {
			sb.WriteString(reverse(" "))
		}
		sb.WriteString(reset)
		rows = append(rows, sb.String())
	}
	return rows
}

// shownChipRe is a chip as a box draws it: a paste's, or an image's name.
var shownChipRe = regexp.MustCompile(pasteRe.String() + `|\[▣ [^\]\n]*\]`)

// chipMask marks the runes of each chip in text, nil when it has none, so
// a chip reads as one thing rather than words you typed.
func chipMask(text []rune) []bool {
	s := string(text)
	if !convo.HasPasteChip(s) && !strings.Contains(s, "[▣ ") {
		return nil
	}
	mask := make([]bool, len(text))
	for _, loc := range shownChipRe.FindAllStringIndex(s, -1) {
		from := len([]rune(s[:loc[0]]))
		for i := from; i < from+len([]rune(s[loc[0]:loc[1]])); i++ {
			mask[i] = true
		}
	}
	return mask
}

func reverse(s string) string { return "\x1b[7m" + s + "\x1b[27m" }

// named is b with each image in its text drawn as its name, the cursor,
// selection and lit chip moved to match.
func (b box) named(r imageRefs) box {
	text, pos := r.shown(b.text)
	if pos == nil {
		return b
	}
	b.text, b.pos = text, pos
	b.cursor = b.drawn(b.cursor)
	if b.anchor >= 0 {
		b.anchor = b.drawn(b.anchor)
	}
	if b.hot != (seg{}) {
		b.hot = seg{b.drawn(b.hot.from), b.drawn(b.hot.to)}
	}
	return b
}

// drawn is where text position p is drawn; textPos is the other way.
func (b box) drawn(p int) int {
	if b.pos == nil {
		return p
	}
	return sort.SearchInts(b.pos, p)
}

func (b box) textPos(d int) int {
	if b.pos == nil {
		return d
	}
	return b.pos[max(0, min(d, len(b.pos)-1))]
}

// at maps a click inside the box's text area (row from the first text row,
// col from the box's left edge) to a position in the text.
func (b box) at(row, col int) int {
	segs := b.segs()
	start, end := b.window(segs)
	i := start + row
	if i < start || i >= end {
		return b.textPos(b.cursor)
	}
	return b.textPos(b.col(segs[i], col))
}

// near is at for a drag, which can leave the box: above its first row is
// that row's start, below its last row is that row's end.
func (b box) near(row, col int) int {
	segs := b.segs()
	start, end := b.window(segs)
	switch {
	case len(segs) == 0:
		return 0
	case row < 0:
		return b.textPos(segs[start].from)
	case start+row >= end:
		return b.textPos(segs[end-1].to)
	}
	return b.textPos(b.col(segs[start+row], col))
}

// col is the text position under screen column col on the row sg.
func (b box) col(sg seg, col int) int {
	lw := b.leadW()
	x := col - 2 - lw // "│ " then the lead
	p, width := sg.from, 0
	for p < sg.to && width+runeW(b.text[p]) <= x {
		width += runeW(b.text[p])
		p++
	}
	return p
}
