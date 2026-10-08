package convo

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi/kitty"

	"github.com/0xdeafcafe/photon/termimg"
	"github.com/0xdeafcafe/rush/internal/host"
)

func writePNG(t *testing.T, w, h int) string {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, w, h))
	for y := range h {
		for x := range w {
			img.SetNRGBA(x, y, color.NRGBA{R: uint8(x), G: uint8(y), B: 200, A: 255})
		}
	}
	p := filepath.Join(t.TempDir(), "shot.png")
	f, err := os.Create(p)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
	return p
}

// A message of only images shows them, small, in its box: its chips until
// they're made, then Kitty placeholders that open each when clicked, the
// picture itself sent to the terminal once.
func TestSentImagesShowInTheBox(t *testing.T) {
	was := termimg.Current
	termimg.Current = termimg.Kitty
	defer func() { termimg.Current = was }()
	a, b := writePNG(t, 400, 200), writePNG(t, 120, 240)
	s := New()
	s.Apply(host.Sent{Text: "[Image #1] [Image #2]", Images: []string{a, b}}, at(0))
	var out string
	for range 200 {
		var raw []string
		for _, l := range s.Render(Options{Width: 90, Now: at(1)}) {
			raw = append(raw, l.Text)
		}
		if out = strings.Join(raw, "\n"); strings.ContainsRune(out, kitty.Placeholder) {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if !strings.ContainsRune(out, kitty.Placeholder) {
		t.Fatalf("no thumbnails:\n%s", out)
	}
	if strings.Contains(out, "Image #1") {
		t.Fatalf("the chips stayed beside the thumbnails:\n%s", out)
	}
	for _, link := range []string{"rush:message/t1/image/1", "rush:message/t1/image/2"} {
		if !strings.Contains(out, link) {
			t.Fatalf("no %s:\n%s", link, out)
		}
	}
	// Side by side: one row holds both.
	both := false
	for _, l := range strings.Split(out, "\n") {
		both = both || strings.Contains(l, "image/1") && strings.Contains(l, "image/2")
	}
	if !both {
		t.Fatalf("not side by side:\n%s", out)
	}
	if w := PictureWrites(); strings.Count(w, "U=1") != 2 || !strings.Contains(w, "\x1b_G") {
		t.Fatalf("transmissions: %d placements in %d bytes", strings.Count(w, "U=1"), len(w))
	}
}

// Tile wraps pictures that don't fit beside each other, top-aligned, a
// blank row between.
func TestTile(t *testing.T) {
	p := func(w, h int) []string {
		rows := make([]string, h)
		for i := range rows {
			rows[i] = strings.Repeat("#", w)
		}
		return rows
	}
	rows, at := Tile([][]string{p(4, 2), p(3, 1), p(5, 3)}, 10, 2, nil)
	if len(rows) != 6 || rows[0] != "####  ###" || rows[1] != "####     " || rows[2] != "" || at[2] != [4]int{0, 3, 5, 3} {
		t.Fatalf("%q %v", rows, at)
	}
}
