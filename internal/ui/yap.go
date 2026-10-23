package ui

import (
	"os"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/theme"

	"github.com/0xdeafcafe/rush/internal/bundled/yap"
)

// A yap dictation shows in the box that has the keys, as yap hears it:
// the words it's sure of in the text's colour, the ones it's still
// guessing dimmer with a light passing over them, a cursor that brightens
// as you speak, and a light running round the box's edge. When you let go
// yap pastes, and that paste goes in as text however long it is: it's
// what you said, not a log to fold away.

// yapLive is the window's view of yap.
type yapLive struct {
	ch    chan yap.State
	stop  chan struct{}
	s     yap.State
	frame int  // the look's animation, a step each yapEvery
	gen   int  // the tick under way; an older one stops
	ticks bool // a tick is under way
	// final is a finished dictation's text until its paste arrives, and
	// when yap said it.
	final   string
	finalAt time.Time
}

type yapMsg struct{ s yap.State }

type yapTickMsg struct{ gen int }

const yapEvery = 50 * time.Millisecond

// yapPasteWait is how long after yap says it's done its paste is taken as
// the dictation's.
const yapPasteWait = 5 * time.Second

// yapApp is the app rush's terminal is, as macOS tells programs started in
// it, so a dictation going to another app isn't shown here.
var yapApp = os.Getenv("__CFBundleIdentifier")

// yapSync follows yap while its plugin is on and stops when it's off.
func (m *Model) yapSync() tea.Cmd {
	on, ok := m.bundledOn[yap.Manifest.Name]
	if !ok || !on {
		if m.yap.stop != nil {
			close(m.yap.stop)
		}
		m.yap = yapLive{}
		return nil
	}
	if m.yap.ch != nil {
		return nil
	}
	ch, stop := make(chan yap.State, 64), make(chan struct{})
	m.yap.ch, m.yap.stop = ch, stop
	go func() {
		defer close(ch)
		yap.Follow(stop, func(s yap.State) {
			select {
			case ch <- s:
			default: // the window's behind: it'll have the next
			}
		})
	}()
	return yapNext(ch)
}

func yapNext(ch chan yap.State) tea.Cmd {
	return func() tea.Msg {
		s, ok := <-ch
		if !ok {
			return nil
		}
		return yapMsg{s}
	}
}

func yapTick(gen int) tea.Cmd {
	return tea.Tick(yapEvery, func(time.Time) tea.Msg { return yapTickMsg{gen} })
}

// onYap takes what yap says, and starts the look's animation when a
// dictation starts.
func (m *Model) onYap(s yap.State) tea.Cmd {
	m.yap.s = s
	if s.Phase == "done" {
		m.yap.final, m.yap.finalAt = s.Settled, time.Now()
	}
	next := yapNext(m.yap.ch)
	if !s.Active() || m.yap.ticks {
		return next
	}
	m.yap.ticks = true
	m.yap.gen++
	return tea.Batch(next, yapTick(m.yap.gen))
}

func (m *Model) onYapTick(gen int) tea.Cmd {
	if gen != m.yap.gen {
		return nil
	}
	if !m.yap.s.Active() {
		m.yap.ticks = false
		return nil
	}
	m.yap.frame++
	if !m.yapShown() {
		m.gate.Keep()
	}
	return yapTick(gen)
}

// yapShown is whether this window shows the dictation: one is under way,
// rush's terminal has the keys, and it's going to rush's terminal.
func (m *Model) yapShown() bool {
	s := m.yap.s
	return s.Active() && !m.blurred && (s.App == "" || yapApp == "" || s.App == yapApp)
}

// yapPaste is whether a paste is the dictation yap just finished, which
// goes in as typed. Any paste ends the dictation's look.
func (m *Model) yapPaste(text string) bool {
	final, at := m.yap.final, m.yap.finalAt
	m.yap.final = ""
	if m.yap.s.Phase == "done" || m.yap.s.Phase == "finishing" {
		m.yap.s = yap.State{Phase: "hidden"}
	}
	return final != "" && time.Since(at) < yapPasteWait && strings.TrimSpace(text) == strings.TrimSpace(final)
}

