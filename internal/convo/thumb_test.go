package convo

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestStepPathsLink(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work/rush"
	s.Apply(host.Sent{Text: "look"}, at(0))
	s.Apply(toolUse("r1", "Read", map[string]any{"file_path": "docs/brand/icon.png"}), at(1))
	s.Apply(toolUse("e1", "Edit", map[string]any{"file_path": "/work/rush/main.go", "old_string": "a", "new_string": "b"}), at(2))
	var out strings.Builder
	for _, l := range s.Render(Options{Width: 100, Now: at(3)}) {
		out.WriteString(l.Text + "\n")
	}
	for _, w := range []string{"\x1b]8;;file:///work/rush/docs/brand/icon.png\x1b\\", "\x1b]8;;file:///work/rush/main.go\x1b\\"} {
		if !strings.Contains(out.String(), w) {
			t.Errorf("missing %q in %q", w, out.String())
		}
	}
	if p := ansi.Strip(out.String()); !strings.Contains(p, "◧ docs/brand/icon.png") || !strings.Contains(p, "✎ main.go") {
		t.Errorf("labels changed:\n%s", p)
	}
}

func TestHalfBlocks(t *testing.T) {
	px := image.NewNRGBA(image.Rect(0, 0, 2, 4))
	px.SetNRGBA(0, 0, color.NRGBA{255, 0, 0, 255})
	px.SetNRGBA(1, 0, color.NRGBA{0, 255, 0, 255})
	px.SetNRGBA(0, 1, color.NRGBA{0, 0, 255, 255})
	px.SetNRGBA(1, 1, color.NRGBA{255, 255, 255, 255})
	px.SetNRGBA(1, 2, color.NRGBA{0, 0, 0, 255})
	px.SetNRGBA(0, 3, color.NRGBA{9, 9, 9, 200})
	got := halfBlocks(px)
	want := []string{
		"\x1b[38;2;255;0;0m\x1b[48;2;0;0;255m▀\x1b[38;2;0;255;0m\x1b[48;2;255;255;255m▀\x1b[0m",
		"\x1b[38;2;9;9;9m\x1b[49m▄\x1b[38;2;0;0;0m\x1b[49m▀\x1b[0m",
	}
	if len(got) != len(want) {
		t.Fatalf("got %q", got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("row %d: got %q, want %q", i, got[i], want[i])
		}
	}
}

func TestShrinkKeepsShape(t *testing.T) {
	wide := shrink(image.NewNRGBA(image.Rect(0, 0, 1440, 900)))
	tall := shrink(image.NewNRGBA(image.Rect(0, 0, 100, 1000)))
	if b := wide.Bounds(); b.Dx() != 32 || b.Dy() != 20 {
		t.Errorf("1440x900 → %v", b)
	}
	if b := tall.Bounds(); b.Dx() != 2 || b.Dy() != 24 {
		t.Errorf("100x1000 → %v", b)
	}
}

// tinyPNG is a 4x2 image, red over blue: one row of half blocks.
func tinyPNG(t *testing.T) []byte {
	px := image.NewNRGBA(image.Rect(0, 0, 4, 2))
	for y := range 2 {
		for x := range 4 {
			c := color.NRGBA{255, 0, 0, 255}
			if y >= 1 {
				c = color.NRGBA{0, 0, 255, 255}
			}
			px.SetNRGBA(x, y, c)
		}
	}
	var b bytes.Buffer
	if err := png.Encode(&b, px); err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}

