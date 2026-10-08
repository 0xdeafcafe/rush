package convo

import (
	"fmt"
	"runtime"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/photon/theme"
	"github.com/charmbracelet/x/ansi"
)

const (
	reset = "\x1b[0m"
	bold  = "\x1b[1m"
)

// rush's palette, with dim and faint raised so anything you read clears
// 4.5:1 and faint is left for decoration. SetColours makes it for the
// terminal's ground.
var (
	cText, cSub, cDim, cFaint, cWhite     string
	cOrange, cGreen, cYellow, cRed, cBlue string
	cOut                                  string // what tools print: under faint text, over faint rules
	cOKq                                  string // a finished step's tick, quiet like its row
	// A card's frame says what its command did: made something (cOKq),
	// rewrote or set something aside (cWarnQ), threw something away or
	// failed (cLostQ); cLost is that last one's glyph.
	cWarnQ, cLost, cLostQ string
)

// Surfaces. The ground is the terminal's own background, so nothing here
// paints a slab over a themed terminal; only raised or tinted rows get one.
var (
	bgWell    string // output, diffs: none, only what you said has a ground
	bgUser    string // what you said: one step off the terminal's ground
	bgFailure string // a quiet full-row tint for failures
	bgAdd     string
	bgDel     string
	// A diff line's changed words, a step brighter than the line.
	bgAddHi, bgDelHi string
)

// spineBar is the one thin rule down a turn's left edge, the same quiet
// colour for every turn: running, done or failed.
var spineBar string

// palette is how many times the colours have changed, so a cached drawing
// in the old ones isn't used.
var palette int

// Palette is how many times the colours have changed, for a caller's own
// cache of what it drew in them.
func Palette() int { return palette }

func init() { SetColours(theme.Dark, false) }

// SetColours makes the palette for the terminal's ground g. colorBlind
// swaps green and red, what rush uses for added and removed, done and
// failed, for sky blue and amber (from the Okabe-Ito palette), which stay
// apart for every common kind of colour blindness.
func SetColours(g theme.Ground, colorBlind bool) {
	ink := func(r, gr, b uint8) string { return g.Ink(theme.RGB{R: r, G: gr, B: b}).FG() }
	accent := func(c theme.RGB) string { return g.Accent(c).FG() }
	quiet := func(c, of theme.RGB) string { return g.Quiet(c, of).FG() }
	surface := func(r, gr, b uint8) string { return g.Surface(theme.RGB{R: r, G: gr, B: b}).BG() }

	cText, cSub = ink(226, 221, 211), ink(168, 162, 152)
	cDim, cFaint = ink(138, 132, 122), ink(94, 89, 82)
	cWhite, cOut = ink(240, 236, 228), ink(119, 113, 106)
	cOrange, cYellow, cBlue = accent(orange), accent(yellow), accent(blue)
	for name, c := range ctxHues {
		ctxInk[name] = accent(c)
	}
	cWarnQ = quiet(theme.RGB{R: 168, G: 136, B: 82}, yellow)
	bgUser = surface(0x1a, 0x18, 0x16)
	bgFailure = surface(0x2a, 0x1a, 0x1a)
	if colorBlind {
		bgFailure = surface(0x2c, 0x24, 0x16)
	}

	green, red := theme.RGB{R: 127, G: 191, B: 138}, theme.RGB{R: 224, G: 104, B: 92}
	if colorBlind {
		green, red = theme.RGB{R: 86, G: 180, B: 233}, theme.RGB{R: 230, G: 159}
		// Amber is taken by failed; lost is vermillion, clear of warn.
		lost := theme.RGB{R: 213, G: 94}
		cGreen, cRed, cOKq = accent(green), accent(red), quiet(theme.RGB{R: 80, G: 140, B: 180}, green)
		cLost, cLostQ = accent(lost), quiet(theme.RGB{R: 160, G: 74, B: 12}, lost)
		bgAdd, bgDel = surface(0x10, 0x2a, 0x3c), surface(0x30, 0x24, 0x0e)
		bgAddHi, bgDelHi = surface(0x1a, 0x46, 0x64), surface(0x52, 0x3a, 0x10)
	} else {
		cGreen, cRed, cOKq = accent(green), accent(red), quiet(theme.RGB{R: 95, G: 138, B: 104}, green)
		cLost, cLostQ = accent(red), quiet(theme.RGB{R: 170, G: 86, B: 76}, red)
		bgAdd, bgDel = surface(0x16, 0x30, 0x1a), surface(0x3a, 0x17, 0x14)
		bgAddHi, bgDelHi = surface(0x22, 0x52, 0x2b), surface(0x62, 0x24, 0x1e)
	}
	spineBar = paint(cFaint, "▏")
	hlKw, hlStr = accent(theme.RGB{R: 204, G: 153, B: 205}), accent(theme.RGB{R: 163, G: 190, B: 140})
	hlNum, hlFn = accent(theme.RGB{R: 222, G: 165, B: 132}), accent(theme.RGB{R: 137, G: 180, B: 222})
	hlType = accent(theme.RGB{R: 120, G: 190, B: 175})
	hlComment, hlOutComment, hlSpace = ink(122, 116, 108), ink(92, 87, 80), ink(92, 88, 82)
	palette++
}

