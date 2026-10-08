package ui

import (
	"fmt"
	"math"
	"slices"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/theme"
)

type mood int

const (
	moodIdle mood = iota
	moodWorking
	moodNeedsYou
	moodSleepy
)

// clkState is everything clanker is drawn from.
type clkState struct {
	md   mood
	tick int   // seconds
	fx   clkFX // what he's reacting to, drawn at its own quicker pace
	rich bool  // today's spend is high: now and then a coin drops off him
}

// clanker draws the header: the RUSH bottle, a light passing over it now
// and then. It never moves; what changes is its colours, its cap, and what
// drifts off it.
func clanker(s clkState) []string {
	if clkLast.ok && clkLast.s == s && clkLast.ground == painted && clkLast.green == cGreen {
		return clkLast.lines
	}
	var g clkGrid
	g.sprite(s)
	clkLast.s, clkLast.ground, clkLast.green, clkLast.lines, clkLast.ok = s, painted, cGreen, g.lines(), true
	return clkLast.lines
}

// clkLast is the last drawing: most frames draw him as the one before.
var clkLast struct {
	s      clkState
	ground theme.Ground
	green  string
	lines  []string
	ok     bool
}

const (
	clkCycle = 45 // seconds between the idle bottle's shimmers
	clkShine = 14 // frames a light takes to pass over him
	clkW     = 9  // every frame's width, so the header's text never moves
	clkH     = 5
	clkX     = 1 // where it stands; the column either side is for what drifts off it
	clkBodyW = 6
	clkFace  = clkLabel // the label, all a narrow header has room for
	clkLabel = 3        // the row the label's text is on
)

// The bottle is 6×8 pixels, two to a cell, drawn ▀ in the top one's
// colour on the bottom one's, with its label a row of text between:
//
//	██████   cap
//	▀████▀   cap over neck
//	██████   shoulders
//	 RUSH    label
//	██████   body
//
// Each string is a row of pixels, a letter to a colour (clkInk); "." is
// none. clkCapped has its cap on, clkPopped has it lifted off to the side.
var (
	clkCapped = [clkH][2]string{
		{"GKGKGK", "KGKGKG"},
		{"kkkkkk", ".dAAa."},
		{"dAAAad", "dAAAad"},
		{},
		{"dAAAad", "dDDDDd"},
	}
	clkPopped = [clkH][2]string{
		{".GKGKG", ".kkkkk"},
		{"......", ".dAAa."},
		{"dAAAad", "dAAAad"},
		{},
		{"dAAAad", "dDDDDd"},
	}
	clkOpen = [clkH][2]string{
		{"......", "......"},
		{"......", ".dAAa."},
		{"dAAAad", "dAAAad"},
		{},
		{"dAAAad", "dDDDDd"},
	}
)

// clkGrid is a frame as cells, so what drifts off him lands around him
// without touching him, and a light can pass over only his body.
type clkGrid struct {
	r    [clkH][clkW]rune
	c    [clkH][clkW]string // foreground
	bg   [clkH][clkW]string // background, for an eye lit inside his head
	body [clkH][clkW]bool
	on   bool // he's drawn: fx keeps off his body
}

// put writes s from (y, x); its spaces leave cells as they are.
func (g *clkGrid) put(y, x int, s, c string, body bool) {
	for _, r := range s {
		if r != ' ' && y >= 0 && y < clkH && x >= 0 && x < clkW {
			g.r[y][x], g.c[y][x], g.body[y][x] = r, c, body
		}
		x++
	}
}

// dot drops one cell of fx, only where it's empty and off his body.
func (g *clkGrid) dot(y, x int, r rune, c string) {
	if y < 0 || y >= clkH || x < 0 || x >= clkW || g.r[y][x] != 0 {
		return
	}
	if g.on && y > 0 && x >= clkX && x < clkX+clkBodyW {
		return
	}
	g.r[y][x], g.c[y][x] = r, c
}

func (g *clkGrid) lines() []string {
	out := make([]string, clkH)
	for y := range g.r {
		var sb strings.Builder
		for x := 0; x < clkW; {
			c, bg, run := g.c[y][x], g.bg[y][x], x
			for run < clkW && g.c[y][run] == c && g.bg[y][run] == bg {
				run++
			}
			cells := []rune(string(g.r[y][x:run]))
			for i, r := range cells {
				if r == 0 {
					cells[i] = ' '
				}
			}
			switch {
			case c == "":
				sb.WriteString(string(cells))
			case bg != "":
				sb.WriteString(clkBG(bg) + paint(c, string(cells)))
			default:
				sb.WriteString(paint(c, string(cells)))
			}
			x = run
		}
		out[y] = sb.String()
	}
	return out
}

