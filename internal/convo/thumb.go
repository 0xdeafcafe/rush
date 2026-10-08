package convo

import (
	"bytes"
	"image"
	"image/color"
	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"

	_ "golang.org/x/image/webp"

	"github.com/0xdeafcafe/photon/theme"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// An image a step read or a tool gave back is drawn under it, small, in
// half blocks: each cell's ▀ takes the upper pixel as its colour and the
// lower as its ground. Colours are 24-bit, as the rest of the palette is;
// the renderer brings them down for a terminal with fewer. A thumbnail is
// made off the UI goroutine: the first frame to want one starts it and
// draws the image's chip, and a frame after it's made has it.

// thumbW and thumbH are a thumbnail's most pixels: 32 cells by 12 rows.
const thumbW, thumbH = 32, 24

// thumbKey is a step's image, by its call and its place among them.
type thumbKey struct {
	id string
	n  int
}

type thumbnail struct {
	done bool
	rows []string // nil when it couldn't be read
}

var (
	thumbs = struct {
		sync.Mutex
		m map[thumbKey]*thumbnail
	}{m: map[thumbKey]*thumbnail{}}
	// Only a couple at once: a transcript full of screenshots asks for
	// them all when it's opened with everything shown.
	thumbSlots = make(chan struct{}, 2)
)

// thumbOf is image img of a step drawn as rows of half blocks, and whether
// they're ready. It never decodes while a frame is drawn.
func thumbOf(k thumbKey, img *event.ImageData) ([]string, bool) {
	thumbs.Lock()
	defer thumbs.Unlock()
	t := thumbs.m[k]
	if t == nil {
		if len(thumbs.m) >= 1024 {
			thumbs.m = map[thumbKey]*thumbnail{} // made again as they're drawn
		}
		t = &thumbnail{}
		thumbs.m[k] = t
		go func() {
			thumbSlots <- struct{}{}
			rows := halfBlocks(readThumb(img, thumbW, thumbH))
			<-thumbSlots
			thumbs.Lock()
			t.rows, t.done = rows, true
			thumbs.Unlock()
			lookupsGen.Add(1)
		}()
	}
	if !t.done {
		lookupWaits.Add(1)
	}
	return t.rows, t.done && t.rows != nil
}

// thumbable is whether a file can be drawn as a thumbnail, by its name.
func thumbable(path string) bool {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".png", ".jpg", ".jpeg", ".gif", ".webp":
		return true
	}
	return false
}

// Preview is the image at path drawn to fit w cells by h rows, nil when
// it can't be read: in half blocks, or when fine in quadrants, twice as
// many pixels across. It decodes, so it's for off the UI goroutine.
func Preview(path string, w, h int, fine bool) []string {
	return PreviewImage(&event.ImageData{Path: path}, w, h, fine)
}

// PreviewImage draws a file or embedded image off the UI goroutine.
func PreviewImage(img *event.ImageData, w, h int, fine bool) []string {
	if fine {
		return quadrants(readThumb(img, 2*w, 4*h))
	}
	return halfBlocks(readThumb(img, w, 2*h))
}

// quadGlyphs are the quadrant blocks by which of a cell's four take the
// foreground: 1 top left, 2 top right, 4 bottom left, 8 bottom right.
var quadGlyphs = []rune(" ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█")

// flatDist is how far apart, squared across red, green and blue, two
// colours can be and still be drawn as one: about 20 a channel.
const flatDist = 3 * 20 * 20

// quadrants draws px two pixels across and four down to a cell, the four
// down taken in pairs so each quarter of the cell is square. A cell has
// two colours, so its quarters are split between the two furthest apart.
// ponytail: transparency shows as black; composite over the ground if
// transparent images turn up here.
func quadrants(px *image.NRGBA) []string {
	if px == nil {
		return nil
	}
	b := px.Bounds()
	at := func(x, y int) [3]int {
		if x >= b.Max.X || y >= b.Max.Y {
			return [3]int{}
		}
		c := px.NRGBAAt(x, y)
		return [3]int{int(c.R), int(c.G), int(c.B)}
	}
	var rows []string
	for y := b.Min.Y; y < b.Max.Y; y += 4 {
		var s strings.Builder
		for x := b.Min.X; x < b.Max.X; x += 2 {
			var q [4][3]int
			for i := range 4 {
				top, bot := at(x+i%2, y+i/2*2), at(x+i%2, y+i/2*2+1)
				for k := range 3 {
					q[i][k] = (top[k] + bot[k]) / 2
				}
			}
			s.WriteString(quadCell(q))
		}
		s.WriteString(reset)
		rows = append(rows, s.String())
	}
	return rows
}

