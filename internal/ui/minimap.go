package ui

import (
	"hash/maphash"
	"sort"
	"strings"
	"time"
	"unicode"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/charmbracelet/x/ansi"
)

const miniColumns = 10
const miniWidth = miniColumns + 2

// miniSamples is how many rows a dots row is averaged from: past that, a
// longer conversation costs the map nothing more.
const miniSamples = 4

// miniEvery is how often what's drawn changing redraws the map; a change
// of size, or of rows by a dots row's worth, redraws it at once.
const miniEvery = 100 * time.Millisecond

// miniTurn is what the map keeps of a turn as last drawn: a few of its
// rows' miniatures, 3 bits a dot column, as many as the map can show of
// it, and enough to tell when it's drawn differently.
type miniTurn struct {
	n, sum int // its rows, and their bytes
	ends   uint64
	rows   []uint64
}

// miniWindow is what the map is drawn from: rows [base, base+len(lines))
// of total as drawn, the rest from what's kept of each turn, or a guess at
// one never drawn. transcript is the rows above the panels; ins the rows
// put in under shell steps in the window. s is nil for a body drawn whole.
type miniWindow struct {
	lines                   []convo.Line
	base, total, transcript int
	ins                     int
	s                       *convo.Session
}

type miniGroup struct {
	at int // its first row in the window
	t  *miniTurn
}

type conversationMap struct {
	turns                      map[string]*miniTurn // by turn ref
	groups                     []miniGroup          // this frame's window, a turn at a time
	panels                     []uint64             // the panel rows under the window's end
	normal, active             []string
	palette                    [7]string
	width, height, scale       int
	total                      int       // the rows it was laid out over
	laidAt                     time.Time // and when
	pending                    bool      // what's drawn changed since
	pixels                     [][miniColumns * 2]uint8
	x, y, viewport, start, end int
	visible, dragging          bool
	dragY, dragStart           int
	parsed                     uint64 // rows decoded, also useful for performance regression checks
}

var miniSeed = maphash.MakeSeed()

func miniPalette() [7]string {
	return [7]string{"", cSub, cBlue, cOrange, cGreen, cRed, cQueue}
}

// prepare draws the map of a body drawn whole.
func (m *conversationMap) prepare(lines []convo.Line, width, height int) {
	m.prepareWindow(miniWindow{lines: lines, total: len(lines), transcript: len(lines)}, width, height)
}

// prepareWindow decodes only the turns drawn differently since, a few rows
// each, and draws the map again when its size or scale moved, or now and
// then when what's drawn changed: wheel and drag frames skip it entirely.
func (m *conversationMap) prepareWindow(win miniWindow, width, height int) {
	palette := miniPalette()
	now := m.width != width || m.palette != palette || m.height != height || m.normal == nil ||
		abs(win.total-m.total) >= max(1, m.scale/(height*4)) ||
		time.Since(m.laidAt) >= time.Minute // its edge shades by age
	if m.width != width || m.palette != palette || m.turns == nil {
		m.turns = map[string]*miniTurn{}
	}
	m.palette, m.width, m.height = palette, width, height
	scale := max(win.total, height*4)
	dirty := false
	lines := win.lines
	tr := min(len(lines), max(0, win.transcript-win.base))
	m.groups = m.groups[:0]
	for a := 0; a < tr; {
		key := turnKey(lines[a].Ref)
		b, sum := a+1, len(lines[a].Text)
		for ; b < tr && (lines[b].Ref == "" || turnKey(lines[b].Ref) == key); b++ {
			sum += len(lines[b].Text)
		}
		n, ends := b-a, maphash.String(miniSeed, lines[a].Text)^maphash.String(miniSeed, lines[b-1].Text)<<1
		g := m.turns[key]
		if g == nil || g.n != n || g.sum != sum || g.ends != ends {
			if g == nil {
				g = &miniTurn{}
				m.turns[key] = g
			}
			k := min(n, 64, max(4, 2*n*height*4/scale+1)) // as many as it takes dots rows, twice
			g.n, g.sum, g.ends, g.rows = n, sum, ends, g.rows[:0]
			for j := range k {
				g.rows = append(g.rows, packMini(miniPixels(lines[a+j*n/k], width, palette)))
				m.parsed++
			}
			dirty = true
		}
		m.groups = append(m.groups, miniGroup{a, g})
		a = b
	}
	panels := lines[tr:]
	if len(panels) != len(m.panels) {
		m.panels, dirty = make([]uint64, len(panels)), true
	}
	for i, l := range panels {
		if px := packMini(miniPixels(l, width, palette)); px != m.panels[i] {
			m.panels[i], dirty = px, true
		}
	}
	m.pending = m.pending || dirty
	if now || m.pending && time.Since(m.laidAt) >= miniEvery {
		m.total, m.scale, m.laidAt, m.pending = win.total, scale, time.Now(), false
		m.layout(win)
	}
}

