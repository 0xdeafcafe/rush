package ui

import (
	"fmt"
	"slices"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/photon/theme"
	"github.com/charmbracelet/x/ansi"
)

func TestHeaderWashPreservesTextStylesAndLinks(t *testing.T) {
	link := "\x1b]8;;https://example.com\x1b\\"
	closeLink := "\x1b]8;;\x1b\\"
	text := paint(cText+bold, "Hello 界 e\u0301 👩‍💻") + "  " + link + "model" + closeLink
	for _, width := range []int{0, 1, 8, 40, 160} {
		t.Run(fmt.Sprint(width), func(t *testing.T) {
			got := headerWash(text, width, 0, "ollama")
			if cellw.String(got) != width {
				t.Fatalf("width %d: %q", cellw.String(got), got)
			}
			if strings.TrimRight(ansi.Strip(got), " ") != strings.TrimRight(ansi.Strip(fit(text, width)), " ") {
				t.Fatalf("changed visible text: %q", ansi.Strip(got))
			}
			if width >= 40 && (!strings.Contains(got, cText+bold) || !strings.Contains(got, link) || !strings.Contains(got, closeLink)) {
				t.Fatal("lost foreground style or hyperlink")
			}
			if got != headerWash(text, width, 0, "ollama") {
				t.Fatal("static header changed between frames")
			}
		})
	}
}

func TestHeaderWashThemeAndContrast(t *testing.T) {
	for _, ground := range []theme.Ground{theme.Dark, theme.Light, {BG: theme.RGB{R: 0, G: 43, B: 54}, FG: theme.RGB{R: 131, G: 148, B: 150}}} {
		for _, provider := range []theme.RGB{looks["ollama"].rgb, looks["gemini"].rgb, looks["codex"].rgb} {
			key := headerColourKey{ground: ground, harness: builtinLook.rgb, provider: provider, width: 160}
			base := ground.Surface(theme.RGB{R: 30, G: 28, B: 26})
			if got := headerColourAt(key, 0); got != base {
				t.Fatalf("left edge = %+v; want normal chrome %+v", got, base)
			}
			changed := false
			for n := range 100 {
				colour := headerColourAt(key, float64(n)/99)
				changed = changed || colour != base
				for _, check := range []struct {
					ink   theme.RGB
					floor float64
				}{
					{theme.RGB{R: 122, G: 117, B: 108}, 3},
					{theme.RGB{R: 168, G: 162, B: 152}, 4.5},
				} {
					ink := ground.Ink(check.ink)
					floor := min(check.floor, theme.Contrast(ink, base))
					if got := theme.Contrast(ink, colour); got < floor-.01 {
						t.Fatalf("contrast %.3f below %.3f on %+v", got, floor, ground)
					}
				}
			}
			if !changed {
				t.Fatal("gradient contains no colour")
			}
		}
	}
}

func TestHeaderWashRestoresBackgroundAfterReset(t *testing.T) {
	got := headerWash(paint(cBright, "name")+" after", 40, 0, "claude")
	if !strings.Contains(got, reset+"\x1b[48;2;") {
		t.Fatal("styled text reset removes background")
	}
}

func BenchmarkHeaderWash(b *testing.B) {
	text := paint(cBright+bold, "Review performance and improve models") + "   " + paint(cOrange, "✻ working 53s") + "           " + dim("ctx 20% · gpt 6")
	for _, width := range []int{100, 240} {
		b.Run(fmt.Sprint(width), func(b *testing.B) {
			headerWash(text, width, 0, "ollama")
			b.ReportAllocs()
			b.ResetTimer()
			for range b.N {
				headerWash(text, width, 0, "ollama")
			}
		})
	}
}

func TestHeaderWashRestrictsPixelsToLastFortyPercent(t *testing.T) {
	for _, ground := range []theme.Ground{theme.Dark, theme.Light} {
		for _, width := range []int{1, 2, 3, 7, 40, 80, 99, 100, 101, 140, 240} {
			for row := range 2 {
				key := headerColourKey{ground: ground, harness: builtinLook.rgb, provider: looks["codex"].rgb, width: width, row: row}
				palette := makeHeaderColours(key)
				base := ground.Surface(theme.RGB{R: 30, G: 28, B: 26}).BG()
				for col := range headerEdgeStart(width) {
					if palette[col] != base {
						t.Fatalf("width %d row %d pixel %d alters first 60%%", width, row, col)
					}
				}
				if width >= 40 && palette[width-1] == base {
					t.Fatalf("width %d row %d leaves the final cell uncoloured", width, row)
				}
			}
		}
	}
}