// A Read of an image draws it under the step, linked to the file, where
// it used to say [image]; its chip stands in until it's made.
func TestImageResult(t *testing.T) {
	data := base64.StdEncoding.EncodeToString(tinyPNG(t))
	msg := `{"role":"user","content":[{"type":"tool_result","tool_use_id":"img1","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"` + data + `"}}]}]}`
	ev, err := headless.DecodeMessage("user", []byte(msg), nil)
	if err != nil {
		t.Fatal(err)
	}
	if b := ev.(headless.Message).Blocks[0]; b.Text != "" || len(b.Images) != 1 || b.Images[0].MediaType != "image/png" {
		t.Fatalf("block: %+v", b)
	}
	s := New()
	s.Info.Cwd = "/work/rush"
	s.Apply(host.Sent{Text: "look"}, at(0))
	s.Apply(toolUse("img1", "Read", map[string]any{"file_path": "/work/rush/shot.png"}), at(1))
	s.Apply(ev, at(2))
	render := func() string {
		var b strings.Builder
		for _, l := range s.Render(Options{Width: 100, Now: at(3)}) {
			b.WriteString(l.Text + "\n")
		}
		return b.String()
	}
	out := render()
	if p := ansi.Strip(out); strings.Contains(p, "[image]") {
		t.Errorf("still says [image]:\n%s", p)
	}
	deadline := time.Now().Add(5 * time.Second)
	for !strings.Contains(out, "▀") && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
		out = render()
	}
	want := "\x1b]8;;file:///work/rush/shot.png\x1b\\\x1b[38;2;255;0;0m\x1b[48;2;0;0;255m▀"
	if !strings.Contains(out, want) {
		t.Errorf("no linked thumbnail in %q", out)
	}
	// It stays once a later step is the latest, and never folds away.
	s.Apply(toolUse("b1", "Bash", map[string]any{"command": "ls"}), at(3))
	s.Apply(toolResult("b1", "shot.png", false, nil), at(3))
	if out := render(); !strings.Contains(out, want) {
		t.Errorf("thumbnail gone once another step came:\n%s", ansi.Strip(out))
	}
	if foldable(s.byID["img1"]) {
		t.Error("a picture folds away")
	}
}

// A frame asking for a thumbnail never waits for it: while no slot's free
// to make it, it says it isn't ready and a frame after it's made has it.
func TestThumbNeverWaits(t *testing.T) {
	for range cap(thumbSlots) {
		thumbSlots <- struct{}{}
	}
	img := &event.ImageData{Data: tinyPNG(t)}
	k := thumbKey{"wait1", 0}
	start := time.Now()
	if _, ok := thumbOf(k, img); ok || time.Since(start) > 100*time.Millisecond {
		t.Fatalf("ready without a slot, or waited %v", time.Since(start))
	}
	for range cap(thumbSlots) {
		<-thumbSlots
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		rows, ok := thumbOf(k, img)
		if ok {
			if len(rows) != 1 {
				t.Errorf("rows: %q", rows)
			}
			return
		}
		if time.Now().After(deadline) {
			t.Fatal("never made")
		}
		time.Sleep(5 * time.Millisecond)
	}
}

// A file's preview fills the cells it's given, keeping the image's shape.
func TestPreviewFits(t *testing.T) {
	path := filepath.Join(t.TempDir(), "wide.png")
	f, _ := os.Create(path)
	png.Encode(f, image.NewNRGBA(image.Rect(0, 0, 1600, 400)))
	f.Close()
	rows := Preview(path, 80, 30, false)
	if len(rows) != 10 || cellw.String(ansi.Strip(rows[0])) != 80 {
		t.Fatalf("want 80×10 cells, got %d rows", len(rows))
	}
	if Preview(filepath.Join(t.TempDir(), "none.png"), 80, 30, false) != nil {
		t.Fatal("a missing file has no preview")
	}
}

// A cell split down the middle draws as a half block, not one blurred
// colour; a fine preview has twice the pixels across.
func TestQuadrants(t *testing.T) {
	px := image.NewNRGBA(image.Rect(0, 0, 2, 4))
	for y := range 4 {
		px.SetNRGBA(0, y, color.NRGBA{R: 255, A: 255})
		px.SetNRGBA(1, y, color.NRGBA{B: 255, A: 255})
	}
	rows := quadrants(px)
	if len(rows) != 1 || !strings.Contains(rows[0], "▌") {
		t.Fatalf("got %q", rows)
	}
	path := filepath.Join(t.TempDir(), "wide.png")
	f, _ := os.Create(path)
	png.Encode(f, image.NewNRGBA(image.Rect(0, 0, 1600, 400)))
	f.Close()
	if rows := Preview(path, 80, 30, true); len(rows) != 10 || cellw.String(ansi.Strip(rows[0])) != 80 {
		t.Fatalf("fine: want 80×10 cells, got %d rows", len(rows))
	}
}
