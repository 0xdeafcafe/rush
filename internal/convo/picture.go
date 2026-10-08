package convo

import (
	"bytes"
	"image"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"

	"golang.org/x/image/draw"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/photon/termimg"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// A picture is an image fitted to a box of cells at the terminal's best
// (termimg): Kitty placeholders, else quarter blocks, its shape kept by the
// cell's real size. Each image, at each size, is made once, off the UI
// goroutine, and read without a lock after. A Kitty picture's lines are
// placeholders, so a turn's cached lines keep drawing it wherever they land.

type picKey struct {
	path   string
	img    *event.ImageData // one held in memory, by itself
	w, h   int
	cw, ch int
	drawn  bool // as Kitty graphics, not blocks
}

type picture struct {
	rows atomic.Pointer[[]string] // nil until made; empty when unreadable
	done chan struct{}
}

var (
	pictures sync.Map // picKey → *picture
	picCount atomic.Int64
	// picWrites are Kitty transmissions made and not yet written: the UI
	// writes them raw (PictureWrites).
	picWrites = make(chan string, 256)
	picMade   = make(chan struct{}, 1)
	picBusy   atomic.Int64 // pictures being made
)

// Picture is img fitted to w cells by h rows: its rows once made (none when
// it can't be read), and a channel closed when it is. It never decodes or
// waits: the first ask starts the making.
func Picture(img *event.ImageData, w, h int) (rows []string, ready bool, made <-chan struct{}) {
	return PictureDrawn(img, w, h, termimg.Drawn())
}

// PictureDrawn is Picture, sharp or in blocks as asked rather than as the
// Pictures setting says; in blocks whatever's asked where the terminal
// can't show them sharp.
func PictureDrawn(img *event.ImageData, w, h int, sharp bool) (rows []string, ready bool, made <-chan struct{}) {
	cw, ch := termimg.Cell()
	k := picKey{path: img.Path, w: w, h: h, cw: cw, ch: ch, drawn: sharp && termimg.Current == termimg.Kitty}
	if k.path == "" {
		k.img = img
	}
	v, ok := pictures.Load(k)
	if !ok {
		if picCount.Add(1) > 512 {
			pictures.Clear() // made again as they're drawn
			picCount.Store(1)
		}
		p := &picture{done: make(chan struct{})}
		if v, ok = pictures.LoadOrStore(k, p); !ok {
			picBusy.Add(1)
			go func() {
				thumbSlots <- struct{}{}
				rows := drawPicture(img, w, h, cw, ch, k.drawn)
				<-thumbSlots
				p.rows.Store(&rows)
				close(p.done)
				lookupsGen.Add(1)
				picBusy.Add(-1)
				select {
				case picMade <- struct{}{}:
				default:
				}
			}()
		}
	}
	p := v.(*picture)
	if r := p.rows.Load(); r != nil {
		return *r, true, p.done
	}
	lookupWaits.Add(1)
	return nil, false, p.done
}

// SetPixelPictures draws pictures in coloured blocks even where the
// terminal could show them sharp, and has every turn drawn again.
func SetPixelPictures(on bool) {
	termimg.SetBlocks(on)
	lookupsGen.Add(1)
}

// PictureMade has a value once a picture's made since it was last taken;
// PicturesBusy is whether any are being made.
func PictureMade() <-chan struct{} { return picMade }
func PicturesBusy() bool           { return picBusy.Load() > 0 }

// PictureWrites are the Kitty transmissions waiting to be written to the
// terminal, raw, outside the frame; "" when there are none.
func PictureWrites() string {
	var b strings.Builder
	for {
		select {
		case s := <-picWrites:
			b.WriteString(s)
		default:
			return b.String()
		}
	}
}

// drawPicture makes a picture: it reads and decodes, so it's for off the
// UI goroutine.
func drawPicture(img *event.ImageData, w, h, cw, ch int, drawn bool) []string {
	data := img.Data
	if len(data) == 0 && img.Path != "" {
		st, err := os.Stat(img.Path)
		if err != nil || st.Size() > 64<<20 {
			return []string{}
		}
		if data, err = os.ReadFile(img.Path); err != nil {
			return []string{}
		}
	}
	cfg, _, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil || cfg.Width*cfg.Height > 64<<20 {
		return []string{}
	}
	src, _, err := image.Decode(bytes.NewReader(data))
	if err != nil {
		return []string{}
	}
	// Blocks take the cells the picture would take sharp: no bigger.
	cols, rows := termimg.Fit(cfg.Width, cfg.Height, w, h, cw, ch)
	if cols == 0 {
		return []string{}
	}
	if !drawn {
		return quadrants(scaled(src, 2*cols, 4*rows))
	}
	id := termimg.ID(data, cols, rows)
	seq, err := termimg.Transmit(id, scaled(src, min(cfg.Width, cols*cw), min(cfg.Height, rows*ch)), cols, rows)
	if err != nil {
		return []string{}
	}
	select {
	case picWrites <- seq:
	default:
		return []string{} // nothing's writing them: drawn as nothing
	}
	return termimg.Placeholders(id, cols, rows)
}

// scaled is src resampled to exactly w by h.
func scaled(src image.Image, w, h int) *image.NRGBA {
	out := image.NewNRGBA(image.Rect(0, 0, max(1, w), max(1, h)))
	draw.CatmullRom.Scale(out, out.Bounds(), src, src.Bounds(), draw.Src, nil)
	return out
}

// SentImages are a message's images: those it held, else its files.
func SentImages(held []*event.ImageData, paths []string) []*event.ImageData {
	if len(held) > 0 {
		return held
	}
	out := make([]*event.ImageData, len(paths))
	for i, p := range paths {
		out[i] = &event.ImageData{Path: p}
	}
	return out
}

// thumbRows are a message's images as thumbnails a few rows tall, side by
// side in w cells, each opening whole when clicked as its chip does; nil
// until every one is made and readable, so its chips stand in till then.
func thumbRows(ref string, imgs []*event.ImageData, w int) []string {
	if len(imgs) == 0 {
		return nil
	}
	pics := make([][]string, len(imgs))
	for i, img := range imgs {
		// The same height each, so a row of them lines up; only one wider
		// than the room is shorter. Small, as the chips they stand for:
		// opened, an image shows whole.
		rows, ready, _ := Picture(img, max(8, min(32, w-2)), 3)
		if !ready || len(rows) == 0 {
			return nil
		}
		pics[i] = framed(rows)
	}
	rows, _ := Tile(pics, w, 2, func(i int, r string) string {
		return "\x1b]8;;rush:message/" + ref + "/image/" + strconv.Itoa(i+1) + "\x1b\\" + r + "\x1b]8;;\x1b\\"
	})
	return rows
}

// framed is a picture in a faint rounded edge, so it sits apart from
// the message around it.
func framed(rows []string) []string {
	bar := strings.Repeat("─", cellw.String(rows[0]))
	out := make([]string, 0, len(rows)+2)
	out = append(out, faint("╭"+bar+"╮"))
	for _, r := range rows {
		out = append(out, faint("│")+r+faint("│"))
	}
	return append(out, faint("╰"+bar+"╯"))
}

// Tile lays pictures side by side, gap cells apart, wrapping to w cells;
// each row of them top-aligned, a blank row between. wrap, when given, dresses picture i's
// rows (a link, say). at is where each landed: x, y, width and height.
func Tile(pics [][]string, w, gap int, wrap func(i int, r string) string) (rows []string, at [][4]int) {
	var group []int
	x := 0
	flush := func() {
		if len(rows) > 0 {
			rows = append(rows, "") // a row of them apart from the last
		}
		h := 0
		for _, i := range group {
			h = max(h, len(pics[i]))
		}
		for y := range h {
			var b strings.Builder
			for k, i := range group {
				if k > 0 {
					b.WriteString(blanks(gap))
				}
				pw := cellw.String(pics[i][0])
				switch {
				case y >= len(pics[i]):
					b.WriteString(blanks(pw))
				case wrap != nil:
					b.WriteString(wrap(i, pics[i][y]))
				default:
					b.WriteString(pics[i][y])
				}
			}
			rows = append(rows, b.String())
		}
		group, x = nil, 0
	}
	at = make([][4]int, len(pics))
	for i, p := range pics {
		if len(p) == 0 {
			continue
		}
		pw := cellw.String(p[0])
		if len(group) > 0 && x+gap+pw > w {
			flush()
		}
		if len(group) > 0 {
			x += gap
		}
		y := len(rows)
		if y > 0 {
			y++ // the blank row flush puts first
		}
		at[i] = [4]int{x, y, pw, len(p)}
		group, x = append(group, i), x+pw
	}
	if len(group) > 0 {
		flush()
	}
	return rows, at
}