func TestHeaderWashFadesAcrossBlocksAndHoldsRightEdge(t *testing.T) {
	for _, ground := range []theme.Ground{theme.Dark, theme.Light} {
		for _, width := range []int{80, 120, 160, 240} {
			for row := range 2 {
				key := headerColourKey{ground: ground, harness: builtinLook.rgb, provider: looks["codex"].rgb, width: width, row: row}
				colours := makeHeaderColours(key)
				start := headerEdgeStart(width)
				// Sixteen to thirty-two cells form a gradual ramp, then a solid end.
				seen := map[string]bool{}
				for x := start; x < min(width, start+36); x += 4 {
					seen[colours[x]] = true
				}
				if len(seen) < 4 {
					t.Fatalf("width %d row %d has only %d fade levels", width, row, len(seen))
				}
				for x := max(start+32, width-4); x < width; x++ {
					if colours[x] != colours[width-1] {
						t.Fatal("right edge fades away")
					}
				}
				if colours[width-1] == colours[0] {
					t.Fatal("right edge lost its tint")
				}
			}
		}
	}
}

func TestHeaderWashPaletteIsStaticAndIdentitySpecific(t *testing.T) {
	key := headerColourKey{ground: theme.Dark, harness: builtinLook.rgb, provider: looks["codex"].rgb, width: 160}
	first := makeHeaderColours(key)
	for range 4 {
		if !slices.Equal(first, makeHeaderColours(key)) {
			t.Fatal("same geometry and identity produced a changing pattern")
		}
	}
	changed := key
	changed.ground = theme.Light
	if slices.Equal(first, makeHeaderColours(changed)) {
		t.Fatal("palette ignored theme")
	}
	changed = key
	changed.provider = looks["gemini"].rgb
	if slices.Equal(first, makeHeaderColours(changed)) {
		t.Fatal("palette ignored model provider")
	}
	changed = key
	changed.harness = looks["codex"].rgb
	if slices.Equal(first, makeHeaderColours(changed)) {
		t.Fatal("palette ignored harness")
	}
	changed = key
	changed.row = 1
	if slices.Equal(first, makeHeaderColours(changed)) {
		t.Fatal("pixel rows should have irregular, distinct edges")
	}
	// A pixel's two cells share their colour, even as nearby pixels vary.
	start := headerEdgeStart(key.width)
	for i := start; i+1 < key.width-1; i += 2 {
		if first[i] != first[i+1] {
			t.Fatal("pixel is not a two-cell block")
		}
	}
}

func TestHeaderWashIsPresentInSessionHeader(t *testing.T) {
	m, a, c := barAgentFixture(t)
	rows := m.paneHeader(a, c, 180)
	base := headerColours(180, 0, sessionAgent(c))[0]
	found := false
	for _, colour := range headerColours(180, 0, sessionAgent(c)) {
		if colour != base && strings.Contains(rows[paneTitleRow], colour) {
			found = true
			break
		}
	}
	if !found {
		t.Fatal("session header no longer applies its colour pattern")
	}
	for width, want := range map[int]int{100: 60, 101: 61, 80: 48, 1: 1} {
		if got := headerEdgeStart(width); got != want {
			t.Fatalf("width %d: starts at %d, want %d", width, got, want)
		}
	}
}

func TestHeaderWashUsesTallChunkyPatches(t *testing.T) {
	const width = 180
	a := headerColours(width, 0, "ollama")
	if !slices.Equal(a, headerColours(width, 1, "ollama")) || !slices.Equal(headerColours(width, 2, "ollama"), headerColours(width, 3, "ollama")) {
		t.Fatal("patches should span two header rows")
	}
	start := headerEdgeStart(width)
	for x := start; x+3 < width-1; x += 4 {
		for dx := 1; dx < 4; dx++ {
			if a[x] != a[x+dx] {
				t.Fatal("patches should span four columns")
			}
		}
	}
	m, agent, c := barAgentFixture(t)
	rows := m.paneHeader(agent, c, width)
	base := headerColours(width, 0, sessionAgent(c))[0]
	for _, row := range []int{0, paneTitleRow, paneMetaRow, paneTabsRow} {
		found := false
		for _, colour := range headerColours(width, row, sessionAgent(c)) {
			if colour != base && strings.Contains(rows[row], colour) {
				found = true
				break
			}
		}
		if !found {
			t.Fatalf("header row %d has no colour", row)
		}
		if ansi.StringWidth(rows[row]) != width {
			t.Fatalf("header row %d changed width", row)
		}
	}
}
