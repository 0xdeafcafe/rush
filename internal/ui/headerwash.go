package ui

import (
	"math"
	"strings"
	"sync"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/photon/theme"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/charmbracelet/x/ansi"
)

// headerWash keeps the first 60% of the header unchanged, then grows sparse
// harness-colour blocks into a continuous provider tint at the right edge.
// The pattern is static; idle redraws only apply cached terminal colour codes.
// Tabs retain their own surfaces in the plain left portion of the header.
func headerWash(text string, width, row int, kind agent.Kind) string {
	if width <= 0 {
		return ""
	}
	colours := headerColours(width, row, kind)
	text = fit(text, width)
	// Truncating before a wide grapheme can leave a spare terminal cell.
	if missing := width - cellw.String(text); missing > 0 {
		text += blanks(missing)
	}
	var out strings.Builder
	out.Grow(len(text) + width*3 + 32)
	x, state, last := 0, byte(0), ""
	for len(text) > 0 {
		colour := colours[min(x, width-1)]
		if colour != last {
			out.WriteString(colour)
			last = colour
		}
		// Most of a header is ASCII; reserve grapheme decoding for escapes
		// and non-ASCII text, including wide and combining characters.
		if state == 0 && text[0] >= 0x20 && text[0] < 0x7f && (len(text) == 1 || text[1] < 0x80) {
			out.WriteByte(text[0])
			text = text[1:]
			x++
			continue
		}
		seq, cells, n, next := ansi.DecodeSequence(text, state, nil)
		if n == 0 {
			break
		}
		out.WriteString(seq)
		// A reset in paint() must not punch a hole in the wash. Foreground
		// styles and OSC hyperlinks pass through without being rewritten.
		if seq == reset || seq == "\x1b[m" || seq == "\x1b[49m" || seq == bgChrome {
			last = ""
		}
		x += cells
		state, text = next, text[n:]
	}
	out.WriteString(reset)
	return out.String()
}

type headerColourKey struct {
	ground            theme.Ground
	harness, provider theme.RGB
	width, row        int
}

type headerColourEntry struct {
	key     headerColourKey
	colours []string
}

// Fixed-size FIFO: switching sessions and resizing cannot retain an unbounded
// set of palettes. Entries are immutable after publication.
var headerColourCache struct {
	sync.Mutex
	entries [12]headerColourEntry
	next    int
}

func headerColours(width, row int, kind agent.Kind) []string {
	key := headerColourKey{painted, lookOf(agent.HarnessOf(kind)).rgb, lookOf(agent.Kind(agent.ProviderOf(kind))).rgb, width, (row % 4) / 2}
	headerColourCache.Lock()
	defer headerColourCache.Unlock()
	for _, e := range headerColourCache.entries {
		if e.key == key && e.colours != nil {
			return e.colours
		}
	}
	colours := makeHeaderColours(key)
	i := headerColourCache.next
	headerColourCache.entries[i] = headerColourEntry{key, colours}
	headerColourCache.next = (i + 1) % len(headerColourCache.entries)
	return colours
}

func headerColourAt(key headerColourKey, x float64) theme.RGB {
	base := theme.RGB{R: 30, G: 28, B: 26}
	ground := key.ground.Surface(base)
	column := int(math.Round(x * float64(max(0, key.width-1))))
	start := headerEdgeStart(key.width)
	if column < start {
		return ground
	}
	block := (column - start) / 4
	blocks := (key.width - start + 3) / 4
	fadeBlocks := min(8, max(4, blocks/2), max(1, blocks-1))
	u := min(1, float64(block)/float64(fadeBlocks))
	// Offset alternate two-row bands within the ramp, never at its end.
	if key.row%2 != 0 {
		u -= .10 * math.Sin(math.Pi*u)
	}
	envelope := headerSmooth(u)
	strength := .24 * envelope
	mix := envelope
	colour := key.ground.Surface(theme.Mix(base, theme.Mix(key.harness, key.provider, mix), strength))
	// Dim metadata remains at least as readable as normal chrome (up to
	// 3:1); stronger text keeps at least 4.5:1 on dark and light terminals.
	dimInk := key.ground.Ink(theme.RGB{R: 122, G: 117, B: 108})
	textInk := key.ground.Ink(theme.RGB{R: 168, G: 162, B: 152})
	dimFloor := min(3.0, theme.Contrast(dimInk, ground))
	textFloor := min(4.5, theme.Contrast(textInk, ground))
	// Move toward the terminal ground when more contrast is needed. That
	// keeps the hue instead of flattening dim terminal themes back to chrome.
	anchor := key.ground.BG
	if theme.Contrast(dimInk, anchor) < dimFloor || theme.Contrast(textInk, anchor) < textFloor {
		anchor = ground
	}
	for range 24 {
		if theme.Contrast(dimInk, colour) >= dimFloor && theme.Contrast(textInk, colour) >= textFloor {
			break
		}
		colour = theme.Mix(colour, anchor, .12)
	}
	if theme.Contrast(dimInk, colour) < dimFloor || theme.Contrast(textInk, colour) < textFloor {
		return ground
	}
	return colour
}

func headerSmooth(x float64) float64 {
	x = min(1, max(0, x))
	return x * x * (3 - 2*x)
}

// Integer arithmetic puts the hard boundary at ceil(width * 0.60).
func headerEdgeStart(width int) int { return (width*6 + 9) / 10 }

func makeHeaderColours(key headerColourKey) []string {
	width := key.width
	colours := make([]string, width)
	ground := key.ground.Surface(theme.RGB{R: 30, G: 28, B: 26})
	base := ground.BG()
	start := headerEdgeStart(width)
	for i := range colours {
		if i < start {
			colours[i] = base
			continue
		}
		// Every four terminal cells are one pixel. Share its code instead
		// of redoing contrast checks and string formatting per cell.
		if (i-start)%4 != 0 {
			colours[i] = colours[i-1]
			continue
		}
		colour := headerColourAt(key, float64(i)/float64(max(1, width-1)))
		if colour == ground {
			colours[i] = base
		} else {
			colours[i] = colour.BG()
		}
	}
	return colours
}
