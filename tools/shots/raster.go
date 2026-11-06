package main

import (
	"fmt"
	"image"
	"image/color"
	"image/draw"
	"math"
	"os"
	"strings"

	uv "github.com/charmbracelet/ultraviolet"
	"github.com/charmbracelet/x/ansi"
	"golang.org/x/image/font"
	"golang.org/x/image/font/opentype"
	"golang.org/x/image/font/sfnt"
	"golang.org/x/image/math/fixed"
)

// The terminal's own colours, as the earlier screenshots had them: a dark
// ground rush is told of, as a terminal would answer it.
var (
	termBG = color.RGBA{17, 17, 17, 255}
	termFG = color.RGBA{240, 236, 230, 255}
)

// fonts are the faces cells are drawn in: the monospace one first, then
// others for what it has no glyph for.
type fonts struct {
	regular, bold []font.Face
	fallback      []*sfnt.Font
	cellW, cellH  int
	ascent        int
	size          float64
}

// loadFonts reads a system monospace font at size pixels, and fallbacks.
func loadFonts(size float64) (*fonts, error) {
	f := &fonts{size: size}
	mono, err := collection("/System/Library/Fonts/Menlo.ttc")
	if err != nil {
		return nil, err
	}
	face := func(fn *sfnt.Font) font.Face {
		fc, err := opentype.NewFace(fn, &opentype.FaceOptions{Size: size, DPI: 72, Hinting: font.HintingNone})
		if err != nil {
			panic(err)
		}
		return fc
	}
	f.regular = append(f.regular, face(mono[0]))
	f.bold = append(f.bold, face(mono[min(1, len(mono)-1)]))
	for _, p := range []string{
		"/System/Library/Fonts/SFNSMono.ttf",
		"/System/Library/Fonts/Apple Symbols.ttf",
		"/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
	} {
		fs, err := collection(p)
		if err != nil {
			continue
		}
		f.fallback = append(f.fallback, fs[0])
		f.regular = append(f.regular, face(fs[0]))
		f.bold = append(f.bold, face(fs[0]))
	}
	m := f.regular[0].Metrics()
	adv, _ := f.regular[0].GlyphAdvance('M')
	f.cellW = int(math.Round(float64(adv) / 64))
	f.cellH = int(math.Round(size * 1.3))
	asc, desc := float64(m.Ascent)/64, float64(m.Descent)/64
	f.ascent = int(math.Round((float64(f.cellH)-(asc+desc))/2 + asc))
	return f, nil
}

func collection(path string) ([]*sfnt.Font, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	c, err := sfnt.ParseCollection(b)
	if err != nil {
		return nil, err
	}
	out := make([]*sfnt.Font, c.NumFonts())
	for i := range out {
		if out[i], err = c.Font(i); err != nil {
			return nil, err
		}
	}
	return out, nil
}

// faceFor is the first face with a glyph for r.
func (f *fonts) faceFor(r rune, bold bool) font.Face {
	faces := f.regular
	if bold {
		faces = f.bold
	}
	for _, fc := range faces {
		if _, ok := fc.GlyphAdvance(r); ok {
			return fc
		}
	}
	return faces[0]
}

// screen parses a frame into cells, as a terminal would.
func screen(frame string, w, h int) uv.ScreenBuffer {
	buf := uv.NewScreenBuffer(w, h)
	buf.Method = ansi.GraphemeWidth
	uv.NewStyledString(frame).Draw(buf, uv.Rect(0, 0, w, h))
	return buf
}

// paint draws the cells into an image.
func (f *fonts) paint(buf uv.ScreenBuffer, w, h int) *image.RGBA {
	img := image.NewRGBA(image.Rect(0, 0, w*f.cellW, h*f.cellH))
	draw.Draw(img, img.Bounds(), image.NewUniform(termBG), image.Point{}, draw.Src)
	for y := range h {
		for x := 0; x < w; x++ {
			c := buf.CellAt(x, y)
			if c == nil {
				continue
			}
			fg, bg := colorOr(c.Style.Fg, termFG), colorOr(c.Style.Bg, termBG)
			if c.Style.Attrs&uv.AttrReverse != 0 {
				fg, bg = bg, fg
			}
			if c.Style.Attrs&uv.AttrFaint != 0 {
				fg = mix(fg, bg, 0.45)
			}
			cw := max(1, c.Width)
			r := image.Rect(x*f.cellW, y*f.cellH, (x+cw)*f.cellW, (y+1)*f.cellH)
			draw.Draw(img, r, image.NewUniform(bg), image.Point{}, draw.Src)
			if c.Content != "" && c.Content != " " {
				f.glyph(img, r, c.Content, fg, c.Style.Attrs&uv.AttrBold != 0)
			}
			if c.Style.Underline != 0 {
				ly := r.Min.Y + f.ascent + max(1, f.cellH/12)
				draw.Draw(img, image.Rect(r.Min.X, ly, r.Max.X, ly+max(1, f.cellH/20)), image.NewUniform(fg), image.Point{}, draw.Src)
			}
			if c.Style.Attrs&uv.AttrStrikethrough != 0 {
				ly := r.Min.Y + f.cellH/2
				draw.Draw(img, image.Rect(r.Min.X, ly, r.Max.X, ly+max(1, f.cellH/20)), image.NewUniform(fg), image.Point{}, draw.Src)
			}
			x += cw - 1
		}
	}
	return img
}

