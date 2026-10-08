package ui

import (
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
)

func TestMinimapCachesChangedRowsAndInvalidatesGeometry(t *testing.T) {
	lines := []convo.Line{{Text: paint(cBlue, "you   a prompt"), Ref: "t1"}, {Text: "  answer 界 e\u0301 👩‍💻"}, {Text: paint(cRed, "failed"), Ref: "t1:s:tool"}}
	var mm conversationMap
	mm.prepare(lines, 100, 20)
	if mm.parsed != 3 {
		t.Fatal(mm.parsed)
	}
	mm.prepare(lines, 100, 20)
	if mm.parsed != 3 {
		t.Fatal("unchanged rows decoded again")
	}
	lines[1].Text += " more"
	mm.prepare(lines, 100, 20)
	if mm.parsed != 6 {
		t.Fatal("did not decode only the changed turn", mm.parsed)
	}
	mm.prepare(lines, 100, 25)
	if mm.parsed != 6 {
		t.Fatal("height change re-parsed text")
	}
	mm.prepare(lines, 80, 25)
	if mm.parsed != 9 {
		t.Fatal("width change did not reproject text")
	}
	for i := range mm.height {
		if cellw.String(mm.line(i)) != miniWidth {
			t.Fatal("wrong rail width")
		}
	}
	mm.prepare(lines[:1], 80, 25)
	if mm.total != 1 || len(mm.groups) != 1 {
		t.Fatal("stale transcript rows retained")
	}
}

func TestMinimapViewportMouseAndSetting(t *testing.T) {
	m, _ := benchModel(200, 55)
	c := m.host
	c.historyMode = convo.HistoryOpen // turns whole, so the window's edges fall on them
	// ponytail: a turn cut at the drawn window's edge is decoded again on a
	// later visit; 49 rows keeps the edges on whole turns.
	m.rushPane(120, 49)
	// A resize lays out a long history over several budgeted frames. Measure
	// pure scrolling only once that independent background work is settled.
	for i := 0; c.stale && i < 100; i++ {
		m.rushPane(120, 49)
	}
	if c.stale {
		t.Fatal("conversation did not finish relayout")
	}
	mm := &c.minimap
	if !mm.visible || c.paneW != 120-miniWidth {
		t.Fatal("missing minimap")
	}
	// Jump to the top through the actual mouse dispatch, before text selection.
	m.Update(tea.MouseClickMsg{X: mm.x + 3, Y: mm.y, Button: tea.MouseLeft})
	m.rushPane(120, 49)
	if mm.start != 0 || c.txt.drag || !mm.dragging {
		t.Fatalf("top click start=%d text drag=%v", mm.start, c.txt.drag)
	}
	m.Update(tea.MouseMotionMsg{X: mm.x + 3, Y: mm.y + mm.height - 1, Button: tea.MouseLeft})
	m.rushPane(120, 49)
	if c.scroll != 0 || mm.end != c.shownTotal { // turns drawn on the way are counted as drawn
		t.Fatalf("bottom drag scroll=%d end=%d total=%d", c.scroll, mm.end, c.shownTotal)
	}
	m.Update(tea.MouseReleaseMsg{X: mm.x + 3, Y: mm.y + mm.height - 1, Button: tea.MouseLeft})
	if mm.dragging {
		t.Fatal("release did not stop drag")
	}
	// Turns once drawn are kept: going there again decodes nothing.
	parsed := mm.parsed
	m.Update(tea.MouseClickMsg{X: mm.x + 3, Y: mm.y, Button: tea.MouseLeft})
	m.rushPane(120, 49)
	m.Update(tea.MouseMotionMsg{X: mm.x + 3, Y: mm.y + mm.height - 1, Button: tea.MouseLeft})
	m.rushPane(120, 49)
	m.Update(tea.MouseReleaseMsg{X: mm.x + 3, Y: mm.y + mm.height - 1, Button: tea.MouseLeft})
	if mm.parsed != parsed {
		t.Fatalf("scroll decoded %d rows", mm.parsed-parsed)
	}
	// Pressing inside the viewport must not jump, even with a one-row thumb.
	first, _ := mm.bounds()
	before := c.scroll
	m.pressMinimap(c, mm.x+2, mm.y+first)
	m.dragMinimap(c, mm.y+first)
	if c.scroll != before {
		t.Fatalf("grabbing thumb jumped from %d to %d", before, c.scroll)
	}
	mm.dragging = false
	c.scroll = 50
	c.scrollOnly = true
	m.rushPane(120, 49)
	first, last := mm.bounds()
	if first < 0 || last > mm.height || first >= last {
		t.Fatal("invalid viewport bounds", first, last)
	}
	m.store.Config.HideMinimap = true
	m.rushPane(120, 49)
	if mm.visible || c.paneW != 120 || mm.hit(mm.x, mm.y) {
		t.Fatal("disabled rail still active")
	}
	m.store.Config.HideMinimap = false
	m.rushPane(80, 50)
	if mm.visible || c.paneW != 80 {
		t.Fatal("rail crowds narrow pane")
	}
}