// rush's accents as they are on its dark ground.
var (
	orange = theme.RGB{R: 217, G: 119, B: 87}
	yellow = theme.RGB{R: 229, G: 181, B: 103}
	blue   = theme.RGB{R: 143, G: 179, B: 217}
)

// cursor is the keyboard's mark in the gutter: one thin neutral stroke, the
// same whoever has focus, so a row is never lit by where you clicked.
func cursor() string { return paint(cSub, "▏") }

func paint(c, s string) string {
	if s == "" {
		return ""
	}
	return c + s + reset
}

// plusSign and minusSign mark a diff's added and removed lines.
func plusSign() string  { return paint(cGreen+bold, "+") }
func minusSign() string { return paint(cRed+bold, "−") }

func text(s string) string  { return paint(cText, s) }
func sub(s string) string   { return paint(cSub, s) }
func dim(s string) string   { return paint(cDim, s) }
func faint(s string) string { return paint(cFaint, s) }

// folded says n lines are folded away: the count in bold and the key that
// unfolds them in orange, so the gap reads as something you can open.
func folded(n int) string {
	return dim("… ") + paint(cText+bold, fmt.Sprint(n)) + dim(" lines ") + faint("·") + " " +
		paint(cOrange+bold, "ctrl+o") + dim(" shows all")
}

var spinner = []string{"·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"}

// spinners are each agent's own spinner, as its own program draws it, so
// what's running shows at a glance; Claude Code's is spinner.
var spinners = map[string][]string{
	"codex":  {"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"},
	"ollama": {"◐", "◓", "◑", "◒"},
	"pi":     {"◜", "◝", "◞", "◟"},
	"gemini": {"✦", "✧", "✦", "✧", "·"},
}

// spin is the spinner's frame at tick for the session's agent.
func (d *drawer) spin(tick int) string {
	if d.s == nil {
		return Spin("", tick)
	}
	return Spin(d.s.Info.Kind, tick)
}

// Spin is the spinner's frame at tick for agent k: an agent through
// Ollama, in any harness, spins as Ollama.
func Spin(k string, tick int) string {
	if strings.HasPrefix(k, "ollama") {
		k = "ollama"
	}
	fs := spinners[k]
	if fs == nil {
		fs = spinner
	}
	return fs[tick%len(fs)]
}

// musings are what a turn thinking is called, one a stretch of thinking:
// mostly British, some tied up in knots.
var musings = []string{
	"thinking", "pondering", "mulling it over", "cogitating", "chin-stroking", "ruminating",
	"faffing about", "dillydallying", "pottering", "dithering", "chuntering", "noodling",
	"having a cuppa", "putting the kettle on", "rummaging", "tinkering", "bodging", "waffling",
	"processing", "inhaling", "taking a deep breath", "rigging", "tying knots",
	"getting tied up in knots", "tying up loose ends", "unpicking it", "gubbing", "wrangling",
	"watching the langs", "binding", "experiencing euphoria", "microgurning",
	"implicating", "golden godding", "consulting bird law",
	"wildcarding", "fighting the Nightman", "rum hamming", "boiling denim",
	"fixing the gas crisis", "opening Hormuz", "closing the Strait", "good boy",
	"consulting the rubber duck", "staring into the middle distance", "reticulating splines",
	"asking its mum", "counting sheep", "doing a little jig", "stroking an imaginary beard",
	"having a biscuit", "reading the manual for once", "pretending to understand",
	"feeding the hamsters", "spinning up the hamster wheel", "herding cats", "shaking the magic 8-ball",
}

// row lays left and right out across width cells on background b ("" for
// the terminal's own), with the right half ending at the content cap so
// numbers stay near their labels on very wide panes.
func row(b, left, right string, width, capw int) string {
	if capw > width || capw <= 0 {
		capw = width
	}
	rw := cellw.String(right)
	if rw > capw-4 {
		right, rw = "", 0
	}
	room := capw - rw
	if rw > 0 {
		room -= 2
	}
	lw := cellw.String(left)
	if lw > room {
		left = cellw.Truncate(left, max(0, room), "…")
		lw = cellw.String(left)
	}
	gap := max(0, capw-lw-rw)
	var sb strings.Builder
	sb.Grow(len(b) + len(left) + gap + len(right) + max(0, width-capw) + 32)
	if b == "" {
		sb.WriteString(left)
		sb.WriteString(blanks(gap))
		sb.WriteString(right)
		sb.WriteString(blanks(width - capw))
		return sb.String()
	}
	// Every reset inside returns to the row's background.
	sb.WriteString(b)
	writeIn(&sb, left, b)
	sb.WriteString(blanks(gap))
	writeIn(&sb, right, b)
	sb.WriteString(blanks(width - capw))
	sb.WriteString(reset)
	return sb.String()
}