// shimmer passes a band of light over his body, low left to high right;
// f of n is how far across it is.
func (g *clkGrid) shimmer(f, n int, tint string, strength float64) {
	p := clkBand(f, n)
	for y := range g.r {
		for x := range g.r[y] {
			if !g.body[y][x] {
				continue
			}
			d := math.Abs(float64(x) + float64(y)*0.7 - p)
			if d < 2 {
				k := strength * (1 - d/2)
				g.c[y][x] = clkMix(g.c[y][x], tint, k)
				if g.bg[y][x] != "" {
					g.bg[y][x] = clkMix(g.bg[y][x], tint, k)
				}
			}
		}
	}
}

// clkInk is a pixel's colour: the cap's black and its ridges, the glass's
// amber, its shine and its shadow.
func clkInk(r byte) string {
	switch r {
	case 'K':
		return rgb(64, 64, 68)
	case 'G':
		return rgb(112, 112, 116)
	case 'k':
		return rgb(88, 88, 92)
	case 'A':
		return rgb(154, 76, 20)
	case 'a':
		return rgb(212, 128, 44)
	case 'd':
		return rgb(104, 46, 8)
	case 'D':
		return rgb(84, 38, 6)
	}
	return ""
}

// clkLabelBG is the label's yellow; needing you, it's orange.
func clkLabelBG(md mood) string {
	if md == moodNeedsYou {
		return cOrange
	}
	return rgb(250, 214, 30)
}

// bottle draws one of the bottle's frames at x, its label in bg.
func (g *clkGrid) bottle(x int, f [clkH][2]string, bg string) {
	ink := rgb(214, 28, 24) + bold // red on the yellow, dark on orange
	if bg == cOrange {
		ink = rgb(40, 16, 4) + bold
	}
	for y, row := range f {
		if y == clkLabel {
			for i, r := range " RUSH " {
				g.r[y][x+i], g.c[y][x+i], g.bg[y][x+i], g.body[y][x+i] = r, ink, bg, true
			}
			continue
		}
		for i := range row[0] {
			top, bot := clkInk(row[0][i]), clkInk(row[1][i])
			cx := x + i
			switch {
			case top != "" && bot != "":
				g.r[y][cx], g.c[y][cx], g.bg[y][cx] = '▀', top, bot
			case top != "":
				g.r[y][cx], g.c[y][cx] = '▀', top
			case bot != "":
				g.r[y][cx], g.c[y][cx] = '▄', bot
			default:
				continue
			}
			g.body[y][cx] = true
		}
	}
}

// sprite is the bottle. Working, its cap pops off and back every other
// second and a pale drop drifts off it; needing you, its cap is off, its
// label orange and a ! glows over it; asleep, it's dim and z's drift up.
func (g *clkGrid) sprite(s clkState) {
	md, tick, fx := s.md, s.tick, s.fx
	f := clkCapped
	switch md {
	case moodWorking:
		if tick/2%2 == 1 {
			f = clkPopped
		}
	case moodNeedsYou:
		f = clkOpen
		c := clkMix(cOrange, cFaint, .55)
		if tick%2 == 0 {
			c = cOrange
		}
		g.put(0, clkX+clkBodyW/2-1, "!", c, false)
	case moodSleepy:
		switch tick % 3 {
		case 0:
			g.put(1, clkX+clkBodyW+1, "z", cFaint, false)
		case 1:
			g.put(0, clkX+clkBodyW, "Z", cDim, false)
			g.put(1, clkX+clkBodyW+1, "z", cFaint, false)
		default:
			g.put(0, clkX+clkBodyW, "Z", cDim, false)
		}
	}
	g.bottle(clkX, f, clkLabelBG(md))
	g.on = true
	band := -99.0 // where the shimmer's colour is, off the bottle when there's none
	if fx.kind == fxShimmer {
		band = clkBand(fx.frame, clkShine)
	}
	if !darkGround() { // on a dark terminal the bottle wears its own colours
		g.mono(band, md == moodNeedsYou)
	}
	if md == moodSleepy {
		g.tint(cFaint, .5)
	}
	if tint, k := fx.tint(); k > 0 {
		g.tint(tint, k)
	}
	if fx.kind != fxNone {
		fx.draw(g)
		return
	}
	if md == moodWorking {
		g.drops(tick, []rune("·."), clkPaints(), 5)
	}
	if s.rich && tick%30 < 3 {
		g.dot(1+tick%30, clkW-1, []rune("$¢.")[tick%30], clkMix(cYellow, cFaint, float64(tick%30)*.3))
	}
}

// clkBand is where a band of light passing over the bottle is, f of n
// frames in: low left to high right, starting and ending off it.
func clkBand(f, n int) float64 {
	return -3 + float64(clkW+6)*float64(f)/float64(max(1, n-1))
}