// turnKey is the turn a row's ref is of: "t13" for "t13:s:…".
func turnKey(ref string) string {
	k, _, _ := strings.Cut(ref, ":")
	return k
}

func packMini(px [miniColumns * 2]uint8) (v uint64) {
	for x, c := range px {
		v |= uint64(c&7) << (3 * x)
	}
	return v
}

// rowAt is row r's miniature: as drawn in the window, else as kept of its
// turn, else a guess at a turn's shape.
func (m *conversationMap) rowAt(win miniWindow, r int) uint64 {
	if i := r - win.base; i >= 0 && i < len(win.lines) {
		if tr := win.transcript - win.base; i >= tr {
			return m.panels[i-tr]
		}
		j := sort.Search(len(m.groups), func(j int) bool { return m.groups[j].at > i }) - 1
		if j < 0 {
			return 0
		}
		g := m.groups[j]
		return g.t.rows[min(len(g.t.rows)-1, (i-g.at)*len(g.t.rows)/max(1, g.t.n))]
	}
	s := win.s
	if s == nil || r >= win.transcript {
		return 0
	}
	if r >= win.base+len(win.lines) {
		r -= win.ins // the session counts no rows under steps
	}
	k := s.TurnAt(r)
	off := r - s.TurnRow(k)
	if g := m.turns[s.TurnRef(k)]; g != nil && len(g.rows) > 0 {
		return g.rows[min(len(g.rows)-1, max(0, off)*len(g.rows)/max(1, g.n))]
	}
	return miniGuess(k, off)
}

// miniGuess is a turn never drawn: its heading, then prose.
func miniGuess(k, off int) (v uint64) {
	from, to, colour := 1, 7, uint64(2)
	if off > 0 {
		h := uint32(k)*2654435761 ^ uint32(off)*40503
		from, to, colour = 2, 8+int(h%11), 1
	}
	for x := from; x < to; x++ {
		v |= colour << (3 * x)
	}
	return v
}

// layout draws every dots row again, each averaged from a few of its rows.
func (m *conversationMap) layout(win miniWindow) {
	height, palette := m.height, m.palette
	h4 := height * 4
	if len(m.pixels) != h4 {
		m.pixels = make([][miniColumns * 2]uint8, h4)
		m.normal, m.active = make([]string, height), make([]string, height)
	}
	// Average coverage before choosing dots, rather than OR-ing every line
	// into a solid block. Ordered dithering retains whitespace at any scale.
	threshold := [4][2]uint32{{1, 5}, {7, 3}, {4, 8}, {6, 2}}
	for y := range h4 {
		var counts [miniColumns * 2][8]uint32
		lo, hi := y*m.scale/h4, min(win.total, (y+1)*m.scale/h4)
		total := uint32(0)
		for r := lo; r < hi; r += max(1, (hi-lo)/miniSamples) {
			px := m.rowAt(win, r)
			total++
			for x := range miniColumns * 2 {
				counts[x][px>>(3*x)&7]++
			}
		}
		m.pixels[y] = [miniColumns * 2]uint8{}
		for x := range m.pixels[y] {
			n := total - counts[x][0]
			if n == 0 || n*8 < total*threshold[y%4][x%2] {
				continue
			}
			colour := uint8(1)
			for c := uint8(2); c < 7; c++ {
				if counts[x][c] > counts[x][colour] {
					colour = c
				}
			}
			m.pixels[y][x] = colour
		}
	}
	now := time.Now()
	dots := [2][4]rune{{1, 2, 4, 64}, {8, 16, 32, 128}}
	// Orientation texture, quieter than the conversation. Cache these inks
	// with the raster so scrolling adds no colour conversion work.
	var ink [7]string
	for i := 1; i < len(palette); i++ {
		ink[i] = fadeCode(fadeCode(palette[i]))
	}
	for y := range height {
		var b strings.Builder
		for x := range miniColumns {
			bits, colour := rune(0), uint8(0)
			for dy := range 4 {
				for dx := range 2 {
					c := m.pixels[y*4+dy][x*2+dx]
					if c != 0 {
						bits |= dots[dx][dy]
						colour = max(colour, c)
					}
				}
			}
			if bits == 0 {
				b.WriteByte(' ')
			} else {
				b.WriteString(ink[colour])
				b.WriteRune(0x2800 + bits)
				b.WriteString(reset)
			}
		}
		text := b.String()
		m.normal[y] = " " + paint(miniAge(win, min(win.total-1, (y*4+2)*m.scale/h4), now), "│") + onBg(painted.BG.BG(), text, miniColumns)
		m.active[y] = " " + paint(fadeCode(cBlue), "▌") + onBg(bgChrome, text, miniColumns)
	}
}

