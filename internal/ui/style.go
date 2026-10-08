package ui

import (
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/photon/theme"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/charmbracelet/x/ansi"
)

const (
	reset = "\x1b[0m"
	bold  = "\x1b[1m"
)

func rgb(r, g, b int) string { return fmt.Sprintf("\x1b[38;2;%d;%d;%dm", r, g, b) }

var cOrange, cText, cSub, cDim, cFaint, cGreen, cYellow, cRed, cBlue string

// painted is the ground the colours are made for.
var painted theme.Ground

func init() { applyColors(theme.Dark, false) }

// applyColors makes rush's colours, here and in the conversation view,
// for the terminal's ground g: each is written as it is on rush's own
// dark ground and moved onto g as text (ink), a ground for a row or
// panel (surface), or a colour that says something (accent). colorBlind
// makes good and bad sky blue and amber, not green and red.
func applyColors(g theme.Ground, colorBlind bool) {
	c := func(r, gr, b uint8) theme.RGB { return theme.RGB{R: r, G: gr, B: b} }
	ink := func(r, gr, b uint8) string { return g.Ink(c(r, gr, b)).FG() }
	accent := func(r, gr, b uint8) string { return g.Accent(c(r, gr, b)).FG() }
	surface := func(r, gr, b uint8) string { return g.Surface(c(r, gr, b)).BG() }

	cText, cSub, cDim, cFaint = ink(226, 221, 211), ink(168, 162, 152), ink(122, 117, 108), ink(72, 68, 63)
	cBright, cEdge = ink(240, 236, 228), ink(79, 73, 67)
	cOrange, cYellow, cBlue = accent(217, 119, 87), accent(229, 181, 103), accent(143, 179, 217)
	cQueue = accent(178, 160, 214)
	if colorBlind {
		cGreen, cRed = accent(86, 180, 233), accent(230, 159, 0)
	} else {
		cGreen, cRed = accent(127, 191, 138), accent(224, 104, 92)
	}

	selBG, hoverBG, panelBG = surface(44, 40, 36), surface(33, 31, 29), surface(30, 28, 26)
	askBG, askSelBG, errBG, errSelBG = surface(44, 38, 24), surface(62, 52, 30), surface(46, 28, 26), surface(66, 36, 32)
	bgChrome, bgTabOn, bgBtw = surface(30, 28, 26), surface(17, 16, 14), surface(36, 33, 30)
	bgSub, bgRuns, bgQueue = surface(24, 31, 42), bgChrome, bgChrome
	bgInput, bgMark, bgChip = surface(40, 36, 32), surface(74, 64, 54), surface(56, 62, 72)
	qCard, qSel, qCap = bgChrome, selBG, surface(48, 44, 40)
	kbHitBG = surface(250, 236, 214)
	barChip, edSelBG, edErrBG = surface(64, 45, 37), surface(72, 62, 52), surface(96, 42, 38)
	barShadow = surface(12, 11, 10) + ink(44, 41, 38)
	selBlue = surface(58, 78, 122) + cBright
	tabOff = cSub + surface(40, 37, 34)

	painted = g
	fade, faded = theme.Mix(g.FG, g.BG, fadeBy).FG(), map[string]string{}
	convo.SetColours(g, colorBlind)
}

func paint(c, s string) string {
	if s == "" {
		return ""
	}
	return c + s + reset
}

func dim(s string) string   { return paint(cDim, s) }
func faint(s string) string { return paint(cFaint, s) }

// fit pads or truncates to exactly w cells.
func fit(s string, w int) string {
	if w <= 0 {
		return ""
	}
	sw := cellw.String(s)
	if sw > w {
		return cellw.Truncate(s, w, "…")
	}
	return s + strings.Repeat(" ", w-sw)
}

// fitTo writes fit(s, w) with every reset in it followed by bg, when set,
// so a faded or tinted row stays so after each styled piece.
func fitTo(b *strings.Builder, s string, w int, bg string) {
	if w <= 0 {
		return
	}
	sw := cellw.String(s)
	if sw > w {
		writeIn(b, cellw.Truncate(s, w, "…"), bg)
		return
	}
	writeIn(b, s, bg)
	b.WriteString(blanks(w - sw))
}

// writeIn writes s with every reset followed by bg, when set.
func writeIn(b *strings.Builder, s, bg string) {
	if bg == "" {
		b.WriteString(s)
		return
	}
	for {
		i := strings.Index(s, reset)
		if i < 0 {
			b.WriteString(s)
			return
		}
		b.WriteString(s[:i+len(reset)])
		b.WriteString(bg)
		s = s[i+len(reset):]
	}
}

const spaces = "                                                                                                                                                                                                                                                                "

// blanks is n spaces.
func blanks(n int) string {
	if n <= 0 {
		return ""
	}
	if n <= len(spaces) {
		return spaces[:n]
	}
	return strings.Repeat(" ", n)
}

// right aligns to exactly w cells.
func right(s string, w int) string {
	sw := cellw.String(s)
	if sw >= w {
		return cellw.Truncate(s, w, "…")
	}
	return strings.Repeat(" ", w-sw) + s
}

func oneLine(s string) string {
	if single(s) {
		return s
	}
	s = strings.ReplaceAll(s, "\r", " ")
	s = strings.ReplaceAll(s, "\n", " ")
	s = strings.ReplaceAll(s, "\t", " ")
	return strings.Join(strings.Fields(s), " ")
}

// single is whether s is already one line of single-spaced ASCII words, so
// oneLine has nothing to do.
func single(s string) bool {
	if s == "" || s[0] == ' ' || s[len(s)-1] == ' ' {
		return s == ""
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c < 0x21 && (c != ' ' || s[i+1] == ' ') || c >= 0x7f {
			return false
		}
	}
	return true
}