// yapLook is the dictation as a box draws it.
func (m *Model) yapLook() *liveText {
	s := m.yap.s
	if s.Phase == "done" {
		s.Guessing = ""
	}
	hint := "listening · let go to paste"
	switch {
	case s.Phase != "listening":
		hint = "pasting"
	case s.Locked:
		hint = "listening · tap fn to paste"
	}
	return &liveText{settled: s.Settled, guessing: s.Guessing, level: float64(s.Level) / 100, frame: m.yap.frame, hint: hint}
}

// liveText is a dictation a box shows at its cursor.
type liveText struct {
	settled, guessing string
	level             float64 // how loud, 0 to 1
	frame             int
	hint              string // what's going on, in the box's bottom edge
}

// The look's colours, in steps from each's resting colour to its lit one,
// made again when the terminal's colours change.
const yapSteps = 16

var yapInk struct {
	ground               theme.Ground
	settled, guess, edge [yapSteps + 1]string
	caret                [yapSteps + 1]string // grounds, for the cursor's cell
}

func yapColours() {
	if yapInk.ground == painted && yapInk.settled[0] != "" {
		return
	}
	g := painted
	c := func(r, gr, b uint8) theme.RGB { return theme.RGB{R: r, G: gr, B: b} }
	text, lit := g.Ink(c(226, 221, 211)), g.Ink(c(255, 250, 240))
	ground := g.Surface(c(40, 36, 32))
	orange, glint := g.Accent(c(217, 119, 87)), g.Ink(c(255, 232, 205))
	for i := 0; i <= yapSteps; i++ {
		t := float64(i) / yapSteps
		yapInk.settled[i] = theme.Mix(text, lit, t).FG()
		yapInk.guess[i] = theme.Mix(theme.Mix(text, ground, .5), lit, t).FG()
		yapInk.edge[i] = theme.Mix(orange, glint, t).FG()
		yapInk.caret[i] = theme.Mix(theme.Mix(orange, ground, .55), theme.Mix(orange, glint, .25), t).BG()
	}
	yapInk.ground = g
}

func yapStep(k float64) int { return int(min(1, max(0, k))*yapSteps + .5) }

// yapSweep is how lit the text at column x of row y is: a band of light
// leaning like yap's, crossing w columns every 1.6s, as wide as a third
// of them.
func yapSweep(frame, x, y, w int) float64 {
	const period = 32 // frames a crossing
	band := min(8, max(3, float64(w)/3))
	at := -band + float64(frame%period)/period*(float64(w)+2*band)
	d := float64(x) + 2*float64(y) - at
	if d < 0 {
		d = -d
	}
	if d >= band {
		return 0
	}
	k := 1 - d/band
	return k * k
}

// yapGlint is how lit a cell of the box's edge is, around cells
// clockwise from its top left out of n: a light with a fading tail,
// round the box every 2.4s.
func yapGlint(frame, around, n int) float64 {
	const lap, tail = 48, 28.0
	at := (frame % lap) * n / lap
	d := float64(((at-around)%n + n) % n)
	if d >= tail {
		return 0
	}
	k := 1 - d/tail
	return k * k
}

// yapDictated puts a dictation's paste in the box that has the keys, at
// its cursor, as typed: never folded into a chip, the prompt's newlines
// kept. False when no box takes it.
func (m *Model) yapDictated(text string) bool {
	if c := m.host; c != nil && m.paneFocus {
		pos := max(0, len(c.input)-c.back)
		c.undo.save(c.input, c.back, false)
		c.input = insert(c.input, pos, []rune(text))
		return true
	}
	if m.dialog == nil && m.acceptsText() && !m.sessionFocused() {
		m.input = insert(m.input, m.cursorPos(), []rune(text))
		return true
	}
	return false
}