// mono greys the bottle but within reach of band, where its own colours
// show through; needing you, the label stays orange.
func (g *clkGrid) mono(band float64, keepLabel bool) {
	for y := range g.r {
		for x := range g.r[y] {
			if !g.body[y][x] {
				continue
			}
			k := 1.0
			if d := math.Abs(float64(x) + float64(y)*0.7 - band); d < 2.5 {
				k = d / 2.5
			}
			g.c[y][x] = clkMix(g.c[y][x], clkGrey(g.c[y][x]), k) + boldOf(g.c[y][x])
			if g.bg[y][x] != "" && (!keepLabel || y != clkLabel) {
				g.bg[y][x] = clkMix(g.bg[y][x], clkGrey(g.bg[y][x]), k)
			}
		}
	}
}

// darkGround is whether the terminal's background is dark (or unknown yet).
func darkGround() bool { return painted == (theme.Ground{}) || painted.BG.Dark() }

// boldOf is bold when c is, which clkMix drops.
func boldOf(c string) string {
	if strings.HasSuffix(c, bold) {
		return bold
	}
	return ""
}

// clkGrey is a colour's grey in the theme's shades, between its
// background and its text: its lightness, pushed apart so the black cap,
// the glass and the white label stay distinct.
func clkGrey(c string) string {
	var r, g, b int
	if _, err := fmt.Sscanf(strings.TrimSuffix(c, bold), "\x1b[38;2;%d;%d;%dm", &r, &g, &b); err != nil {
		return c
	}
	l := (.3*float64(r) + .59*float64(g) + .11*float64(b)) / 255
	l = min(1, max(0, (l-.5)*1.6+.5))
	gr := painted
	if gr == (theme.Ground{}) {
		gr = theme.Dark
	}
	// Dark to light whichever the theme is, so the cap stays black; its
	// black a step off the background, so the lid shows on any theme.
	lo, hi := gr.BG, gr.FG
	if !lo.Dark() {
		lo, hi = hi, lo
	}
	out := theme.Mix(lo, hi, .2+l*.9).FG()
	if strings.HasSuffix(c, bold) {
		out += bold
	}
	return out
}

// tint mixes the bottle's colours k of the way to c.
func (g *clkGrid) tint(c string, k float64) {
	for y := range g.r {
		for x := range g.r[y] {
			if !g.body[y][x] {
				continue
			}
			g.c[y][x] = clkMix(g.c[y][x], c, k)
			if g.bg[y][x] != "" {
				g.bg[y][x] = clkMix(g.bg[y][x], c, k)
			}
		}
	}
}

// drops drift off him a second at a time: one falls a row a second down a
// side and fades. One in every `every` seconds starts one.
func (g *clkGrid) drops(tick int, look []rune, paints []string, every int) {
	for age, l := range slices.Backward(look) {
		h := clkHash(tick - age)
		if h%every != 0 {
			continue
		}
		x := h / 7 % 2
		if h/3%2 == 0 {
			x = clkW - 1 - x
		}
		c := clkMix(paints[h/11%len(paints)], cFaint, .3+.4*float64(age))
		g.dot(2+age, x, l, c)
	}
}

func clkPaints() []string { return []string{cOrange, cYellow, cBlue, cGreen} }

// clkBG is a palette colour as a background.
func clkBG(c string) string {
	return strings.Replace(strings.TrimSuffix(c, bold), "[38;", "[48;", 1)
}

// clkMix blends two of the palette's colours, t of the way from a to b;
// bold on either is dropped.
func clkMix(a, b string, t float64) string {
	t = min(1, max(0, t))
	var ar, ag, ab, br, bg, bb int
	_, errA := fmt.Sscanf(strings.TrimSuffix(a, bold), "\x1b[38;2;%d;%d;%dm", &ar, &ag, &ab)
	_, errB := fmt.Sscanf(strings.TrimSuffix(b, bold), "\x1b[38;2;%d;%d;%dm", &br, &bg, &bb)
	if errA != nil || errB != nil {
		if t < .5 {
			return a
		}
		return b
	}
	l := func(x, y int) int { return int(math.Round(float64(x) + (float64(y)-float64(x))*t)) }
	return rgb(l(ar, br), l(ag, bg), l(ab, bb))
}

// clkHash scatters n so drops and sparks land somewhere new each time but
// the same frame always draws the same.
func clkHash(n int) int {
	x := uint32(n) * 2654435761
	x ^= x >> 15
	x *= 2246822519
	x ^= x >> 13
	return int(x % 100003)
}

// fxKind is something clanker is reacting to, weakest first: a stronger
// one takes over a weaker one already running, never the other way.
type fxKind int

const (
	fxNone     fxKind = iota
	fxShimmer         // a light passes over him, now and then
	fxDone            // an agent finished: a glint and a drop
	fxAnswered        // one you kept waiting moved on: he greens, confetti
	fxAsk             // one started waiting on you: streaks fly off him
	fxError           // one hit an error, or rush did: he reddens, sparks
)