// miniAge is the map's edge beside row r: bright where the turn there
// ended in the last few minutes, fading through the hour and the day.
func miniAge(win miniWindow, r int, now time.Time) string {
	s := win.s
	if s == nil || len(s.Turns) == 0 || r < 0 {
		return cFaint
	}
	if r >= win.base+len(win.lines) {
		r -= win.ins // the session counts no rows under steps
	}
	t := s.Turns[min(len(s.Turns)-1, max(0, s.TurnAt(r)))]
	at := t.End
	if at.IsZero() {
		at = t.Start // still going
	}
	switch ago := now.Sub(at); {
	case at.IsZero():
		return cFaint
	case ago < 5*time.Minute:
		return cText
	case ago < time.Hour:
		return cSub
	case ago < 24*time.Hour:
		return cDim
	}
	return cFaint
}

func miniPixels(line convo.Line, width int, palette [7]string) (pixels [miniColumns * 2]uint8) {
	base := uint8(1)
	if isTurnRef(line.Ref) {
		base = 2
	} else if strings.Contains(line.Ref, ":s:") || strings.Contains(line.Ref, ":run:") {
		base = 3
	}
	colour, x, state := base, 0, byte(0)
	text := line.Text
	for len(text) > 0 && x < width {
		// ASCII prose dominates transcripts; decode graphemes only for escapes
		// and non-ASCII, including an ASCII base followed by a combining mark.
		if state == 0 && text[0] >= 0x20 && text[0] < 0x7f && (len(text) == 1 || text[1] < 0x80) {
			if text[0] != ' ' {
				pixels[min(miniColumns*2-1, x*miniColumns*2/max(1, width))] = colour
			}
			x++
			text = text[1:]
			continue
		}
		seq, cells, n, next := ansi.DecodeSequence(text, state, nil)
		if n == 0 {
			break
		}
		if cells > 0 {
			ink := false
			for _, r := range seq {
				if !unicode.IsSpace(r) && !strings.ContainsRune("│─┆┊", r) {
					ink = true
					break
				}
			}
			if ink {
				for col := x; col < min(x+cells, width); col++ {
					pixels[min(miniColumns*2-1, col*miniColumns*2/max(1, width))] = colour
				}
			}
			x += cells
		} else if seq == reset || seq == "\x1b[m" {
			colour = base
		} else {
			for i := 2; i < len(palette); i++ {
				if seq == palette[i] {
					colour = uint8(i)
					break
				}
			}
		}
		text, state = text[n:], next
	}
	return
}

func (m *conversationMap) bounds() (int, int) {
	return m.start * m.height / max(1, m.scale), min(m.height, max(1, (m.end*m.height+m.scale-1)/max(1, m.scale)))
}

func (m *conversationMap) line(y int) string {
	first, last := m.bounds()
	if y >= first && y < last {
		return m.active[y]
	}
	return m.normal[y]
}

func (m *conversationMap) hit(x, y int) bool {
	return m.visible && x >= m.x && x < m.x+miniWidth && y >= m.y && y < m.y+m.height
}

func (m *Model) pressMinimap(c *hostConn, x, y int) bool {
	mm := &c.minimap
	if !mm.hit(x, y) {
		return false
	}
	first, last := mm.bounds()
	local := y - mm.y
	if local < first || local >= last {
		switch local {
		case 0:
			m.seekMinimap(c, 0)
		case mm.height - 1:
			m.seekMinimap(c, c.shownTotal)
		default:
			m.seekMinimap(c, local*mm.scale/max(1, mm.height)-mm.viewport/2)
		}
	}
	mm.dragging, mm.dragY, mm.dragStart = true, y, mm.start
	m.paneFocus = true
	c.txt = textSel{}
	return true
}

func (m *Model) dragMinimap(c *hostConn, y int) {
	mm := &c.minimap
	if y == mm.dragY {
		m.seekMinimap(c, mm.dragStart)
		return
	}
	switch {
	case y <= mm.y:
		m.seekMinimap(c, 0)
	case y >= mm.y+mm.height-1:
		m.seekMinimap(c, c.shownTotal)
	default:
		m.seekMinimap(c, mm.dragStart+(y-mm.dragY)*mm.scale/max(1, mm.height))
	}
}

func (m *Model) seekMinimap(c *hostConn, start int) {
	mm := &c.minimap
	total := c.shownTotal
	if start >= total-mm.viewport {
		c.scroll = 0
		mm.start = max(0, total-mm.viewport)
	} else if start <= 0 {
		mm.start, c.scroll = 0, total // as Home: the frame clamps it to the top once the rows above are counted
	} else {
		mm.start = start
		c.scroll = max(0, total-mm.start-(mm.viewport-1))
	}
	c.scrollOnly, c.selMoved, c.pinTop = true, false, false
}

func (m *Model) minimapEnabled(c *hostConn, w int) bool {
	return m.conversationWidth(w) != w && m.viewName(c) == "conversation"
}

// Shared with warming and relayout: all three must agree on wrapping width.
func (m *Model) conversationWidth(w int) int {
	if (m.store == nil || !m.store.Config.HideMinimap) && w >= 96 && !m.zen {
		return w - miniWidth
	}
	return w
}