// quadCell draws one cell's four quarters in its two colours.
func quadCell(q [4][3]int) string {
	dist := func(a, c [3]int) int {
		return (a[0]-c[0])*(a[0]-c[0]) + (a[1]-c[1])*(a[1]-c[1]) + (a[2]-c[2])*(a[2]-c[2])
	}
	rgb := func(c [3]int, n int) theme.RGB {
		n = max(n, 1)
		return theme.RGB{R: uint8(c[0] / n), G: uint8(c[1] / n), B: uint8(c[2] / n)}
	}
	// The two quarters furthest apart seed the two colours.
	sa, sb, far := 0, 0, -1
	for i := range 4 {
		for j := i + 1; j < 4; j++ {
			if d := dist(q[i], q[j]); d > far {
				sa, sb, far = i, j, d
			}
		}
	}
	// A cell near enough one colour is drawn as ground alone: a glyph's
	// edge leaves a seam, and a page's flat white would be lined with them.
	if far < flatDist {
		var sum [3]int
		for i := range 4 {
			sum[0], sum[1], sum[2] = sum[0]+q[i][0], sum[1]+q[i][1], sum[2]+q[i][2]
		}
		return rgb(sum, 4).BG() + " "
	}
	mask, fg, bg, nf, nb := 0, [3]int{}, [3]int{}, 0, 0
	for i := range 4 {
		if dist(q[i], q[sa]) <= dist(q[i], q[sb]) {
			mask |= 1 << i
			fg[0], fg[1], fg[2], nf = fg[0]+q[i][0], fg[1]+q[i][1], fg[2]+q[i][2], nf+1
		} else {
			bg[0], bg[1], bg[2], nb = bg[0]+q[i][0], bg[1]+q[i][1], bg[2]+q[i][2], nb+1
		}
	}
	return rgb(fg, nf).FG() + rgb(bg, nb).BG() + string(quadGlyphs[mask])
}

// readThumb decodes an image, from its bytes or else its file, and
// shrinks it to fit w by h pixels; nil when it can't.
func readThumb(img *event.ImageData, w, h int) *image.NRGBA {
	var r io.ReadSeeker
	switch {
	case len(img.Data) > 0:
		r = bytes.NewReader(img.Data)
	case img.Path != "":
		f, err := os.Open(img.Path)
		if err != nil {
			return nil
		}
		defer f.Close()
		r = f
	default:
		return nil
	}
	// Nothing huge is decoded to draw a terminal's worth of it.
	cfg, _, err := image.DecodeConfig(r)
	if err != nil || cfg.Width*cfg.Height > 64<<20 {
		return nil
	}
	if _, err := r.Seek(0, io.SeekStart); err != nil {
		return nil
	}
	src, _, err := image.Decode(r)
	if err != nil {
		return nil
	}
	return shrinkTo(src, w, h)
}

// shrink scales src to fit a thumbnail.
func shrink(src image.Image) *image.NRGBA { return shrinkTo(src, thumbW, thumbH) }

// shrinkTo scales src to fit maxW by maxH, keeping its shape, each pixel
// the average of those it covers.
func shrinkTo(src image.Image, maxW, maxH int) *image.NRGBA {
	b := src.Bounds()
	W, H := b.Dx(), b.Dy()
	if W <= 0 || H <= 0 {
		return nil
	}
	w := min(W, maxW)
	h := max(1, (H*w+W/2)/W)
	if h > maxH {
		h = maxH
		w = max(1, (W*h+H/2)/H)
	}
	out := image.NewNRGBA(image.Rect(0, 0, w, h))
	for y := range h {
		y0, y1 := b.Min.Y+y*H/h, b.Min.Y+(y+1)*H/h
		for x := range w {
			x0, x1 := b.Min.X+x*W/w, b.Min.X+(x+1)*W/w
			var r, g, bl, a, n uint64
			for sy := y0; sy < max(y1, y0+1); sy++ {
				for sx := x0; sx < max(x1, x0+1); sx++ {
					cr, cg, cb, ca := src.At(sx, sy).RGBA()
					r, g, bl, a, n = r+uint64(cr), g+uint64(cg), bl+uint64(cb), a+uint64(ca), n+1
				}
			}
			if a == 0 {
				continue
			}
			// The sums are premultiplied: dividing by alpha's undoes it.
			out.SetNRGBA(x, y, color.NRGBA{R: uint8(r * 255 / a), G: uint8(g * 255 / a), B: uint8(bl * 255 / a), A: uint8(a / n / 257)})
		}
	}
	return out
}

// halfBlocks draws px two rows of pixels to a line. A pixel less than
// half opaque shows the terminal's ground.
func halfBlocks(px *image.NRGBA) []string {
	if px == nil {
		return nil
	}
	b := px.Bounds()
	rgb := func(c color.NRGBA) theme.RGB { return theme.RGB{R: c.R, G: c.G, B: c.B} }
	var rows []string
	for y := b.Min.Y; y < b.Max.Y; y += 2 {
		var s strings.Builder
		for x := b.Min.X; x < b.Max.X; x++ {
			top, bot := px.NRGBAAt(x, y), color.NRGBA{}
			if y+1 < b.Max.Y {
				bot = px.NRGBAAt(x, y+1)
			}
			switch up, down := top.A >= 128, bot.A >= 128; {
			case up && down && sameish(top, bot):
				s.WriteString(rgb(top).BG() + " ") // no glyph, so no seam
			case up && down:
				s.WriteString(rgb(top).FG() + rgb(bot).BG() + "▀")
			case up:
				s.WriteString(rgb(top).FG() + "\x1b[49m▀")
			case down:
				s.WriteString(rgb(bot).FG() + "\x1b[49m▄")
			default:
				s.WriteString("\x1b[49m ")
			}
		}
		s.WriteString(reset)
		rows = append(rows, s.String())
	}
	return rows
}

// sameish is whether two colours are near enough to draw as one.
func sameish(a, b color.NRGBA) bool {
	d := func(x, y uint8) int { return (int(x) - int(y)) * (int(x) - int(y)) }
	return d(a.R, b.R)+d(a.G, b.G)+d(a.B, b.B) < flatDist
}