// writeIn writes s with every reset followed by bg.
func writeIn(sb *strings.Builder, s, bg string) {
	for {
		i := strings.Index(s, reset)
		if i < 0 {
			sb.WriteString(s)
			return
		}
		sb.WriteString(s[:i+len(reset)])
		sb.WriteString(bg)
		s = s[i+len(reset):]
	}
}

// fastUnder is how long a timer shows tenths of a second for, and ticks
// in them while it runs.
const fastUnder = 5 * time.Second

func dur(d time.Duration) string {
	switch {
	case d < 0:
		return ""
	case d < fastUnder:
		return fmt.Sprintf("%.1fs", d.Seconds())
	case d < time.Minute:
		return fmt.Sprintf("%ds", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm %02ds", int(d.Minutes()), int(d.Seconds())%60)
	default:
		return fmt.Sprintf("%dh %02dm", int(d.Hours()), int(d.Minutes())%60)
	}
}

func money(v float64) string {
	if v <= 0 {
		return ""
	}
	return fmt.Sprintf("$%.2f", v)
}

func oneLine(s string) string {
	if single(s) {
		return s
	}
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

// wrap breaks styled text to w cells.
func wrap(s string, w int) []string {
	if w < 10 {
		w = 10
	}
	rows := strings.Split(ansi.Wrap(s, w, ""), "\n")
	// A row holding nothing but style codes is an artefact of the wrap;
	// fold its codes into the row before it.
	for len(rows) > 1 && strings.TrimSpace(ansi.Strip(rows[len(rows)-1])) == "" {
		rows[len(rows)-2] += rows[len(rows)-1]
		rows = rows[:len(rows)-1]
	}
	return CarryStyle(rows)
}

// CarryStyle reopens, at the start of each wrapped row, the style still
// open at the end of the row before, and closes it at the row's end.
// Each row is drawn on its own, so without this a paragraph's second row
// on is in the terminal's own colour, not the paragraph's.
//
// A link is carried the same way: a row that ends inside one closes it
// and the next opens it again, or the terminal is left with half a link.
func CarryStyle(rows []string) []string {
	open, link := "", ""
	for i, r := range rows {
		carried, linked := open, link
		open, link = openStyle(open, r), openLink(link, r)
		// One string made per row, not one per addition.
		switch {
		case carried != "" && open != "":
			r = carried + r + reset
		case carried != "":
			r = carried + r
		case open != "":
			r += reset
		}
		pre, post := "", ""
		if linked != "" {
			pre = "\x1b]8;;" + linked + "\x1b\\"
		}
		if link != "" {
			post = "\x1b]8;;\x1b\\"
		}
		if pre != "" || post != "" {
			r = pre + r + post
		}
		rows[i] = r
	}
	return rows
}

// openLink is the link still open after s, given the one open before it.
func openLink(open, s string) string {
	for {
		i := strings.Index(s, "\x1b]8;")
		if i < 0 {
			return open
		}
		s = s[i+len("\x1b]8;"):]
		// Its parameters, then the URL, to the string terminator.
		if j := strings.IndexByte(s, ';'); j >= 0 {
			s = s[j+1:]
		}
		end, n := strings.Index(s, "\x1b\\"), 2
		if k := strings.IndexByte(s, '\a'); k >= 0 && (end < 0 || k < end) {
			end, n = k, 1
		}
		if end < 0 {
			return open
		}
		open, s = s[:end], s[end+n:]
	}
}

// openStyle is the style codes open after s, given open before it.
func openStyle(open, s string) string {
	for {
		i := strings.Index(s, "\x1b[")
		if i < 0 {
			return open
		}
		s = s[i+2:]
		j := 0
		for j < len(s) && (s[j] >= '0' && s[j] <= '9' || s[j] == ';' || s[j] == ':') {
			j++
		}
		if j == len(s) || s[j] != 'm' {
			continue
		}
		params := s[:j]
		if params == "" || params == "0" || strings.HasPrefix(params, "0;") {
			open = ""
		}
		if params != "" && params != "0" {
			open += "\x1b[" + params + "m"
		}
		s = s[j+1:]
	}
}

// KeyWord is a key's name as your keyboard says it: a Mac has no alt
// key, so alt+ is ⌥ there.
func KeyWord(s string) string {
	if runtime.GOOS == "darwin" {
		return strings.ReplaceAll(s, "alt+", "⌥")
	}
	return s
}