func (f *fonts) glyph(img *image.RGBA, r image.Rectangle, s string, fg color.RGBA, bold bool) {
	rs := []rune(s)
	if len(rs) == 1 && f.drawn(img, r, rs[0], fg) {
		return
	}
	d := font.Drawer{Dst: img, Src: image.NewUniform(fg), Face: f.faceFor(rs[0], bold),
		Dot: fixed.P(r.Min.X, r.Min.Y+f.ascent)}
	d.DrawString(s)
}

// drawn draws box-drawing and block characters by hand, as terminals do,
// so lines meet from one cell to the next.
func (f *fonts) drawn(img *image.RGBA, r image.Rectangle, ch rune, fg color.RGBA) bool {
	W, H := r.Dx(), r.Dy()
	t := max(1, int(math.Round(f.size/13)))
	cx, cy := r.Min.X+W/2-t/2, r.Min.Y+H/2-t/2
	fill := func(x0, y0, x1, y1 int) {
		draw.Draw(img, image.Rect(x0, y0, x1, y1), image.NewUniform(fg), image.Point{}, draw.Over)
	}
	left := func(th int) { fill(r.Min.X, cy, cx+th, cy+th) }
	right := func(th int) { fill(cx, cy, r.Max.X, cy+th) }
	up := func(th int) { fill(cx, r.Min.Y, cx+th, cy+th) }
	down := func(th int) { fill(cx, cy, cx+th, r.Max.Y) }
	switch ch {
	case '─':
		left(t)
		right(t)
	case '━':
		cy -= t / 2
		left(2 * t)
		right(2 * t)
	case '│':
		up(t)
		down(t)
	case '┃':
		cx -= t / 2
		up(2 * t)
		down(2 * t)
	case '┌':
		right(t)
		down(t)
	case '┐':
		left(t)
		down(t)
	case '└':
		right(t)
		up(t)
	case '┘':
		left(t)
		up(t)
	case '├':
		up(t)
		down(t)
		right(t)
	case '┤':
		up(t)
		down(t)
		left(t)
	case '┬':
		left(t)
		right(t)
		down(t)
	case '┴':
		left(t)
		right(t)
		up(t)
	case '┼':
		left(t)
		right(t)
		up(t)
		down(t)
	case '╭', '╮', '╰', '╯':
		f.arc(img, r, ch, t, fg)
	case '█':
		fill(r.Min.X, r.Min.Y, r.Max.X, r.Max.Y)
	case '▀':
		fill(r.Min.X, r.Min.Y, r.Max.X, r.Min.Y+H/2)
	case '▄':
		fill(r.Min.X, r.Min.Y+H/2, r.Max.X, r.Max.Y)
	case '▌':
		fill(r.Min.X, r.Min.Y, r.Min.X+W/2, r.Max.Y)
	case '▐':
		fill(r.Min.X+W/2, r.Min.Y, r.Max.X, r.Max.Y)
	case '▁', '▂', '▃', '▅', '▆', '▇':
		n := int(ch-'▁') + 1
		fill(r.Min.X, r.Max.Y-H*n/8, r.Max.X, r.Max.Y)
	case '▏', '▎', '▍', '▋', '▊', '▉':
		n := 8 - int(ch-'▉')
		fill(r.Min.X, r.Min.Y, r.Min.X+W*n/8, r.Max.Y)
	default:
		return false
	}
	return true
}

// arc is a rounded corner: a quarter circle from the cell's side, then
// straight on to its top or bottom.
func (f *fonts) arc(img *image.RGBA, r image.Rectangle, ch rune, t int, fg color.RGBA) {
	W := float64(r.Dx())
	cx, cy := float64(r.Min.X)+W/2, float64(r.Min.Y+r.Dy()/2)
	rad := W / 2
	ox, oy := cx+rad, cy+rad // ╭
	switch ch {
	case '╮':
		ox = cx - rad
	case '╰':
		oy = cy - rad
	case '╯':
		ox, oy = cx-rad, cy-rad
	}
	down := ch == '╭' || ch == '╮'
	half := float64(t) / 2
	for y := r.Min.Y; y < r.Max.Y; y++ {
		for x := r.Min.X; x < r.Max.X; x++ {
			px, py := float64(x)+0.5, float64(y)+0.5
			d := math.Abs(math.Hypot(px-ox, py-oy) - rad)
			if down && py > oy || !down && py < oy {
				d = math.Abs(px - cx)
			}
			if a := cover(d - half + 0.5); a > 0 {
				blend(img, x, y, fg, a)
			}
		}
	}
}

// cover is how much of a pixel a line d from its edge covers.
func cover(d float64) float64 { return math.Max(0, math.Min(1, 1-d)) }