// age matches the native view: 3s, 12m, 4h, 2d.
func age(d time.Duration) string {
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%ds", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 48*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	default:
		return fmt.Sprintf("%dd", int(d.Hours()/24))
	}
}

func dur(d time.Duration) string {
	switch {
	case d <= 0:
		return "–"
	case d < time.Minute:
		return fmt.Sprintf("%ds", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d >= 48*time.Hour:
		// Past two days, days and hours read better than 226h46m.
		return fmt.Sprintf("%dd%02dh", int(d.Hours())/24, int(d.Hours())%24)
	default:
		return fmt.Sprintf("%dh%02dm", int(d.Hours()), int(d.Minutes())%60)
	}
}

func mem(b uint64) string {
	switch {
	case b == 0:
		return "–"
	case b >= 1<<30:
		return fmt.Sprintf("%.1fG", float64(b)/(1<<30))
	default:
		return fmt.Sprintf("%dM", b>>20)
	}
}

func money(c float64) string {
	switch {
	case c <= 0:
		return "–"
	case c < 10:
		return fmt.Sprintf("$%.2f", c)
	default:
		return "$" + thousands(int64(c+0.5))
	}
}

// moneyShort is money in at most six characters, for a column: past $9,999
// it counts thousands, so it never runs into the column beside it.
func moneyShort(c float64) string {
	switch {
	case c < 10_000:
		return money(c)
	case c < 99_950:
		return fmt.Sprintf("$%.1fk", c/1e3)
	case c < 999_500:
		return fmt.Sprintf("$%.0fk", c/1e3)
	default:
		return fmt.Sprintf("$%.1fM", c/1e6)
	}
}

func thousands(n int64) string {
	s := fmt.Sprintf("%d", n)
	for i := len(s) - 3; i > 0; i -= 3 {
		s = s[:i] + "," + s[i:]
	}
	return s
}

func tokens(n int64) string {
	switch {
	case n >= 1_000_000 && n%1_000_000 == 0:
		return fmt.Sprintf("%dM", n/1_000_000)
	case n >= 1_000_000:
		return fmt.Sprintf("%.1fM", float64(n)/1e6)
	case n >= 1000:
		return fmt.Sprintf("%dk", n/1000)
	default:
		return fmt.Sprintf("%d", n)
	}
}

func cpuColor(p float64, s string) string {
	switch {
	case p >= 50:
		return paint(cYellow, s)
	case p >= 1:
		return paint(cGreen, s)
	default:
		return s
	}
}

func memColor(b uint64, s string) string {
	if b >= 1<<30 {
		return paint(cYellow, s)
	}
	return s
}

// bar draws a 10-cell usage meter.
func bar(pct float64) string {
	n := int(pct/10 + 0.5)
	if n > 10 {
		n = 10
	}
	if n < 0 {
		n = 0
	}
	c := cGreen
	switch {
	case pct >= 80:
		c = cRed
	case pct >= 50:
		c = cYellow
	}
	return paint(c, strings.Repeat("▰", n)) + faint(strings.Repeat("▱", 10-n))
}

var spinner = []string{"·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"}

func wrap(s string, w int) []string {
	if w < 8 {
		w = 8
	}
	var out []string
	for _, para := range strings.Split(s, "\n") {
		para = strings.TrimRight(para, " ")
		if para == "" {
			out = append(out, "")
			continue
		}
		out = append(out, convo.CarryStyle(strings.Split(ansi.Wrap(para, w, " -/"), "\n"))...)
	}
	return out
}

var selBG string

// highlight paints a full-width selection bar that survives inner resets.
func highlight(line string, w int) string {
	line = fit(line, w)
	return selBG + strings.ReplaceAll(line, reset, reset+selBG) + reset
}

// rule is a section title with a hairline running to the edge.
func rule(title, meta string, w int) string {
	head := paint(cSub+bold, title)
	if meta != "" {
		head += "  " + dim(meta)
	}
	n := w - cellw.String(head) - 2
	if n < 0 {
		n = 0
	}
	return head + " " + faint(strings.Repeat("─", n))
}

// tidyFit is fit for a sentence that may not fit: your home becomes ~,
// a long path keeps its start and its file ("~/.config/…/memory/note.md"),
// and what still runs over ends on a word, not mid-way through one.
func tidyFit(s string, w int) string {
	if w <= 0 || cellw.String(s) <= w {
		return fit(s, w)
	}
	if home, _ := os.UserHomeDir(); home != "" {
		s = strings.ReplaceAll(s, home+"/", "~/")
	}
	words := strings.Split(s, " ")
	for keep := 2; keep >= 1 && cellw.String(strings.Join(words, " ")) > w; keep-- {
		for i, x := range words {
			// A path keeps its first folder and its last parts.
			if parts := strings.Split(x, "/"); len(parts) > keep+2 {
				words[i] = parts[0] + "/" + parts[1] + "/…/" + strings.Join(parts[len(parts)-keep:], "/")
			}
		}
	}
	s = strings.Join(words, " ")
	if cellw.String(s) <= w {
		return fit(s, w)
	}
	cut := cellw.Truncate(s, w-1, "")
	if i := strings.LastIndexByte(cut, ' '); s[len(cut)] != ' ' && i > 0 && cellw.String(cut[:i]) >= w*2/3 {
		cut = cut[:i] // ends on a word
	}
	return fit(strings.TrimRight(cut, " ,.;:·-")+"…", w)
}
