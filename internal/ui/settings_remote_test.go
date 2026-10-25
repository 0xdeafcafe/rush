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
	"time"

	"rsc.io/qr"

	"github.com/0xdeafcafe/rush/internal/remote"
)

var qrCell = regexp.MustCompile(`\x1b\[38;2;([\d;]+);48;2;([\d;]+)m▀`)

// The half blocks qrLines draws read back as exactly the code's modules,
// quiet zone included (the top half is the foreground, the bottom the
// back), but for the bottle in the middle.
func TestQRLinesDrawTheCode(t *testing.T) {
	url := "https://afrs-macbook-pro-1.tail00f578.ts.net:8443/#pair=K7QM3XHP"
	c, _ := qr.Encode(url, qr.Q)
	lines := qrLines(url)
	n := c.Size + 4
	if len(lines) != (n+1)/2 {
		t.Fatalf("%d lines for %d modules", len(lines), n)
	}
	eyes, bottle := 0, 0
	bw, bh := len(qrBottle[0])+2, len(qrBottle)+2
	bx, by := (c.Size-bw)/2+2, (c.Size-bh)/2+2
	for row, l := range lines {
		cells := qrCell.FindAllStringSubmatch(l, -1)
		if len(cells) != n {
			t.Fatalf("line %d: %d cells, want %d", row, len(cells), n)
		}
		for x, m := range cells {
			for half, col := range m[1:] {
				y := 2*row + half
				if x >= bx && x < bx+bw && y >= by && y < by+bh {
					if col != qrLight {
						bottle++
					}
					continue
				}
				if want := c.Black(x-2, y-2); (col != qrLight) != want {
					t.Fatalf("module %d,%d: colour %s, dark %v", x, y, col, want)
				}
				if col == qrEye {
					eyes++
				}
			}
		}
	}
	if bottle != 58 {
		t.Errorf("%d of the bottle's pixels, want 58", bottle)
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

// Paired devices list with when they were last seen, x twice unpairs one,
// and the pair sheet says when a device turns up.
func TestRemotePageDevices(t *testing.T) {
	m, _ := accountsModel(t)
	remoteSnap.Lock()
	remoteSnap.at, remoteSnap.cfg, remoteSnap.devErr = time.Now(), remote.Config{}, nil
	remoteSnap.devs = []remote.Device{
		{ID: "a1", Kind: "web", Name: "iPhone · Safari", LastSeen: time.Now().Add(-3 * time.Hour)},
		{ID: "b2", Kind: "ssh", Name: "Termius · abcdefgh", Live: 1},
	}
	remoteSnap.Unlock()
	t.Cleanup(func() { remoteSnap.Lock(); remoteSnap.at, remoteSnap.devs = time.Time{}, nil; remoteSnap.Unlock() })
	devices := func() []setting {
		for _, s := range m.remoteSections() {
			if s.title == "Devices" {
				return s.rows
			}
		}
		return nil
	}
	// line draws device row i as the page does, from a fresh build of it.
	line := func(i int) string { return m.settingRow(false, devices()[i], 24, 20, 80) }
	if n := len(devices()); n != 2 {
		t.Fatalf("%d device rows", n)
	}
	if l := line(0); !strings.Contains(l, "iPhone · Safari") || !strings.Contains(l, "seen 3h ago") {
		t.Errorf("web row: %q", l)
	}
	if l := line(1); !strings.Contains(l, "connected") {
		t.Errorf("ssh row: %q", l)
	}
	if cmd, used := devices()[0].key("x"); cmd != nil || !used || !strings.Contains(line(0), "x again") {
		t.Errorf("one x unpairs, or isn't asked again: %v", line(0))
	}
	devices()[0].key("down")
	if strings.Contains(line(0), "x again") {
		t.Error("another key doesn't take back the x")
	}

	s := &pairSheet{paired: &remote.Device{Name: "Blink · 12345678"}}
	if b := strings.Join(s.body(m, 72, 40), "\n"); !strings.Contains(b, "✓ Paired") || !strings.Contains(b, "Blink") {
		t.Errorf("paired sheet: %s", b)
	}
}

// The page is serve's plugins, how it's reached and whether SSH is let in,
// and the web app from remote.json; other machines only once there are some.
func TestRemotePageSwitches(t *testing.T) {
	m, _ := accountsModel(t)
	remoteSnap.Lock()
	remoteSnap.at, remoteSnap.devs, remoteSnap.devErr = time.Now(), nil, nil
	remoteSnap.cfg = remote.Config{Reach: remote.ReachTailscale, Web: true}
	remoteSnap.on = map[string]bool{remote.PluginTailscale: true}
	remoteSnap.Unlock()
	t.Cleanup(func() {
		remoteSnap.Lock()
		remoteSnap.at, remoteSnap.cfg, remoteSnap.on = time.Time{}, remote.Config{}, nil
		remoteSnap.Unlock()
	})
	rows := map[string]setting{}
	var titles []string
	for _, s := range m.remoteSections() {
		titles = append(titles, s.title)
		for _, r := range s.rows {
			rows[r.label] = r
		}
	}
	if rows["Over Tailscale"].value != "on" || rows["Through Cloudflare"].value != "off" || rows["Remote mode"].value != "off" {
		t.Errorf("tailscale %q, cloudflare %q, remote mode %q", rows["Over Tailscale"].value, rows["Through Cloudflare"].value, rows["Remote mode"].value)
	}
	if !strings.Contains(rows["Over Tailscale"].what, "remote-tailscale plugin") {
		t.Errorf("the row doesn't name its plugin: %q", rows["Over Tailscale"].what)
	}
	if rows["Web app"].value != "on" || rows["Terminal (SSH)"].value != "off" {
		t.Errorf("web %q, ssh %q", rows["Web app"].value, rows["Terminal (SSH)"].value)
	}
	if strings.Join(titles, ",") != "Remote" {
		t.Errorf("sections %v: other machines shows with none added", titles)
	}
	remoteSnap.Lock()
	remoteSnap.cfg.Peers = []remote.Peer{{Name: "box"}}
	remoteSnap.Unlock()
	if s := m.remoteSections(); s[len(s)-1].title != "Other machines" {
		t.Errorf("no Other machines section with a peer added")
	}
}