func TestMinimapRasterReflectsSpacesColoursAndWideText(t *testing.T) {
	p := miniPalette()
	empty := miniPixels(convo.Line{Text: "  │    ─────    "}, 100, p)
	if empty != [miniColumns * 2]uint8{} {
		t.Fatal("spines fill minimap")
	}
	got := miniPixels(convo.Line{Text: paint(cRed, "界") + "                  " + paint(cGreen, "ok")}, 40, p)
	if got[0] != 5 || got[10] != 4 || got[5] != 0 {
		t.Fatalf("wrong spatial or colour mapping: %v", got)
	}
}

func TestMinimapPreview(t *testing.T) {
	if os.Getenv("RUSH_MINIMAP_PREVIEW") == "" {
		t.Skip("visual preview")
	}
	m, _ := benchModel(200, 55)
	m.host.sess = benchConvo(8)
	m.host.historyMode = convo.HistoryOpen
	m.host.scroll = 100
	rows := m.rushPane(120, 50)
	if err := os.WriteFile("/tmp/rush-minimap-preview.ansi", []byte(strings.Join(rows, "\n")), 0600); err != nil {
		t.Fatal(err)
	}
}

func BenchmarkMinimap(b *testing.B) {
	for _, n := range []int{3000, 30000} {
		lines := make([]convo.Line, n)
		for i := range lines {
			lines[i] = convo.Line{Text: paint(cBlue, fmt.Sprintf("  row %d", i)) + "  rendering a long conversation with cached miniature text"}
		}
		b.Run(fmt.Sprintf("%d/cold", n), func(b *testing.B) {
			b.ReportAllocs()
			for b.Loop() {
				var mm conversationMap
				mm.prepare(lines, 100, 50)
			}
		})
		b.Run(fmt.Sprintf("%d/stream", n), func(b *testing.B) {
			var mm conversationMap
			mm.prepare(lines, 100, 50)
			b.ReportAllocs()
			b.ResetTimer()
			for b.Loop() {
				lines[n-1].Text += "x"
				if len(lines[n-1].Text) > 200 {
					lines[n-1].Text = "stream"
				}
				mm.prepare(lines, 100, 50)
			}
		})
		b.Run(fmt.Sprintf("%d/warm", n), func(b *testing.B) {
			var mm conversationMap
			mm.prepare(lines, 100, 50)
			b.ReportAllocs()
			b.ResetTimer()
			for b.Loop() {
				mm.prepare(lines, 100, 50)
			}
		})
		b.Run(fmt.Sprintf("%d/scroll", n), func(b *testing.B) {
			var mm conversationMap
			mm.prepare(lines, 100, 50)
			b.ReportAllocs()
			b.ResetTimer()
			for b.Loop() {
				mm.start = (mm.start + 3) % n
				mm.end = min(n, mm.start+50)
				for y := range mm.height {
					_ = mm.line(y)
				}
			}
		})
	}
}

func TestMinimapSettingAndTheme(t *testing.T) {
	m, _ := benchModel(200, 50)
	var st setting
	for _, row := range flat(m.interfaceSections()) {
		if row.label == "Conversation minimap" {
			st = row
		}
	}
	if st.set == nil {
		t.Fatal("minimap setting is unreachable")
	}
	st.set("hidden")
	if !m.store.Config.HideMinimap {
		t.Fatal("setting did not hide minimap")
	}
	st.set("shown")
	if m.store.Config.HideMinimap {
		t.Fatal("setting did not show minimap")
	}
	lines := []convo.Line{{Text: "hello", Ref: "t1"}}
	var mm conversationMap
	mm.prepare(lines, 100, 20)
	before := mm.normal[0]
	old := cBlue
	cBlue = "\x1b[38;2;1;2;3m"
	defer func() { cBlue = old }()
	mm.prepare(lines, 100, 20)
	if mm.normal[0] == before {
		t.Fatal("cached colour survived theme change")
	}
}

// The map's edge is bright beside a turn that just ended and fades with
// how long ago each one did.
func TestMinimapEdgeShadesByAge(t *testing.T) {
	m, _ := benchModel(120, 40)
	s := m.host.sess
	m.View()
	now := time.Now()
	ages := []time.Duration{time.Minute, 30 * time.Minute, 5 * time.Hour, 72 * time.Hour}
	want := []string{cText, cSub, cDim, cFaint}
	if len(s.Turns) < len(ages) {
		t.Skip("too few turns")
	}
	win := miniWindow{s: s, total: 1 << 30}
	for i, ago := range ages {
		k := len(s.Turns) - 1 - i
		s.Turns[k].End = now.Add(-ago)
		if got := miniAge(win, s.TurnRow(k), now); got != want[i] {
			t.Errorf("a turn ended %s ago: edge %q, want %q", ago, got, want[i])
		}
	}
}