func blend(img *image.RGBA, x, y int, c color.RGBA, a float64) {
	o := img.RGBAAt(x, y)
	img.SetRGBA(x, y, mix(o, c, a))
}

func mix(a, b color.RGBA, t float64) color.RGBA {
	l := func(x, y uint8) uint8 { return uint8(math.Round(float64(x) + (float64(y)-float64(x))*t)) }
	return color.RGBA{l(a.R, b.R), l(a.G, b.G), l(a.B, b.B), 255}
}

func colorOr(c color.Color, def color.RGBA) color.RGBA {
	if c == nil {
		return def
	}
	r, g, b, _ := c.RGBA()
	return color.RGBA{uint8(r >> 8), uint8(g >> 8), uint8(b >> 8), 255}
}

// window puts the terminal's image in a macOS window, as a window capture
// has it: a dark title bar with its three lights, rounded corners, a thin
// lighter edge, and the large soft shadow on a clear ground.
func window(term *image.RGBA, title string, f *fonts, scale float64) *image.RGBA {
	px := func(v float64) int { return int(math.Round(v * scale)) }
	pad, bar := px(10), px(28)
	side, top, bottom := px(64), px(40), px(88)
	radius := 11 * scale
	tw, th := term.Bounds().Dx(), term.Bounds().Dy()
	ww, wh := tw+2*pad, th+bar+pad
	img := image.NewRGBA(image.Rect(0, 0, ww+2*side, wh+top+bottom))
	x0, y0 := float64(side), float64(top)
	// The shadow: the window's shape blurred, 20 points below it.
	sigma, drop, dark := 22*scale, 20*scale, 0.48
	for y := 0; y < img.Bounds().Dy(); y++ {
		for x := 0; x < img.Bounds().Dx(); x++ {
			d := roundedDist(float64(x)+0.5, float64(y)+0.5-drop, x0, y0, float64(ww), float64(wh), radius)
			a := dark * 0.5 * math.Erfc(d/(sigma*math.Sqrt2))
			img.SetRGBA(x, y, color.RGBA{0, 0, 0, uint8(math.Round(a * 255))})
		}
	}
	barBG, edge := color.RGBA{36, 36, 36, 255}, color.RGBA{78, 78, 78, 255}
	for y := 0; y < wh; y++ {
		for x := 0; x < ww; x++ {
			d := roundedDist(float64(x)+0.5, float64(y)+0.5, 0, 0, float64(ww), float64(wh), radius)
			a := cover(d + 0.5)
			if a <= 0 {
				continue
			}
			c := termBG
			switch {
			case d > -scale:
				c = edge
			case y >= bar-px(1) && y < bar:
				c = color.RGBA{10, 10, 10, 255}
			case y < bar:
				c = barBG
			}
			over(img, side+x, top+y, c, a)
		}
	}
	draw.Draw(img, image.Rect(side+pad, top+bar, side+pad+tw, top+bar+th), term, image.Point{}, draw.Src)
	for i, c := range []color.RGBA{{0xFF, 0x5F, 0x57, 255}, {0xFE, 0xBC, 0x2E, 255}, {0x28, 0xC8, 0x40, 255}} {
		cx, cy, r := x0+(18+float64(i)*20)*scale, y0+float64(bar)/2, 6*scale
		for y := int(cy - r - 1); y <= int(cy+r+1); y++ {
			for x := int(cx - r - 1); x <= int(cx+r+1); x++ {
				if a := cover(math.Hypot(float64(x)+0.5-cx, float64(y)+0.5-cy) - r + 0.5); a > 0 {
					blend(img, x, y, c, a)
				}
			}
		}
	}
	d := font.Drawer{Dst: img, Src: image.NewUniform(color.RGBA{140, 140, 140, 255}), Face: f.regular[0]}
	w := d.MeasureString(title).Round()
	d.Dot = fixed.P(side+(ww-w)/2, top+bar/2+f.ascent-f.cellH/2)
	d.DrawString(title)
	return img
}

// roundedDist is how far a point lies inside (negative) or outside a
// rounded rectangle.
func roundedDist(px, py, x, y, w, h, r float64) float64 {
	cx, cy := x+w/2, y+h/2
	qx := math.Abs(px-cx) - (w/2 - r)
	qy := math.Abs(py-cy) - (h/2 - r)
	return math.Hypot(math.Max(qx, 0), math.Max(qy, 0)) + math.Min(math.Max(qx, qy), 0) - r
}

// over lays c, at coverage a, over what's at x, y: premultiplied, as
// image.RGBA keeps it.
func over(img *image.RGBA, x, y int, c color.RGBA, a float64) {
	o := img.RGBAAt(x, y)
	l := func(dst, src uint8) uint8 { return uint8(math.Round(float64(src)*a + float64(dst)*(1-a))) }
	img.SetRGBA(x, y, color.RGBA{l(o.R, c.R), l(o.G, c.G), l(o.B, c.B), l(o.A, 255)})
}

// plain is a frame's text without its styles, to look for what shouldn't
// be in it.
func plain(frame string) string {
	return strings.TrimRight(ansi.Strip(frame), "\n")
}

var _ = fmt.Sprint
