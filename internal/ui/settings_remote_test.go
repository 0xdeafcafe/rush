package ui

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"rsc.io/qr"
)

var qrCell = regexp.MustCompile(`\x1b\[38;2;([\d;]+);48;2;([\d;]+)m▀`)

// The half blocks qrLines draws read back as exactly the code's modules,
// quiet zone included: the top half is the foreground, the bottom the back.
func TestQRLinesDrawTheCode(t *testing.T) {
	url := "https://afrs-macbook-pro-1.tail00f578.ts.net:8443/#pair=AAAAAAAAAAAAAAAAAAAAAA"
	c, _ := qr.Encode(url, qr.L)
	lines := qrLines(url)
	n := c.Size + 4
	if len(lines) != (n+1)/2 {
		t.Fatalf("%d lines for %d modules", len(lines), n)
	}
	eyes := 0
	for row, l := range lines {
		cells := qrCell.FindAllStringSubmatch(l, -1)
		if len(cells) != n {
			t.Fatalf("line %d: %d cells, want %d", row, len(cells), n)
		}
		for x, m := range cells {
			for half, col := range m[1:] {
				y := 2*row + half
				if want := c.Black(x-2, y-2); (col != qrLight) != want {
					t.Fatalf("module %d,%d: colour %s, dark %v", x, y, col, want)
				}
				if col == qrEye {
					eyes++
				}
			}
		}
	}
	if eyes != 27 {
		t.Errorf("%d orange modules, want the three 3×3 eyes", eyes)
	}
}

// With RUSH_QR_PNG=path, writes the code as drawn, colour for colour, to a
// PNG: for a decoder to read (it isn't one of the tests that always run).
func TestQRLinesPNG(t *testing.T) {
	path := os.Getenv("RUSH_QR_PNG")
	if path == "" {
		t.Skip("RUSH_QR_PNG=path writes the drawn code for a decoder")
	}
	lines := qrLines(os.Getenv("RUSH_QR_TEXT"))
	const px = 8
	n := len(qrCell.FindAllStringSubmatch(lines[0], -1))
	img := image.NewRGBA(image.Rect(0, 0, n*px, len(lines)*2*px))
	rgb := func(s string) color.RGBA {
		p := strings.Split(s, ";")
		v := func(i int) uint8 { n, _ := strconv.Atoi(p[i]); return uint8(n) }
		return color.RGBA{v(0), v(1), v(2), 255}
	}
	for row, l := range lines {
		for x, m := range qrCell.FindAllStringSubmatch(l, -1) {
			for half, col := range m[1:] {
				for dy := range px {
					for dx := range px {
						img.Set(x*px+dx, (2*row+half)*px+dy, rgb(col))
					}
				}
			}
		}
	}
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}