// clkFX is a reaction and how far into it he is, in frames of fxEvery.
type clkFX struct {
	kind  fxKind
	frame int
}

const fxEvery = 70 * time.Millisecond

var fxLen = map[fxKind]int{
	fxShimmer: clkShine,
	fxDone:    14, fxAnswered: 22, fxAsk: 18, fxError: 16,
}

type fxTickMsg struct{}

func fxTick() tea.Cmd {
	return tea.Tick(fxEvery, func(time.Time) tea.Msg { return fxTickMsg{} })
}

// tint is the colour a reaction washes the bottle in, and how far, easing
// back as it goes.
func (fx clkFX) tint() (string, float64) {
	f := float64(fx.frame)
	switch fx.kind {
	case fxAsk:
		return cYellow, .5 * max(0, 1-f/10)
	case fxAnswered:
		return cGreen, .5 * max(0, 1-f/14)
	case fxError:
		return cRed, .6 * max(0, 1-f/12)
	}
	return "", 0
}

// draw is what comes off him.
func (fx clkFX) draw(g *clkGrid) {
	f := fx.frame
	l, r := clkX-1, clkX+clkBodyW // the columns just off each side of him
	switch fx.kind {
	case fxShimmer: // sprite lets the bottle's colour through where it passes
	case fxAsk:
		// Streaks out from him, a tip with a short tail, fading as they
		// go; then a yellow light passes over him.
		type ray struct {
			y, x, dy, dx int
			r            rune
		}
		rays := []ray{
			{2, l, 0, -1, '─'}, {2, r, 0, 1, '─'},
			{1, l, -1, -1, '╲'}, {1, r, -1, 1, '╱'},
			{3, l, 1, -1, '╱'}, {3, r, 1, 1, '╲'},
		}
		if f < 8 {
			step := f/2 + 1
			c := clkMix(cYellow, cFaint, float64(f)/8)
			for _, ry := range rays {
				for s := max(0, step-2); s < step && s < 2; s++ {
					ch, cc := '·', clkMix(c, cFaint, .5)
					if s == step-1 {
						ch, cc = ry.r, c
					}
					g.dot(ry.y+ry.dy*s, ry.x+ry.dx*s, ch, cc)
				}
			}
		}
		if f >= 4 {
			g.shimmer(f-4, clkShine, cYellow, .8)
		}
	case fxAnswered:
		// Confetti drifting down past him, dimming as it falls.
		paints := append(clkPaints(), cText)
		for i := 0; i < 7; i++ {
			h := clkHash(1000 + i)
			y := f/2 - h%5
			x := []int{0, 1, clkW - 2, clkW - 1, clkX + 1, clkX + 5, clkX + 9}[i]
			if y < 0 || y >= clkH {
				continue
			}
			g.dot(y, x, []rune("·•°+")[h/9%4], clkMix(paints[h/13%len(paints)], cFaint, float64(y)*.25))
		}
		if f >= 6 {
			g.shimmer(f-6, clkShine, cGreen, .5)
		}
	case fxDone:
		// A glint across him, and a drop off each side.
		g.shimmer(f, fxLen[fxDone], cText, .5)
		for i, x := range []int{l, r} {
			y := (f+2*i)/4 + 1
			g.dot(y, x, '·', clkMix(cOrange, cFaint, float64(f)/14))
		}
	case fxError:
		// A few red sparks, then nothing.
		if f < 9 && f%3 == 0 {
			for i := 0; i < 2; i++ {
				h := clkHash(5000 + f*3 + i)
				g.dot(h%clkH, []int{0, 1, clkW - 2, clkW - 1}[h/5%4], []rune("×·")[h/7%2], clkMix(cRed, cFaint, float64(f)/10))
			}
		}
	}
}

// react starts a reaction, unless a stronger one is running.
func (m *Model) react(k fxKind) {
	if k < m.fx.kind {
		return
	}
	m.fx = clkFX{kind: k}
	if !m.fxOn {
		m.fxOn, m.fxKick = true, true
	}
}

// onFXTick moves the reaction on a frame, and stops ticking when it's over.
func (m *Model) onFXTick() tea.Cmd {
	m.fx.frame++
	if m.fx.frame >= fxLen[m.fx.kind] {
		m.fx, m.fxOn = clkFX{}, false
		return nil
	}
	return fxTick()
}

// clkBeat is clanker's second: now and then a light passes over it.
func (m *Model) clkBeat(md mood) {
	if m.fx.kind != fxNone || m.zen {
		return
	}
	if md == moodWorking && m.tick%15 == 8 || md == moodIdle && m.tick%clkCycle == 25 {
		m.react(fxShimmer)
	}
}

func (m *Model) clkState(md mood, t tally) clkState {
	return clkState{md: md, tick: m.tick, fx: m.fx, rich: t.today >= 500}
}
