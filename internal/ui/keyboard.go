package ui

import (
	"math"
	"math/rand/v2"
	"runtime"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/keymap"
)

// kbKey is a key on the drawn keyboard: its name as keymap spells it, what
// its cap says, and how wide the cap is.
type kbKey struct {
	name, label string
	w           int
}

// kbRows are the keyboard's rows, each staggered as a real one's are.
var kbRows = func() [][]kbKey {
	plain := func(ks string) []kbKey {
		var out []kbKey
		for _, k := range strings.Split(ks, " ") {
			out = append(out, kbKey{k, k, 3})
		}
		return out
	}
	alt, super := "alt", "super"
	if runtime.GOOS == "darwin" {
		alt, super = "⌥", "⌘"
	}
	cat := func(parts ...[]kbKey) []kbKey {
		var out []kbKey
		for _, p := range parts {
			out = append(out, p...)
		}
		return out
	}
	return [][]kbKey{
		{{"esc", "esc", 5}},
		cat(plain("` 1 2 3 4 5 6 7 8 9 0 - ="), []kbKey{{"backspace", "⌫", 6}}),
		cat([]kbKey{{"tab", "tab", 5}}, plain("q w e r t y u i o p [ ]"), []kbKey{{`\`, `\`, 4}}),
		cat([]kbKey{{"", "", 6}}, plain("a s d f g h j k l ; '"), []kbKey{{"enter", "enter", 7}}),
		cat([]kbKey{{"shift", "shift", 8}}, plain("z x c v b n m , . /"), []kbKey{{"shift", "shift", 9}}),
		{{"ctrl", "ctrl", 5}, {"alt", alt, 3}, {"super", super, 5}, {"space", "space", 17}, {"super", super, 5}, {"alt", alt, 3}, {"left", "←", 3}, {"up", "↑", 3}, {"down", "↓", 3}, {"right", "→", 3}},
	}
}()

// kbWide is the keyboard's width in cells.
const kbWide = 58

// shifted is the key each shifted character is typed on.
var shifted = map[string]string{
	"~": "`", "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7", "*": "8", "(": "9", ")": "0",
	"_": "-", "+": "=", "{": "[", "}": "]", "|": `\`, ":": ";", `"`: "'", "<": ",", ">": ".", "?": "/",
}

// keyParts are the keyboard's keys a binding is pressed with, a chord's
// keys all together.
func keyParts(seq keymap.Seq) map[string]bool {
	out := map[string]bool{}
	for _, k := range seq {
		base, mods := k, ""
		if i := strings.LastIndex(k[:max(0, len(k)-1)], "+"); i >= 0 {
			base, mods = k[i+1:], k[:i]
		}
		for _, mod := range strings.Split(mods, "+") {
			out[mod] = mods != ""
		}
		switch {
		case shifted[base] != "":
			out["shift"], base = true, shifted[base]
		case len(base) == 1 && base != strings.ToLower(base):
			out["shift"], base = true, strings.ToLower(base)
		}
		out[base] = true
	}
	delete(out, "")
	return out
}

// kbFaces are the keycaps' faces, in the theme's colours: the key just
// pressed, the keys lit, those bound here, and the rest, dimmer than any
// cap in use.
func kbFaces() (hit, lit, used, free string) {
	return kbHitBG + qInk + bold, qCapOn + qInk + bold, qCap + cText, qCard + cFaint
}

// kbHitBG is the key just pressed's cap, set with the theme's colours.
var kbHitBG string

// kbCell is one cell of a drawn keycap: where it sits, what it says, and
// its face.
type kbCell struct {
	x, y int
	r    rune
	face string
}

// kbCells are the keyboard's cells, each key's face picked as keyboard
// draws it.
func kbCells(hit, lit, used map[string]bool) []kbCell {
	var out []kbCell
	kbHit, kbLit, kbUsed, kbFree := kbFaces()
	for y, row := range kbRows {
		x := 0
		for _, k := range row {
			face := kbFree
			switch {
			case hit[k.name]:
				face = kbHit
			case lit[k.name]:
				face = kbLit
			case used[k.name]:
				face = kbUsed
			}
			pad := k.w - cellw.String(k.label)
			for _, r := range strings.Repeat(" ", pad/2) + k.label + strings.Repeat(" ", pad-pad/2) {
				out = append(out, kbCell{x, y, r, face})
				x++
			}
			x++
		}
	}
	return out
}

// keyboard draws the keyboard: the key just pressed (hit) brightest, lit's
// keys lit, used's plainly and the rest faint, so a binding shows as where
// your fingers go.
func keyboard(hit, lit, used map[string]bool) []string {
	g := newKbGrid(kbWide, len(kbRows))
	for _, c := range kbCells(hit, lit, used) {
		g.set(c.x, c.y, c.r, c.face)
	}
	return g.lines()
}

// kbLegend says what the keyboard's faces mean.
func kbLegend() string {
	sw := func(face, what string) string { return face + "   " + reset + " " + dim(what) }
	_, kbLit, kbUsed, kbFree := kbFaces()
	return sw(kbLit, "this one's keys") + "   " + sw(kbUsed, "bound here") + "   " + sw(kbFree, "free") + faint("   · press a key to find it")
}

// kbGrid is a drawing in cells, each a rune in a face; a zero cell is blank.
type kbGrid struct {
	w, h  int
	r     []rune
	faces []string
}

func newKbGrid(w, h int) *kbGrid {
	return &kbGrid{w: w, h: h, r: make([]rune, w*h), faces: make([]string, w*h)}
}

func (g *kbGrid) set(x, y int, r rune, face string) {
	if x >= 0 && y >= 0 && x < g.w && y < g.h {
		g.r[y*g.w+x], g.faces[y*g.w+x] = r, face
	}
}

func (g *kbGrid) lines() []string {
	out := make([]string, g.h)
	for y := range g.h {
		var b strings.Builder
		for x := range g.w {
			i := y*g.w + x
			switch {
			case g.r[i] == 0:
				b.WriteByte(' ')
			default:
				b.WriteString(g.faces[i] + string(g.r[i]) + reset)
			}
		}
		out[y] = strings.TrimRight(b.String(), " ")
	}
	return out
}

// kbWords, typed on Keys, blow the keyboard up.
var kbWords = []string{"poppers", "alex"}

const (
	kbBoomLen = 3600 * time.Millisecond // the whole show, back together at the end
	kbFlyLen  = 1200 * time.Millisecond // the pieces flying off
	kbBackAt  = 2600 * time.Millisecond // when they start flying home
)

// kbBurst is the keyboard at t into its burst, w cells wide: its caps fly
// apart, fireworks go up where it stood, and the pieces fly home. seed
// makes each burst its own, and every frame of one the same.
func kbBurst(used map[string]bool, t time.Duration, seed uint64, w int) []string {
	h := len(kbRows)
	g := newKbGrid(w, h)
	rnd := rand.New(rand.NewPCG(seed, 7))
	sec := t.Seconds()

	// Fireworks: a rocket climbs, then a ring of sparks falls away.
	sparks := []rune{'✦', '*', '+', '·'}
	colours := []string{cOrange, cYellow, cBlue, cGreen, cRed}
	for i := range 6 {
		at := .2 + .38*float64(i) + .1*rnd.Float64()
		cx, cy := 4+rnd.Float64()*float64(w-8), 1+rnd.Float64()*2
		colour := colours[rnd.IntN(len(colours))]
		switch age := sec - at; {
		case age < 0:
		case age < .25:
			g.set(int(cx), h-1-int((float64(h-1)-cy)*age/.25), '╵', colour)
		case age < 1.15:
			a := age - .25
			for k := range 14 {
				ang := 2 * math.Pi * float64(k) / 14
				x, y := cx+math.Cos(ang)*a*22, cy+math.Sin(ang)*a*7+3*a*a
				g.set(int(math.Round(x)), int(math.Round(y)), sparks[min(3, int(a/.9*4))], colour+bold)
			}
		}
	}

	// The caps: each flies off from the middle, falls, then comes home.
	ox := float64(kbWide) / 2
	for _, c := range kbCells(nil, nil, used) {
		hx, hy := float64(c.x), float64(c.y)
		vx := (hx-ox)/ox*38 + (rnd.Float64()-.5)*24
		vy := (hy-2.5)*4 - 8 - rnd.Float64()*10
		fly := func(s float64) (float64, float64) { return hx + vx*s, hy + vy*s + 22*s*s }
		var x, y float64
		switch {
		case t < kbFlyLen:
			x, y = fly(sec)
		case t < kbBackAt:
			continue // off the screen
		default:
			k := min(1, (t-kbBackAt).Seconds()/(kbBoomLen-kbBackAt-200*time.Millisecond).Seconds())
			e := 1 - math.Pow(1-k, 3)
			fx, fy := fly(kbFlyLen.Seconds())
			x, y = fx+(hx-fx)*e, fy+(hy-fy)*e
		}
		g.set(int(math.Round(x)), int(math.Round(y)), c.r, c.face)
	}
	return g.lines()
}

// kbCheer is what the legend says while the keyboard bursts: the word, in
// colours going round.
func kbCheer(word string, t time.Duration) string {
	colours := []string{cOrange, cYellow, cBlue, cGreen, cRed}
	var b strings.Builder
	for i, r := range strings.ToUpper(word) + "!" {
		b.WriteString(paint(colours[(i+int(t/(120*time.Millisecond)))%len(colours)], bold+string(r)) + " ")
	}
	return b.String()
}
