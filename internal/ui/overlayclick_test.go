package ui

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/termimg"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
)

// clickSheet is a sheet as most are: tabs, rows under a cursor and a row
// of keys, every one of them on a key.
type clickSheet struct {
	tab, cur int
	picked   string
	keyed    []string
}

var clickRows = []string{"alpha", "bravo", "charlie", "delta"}

func (s *clickSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Click", "a sheet to click", w), "", sheetTabs([]string{"One", "Two", "Three"}, s.tab), ""}
	for i, r := range clickRows {
		out = append(out, sheetRow(r, i == s.cur, w))
	}
	return append(out, "", keysFit(w, "↑↓", "choose", "enter", "pick", "x", "remove", "esc", "close"))
}

func (s *clickSheet) key(m *Model, k tea.KeyPressMsg, key string) tea.Cmd {
	s.keyed = append(s.keyed, key)
	switch key {
	case "up":
		s.cur = max(0, s.cur-1)
	case "down":
		s.cur = min(len(clickRows)-1, s.cur+1)
	case "]":
		s.tab = (s.tab + 1) % 3
	case "enter":
		s.picked = clickRows[s.cur]
		m.sheet = nil
	case "x":
		s.picked = "x on " + clickRows[s.cur]
	case "esc":
		m.sheet = nil
	}
	return nil
}

// at is where text is on the screen as last drawn, for a click.
func clickAt(t *testing.T, screen, text string) (int, int) {
	t.Helper()
	for y, l := range strings.Split(ansi.Strip(screen), "\n") {
		if x := strings.Index(l, text); x >= 0 {
			return ansi.StringWidth(l[:x]), y
		}
	}
	t.Fatalf("no %q on screen:\n%s", text, ansi.Strip(screen))
	return 0, 0
}

func clickModel(t *testing.T) *Model {
	t.Setenv("HOME", t.TempDir())
	return &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, w: 120, h: 40}
}

// A click in a sheet is the keys it stands for: on a row, the cursor goes
// there and enter picks it; on a tab, that tab; on a key, that key; and
// outside the box, esc.
func TestSheetClickRouting(t *testing.T) {
	base := strings.Repeat("\n", 39)
	click := func(m *Model, text string) {
		x, y := clickAt(t, m.sheetView(base), text)
		m.Update(tea.MouseClickMsg{X: x + 1, Y: y, Button: tea.MouseLeft})
	}

	m := clickModel(t)
	s := &clickSheet{}
	m.sheet = s
	click(m, "charlie")
	if s.picked != "charlie" || m.sheet != nil {
		t.Fatalf("a row: picked %q, sheet %T, keys %v", s.picked, m.sheet, s.keyed)
	}

	s = &clickSheet{cur: 3}
	m.sheet = s
	click(m, "bravo")
	if s.picked != "bravo" {
		t.Fatalf("a row above: picked %q, keys %v", s.picked, s.keyed)
	}

	s = &clickSheet{}
	m.sheet = s
	click(m, "Three")
	if s.tab != 2 || m.sheet == nil {
		t.Fatalf("a tab: tab %d, keys %v", s.tab, s.keyed)
	}
	click(m, "One")
	if s.tab != 0 {
		t.Fatalf("back to the first tab: %d, keys %v", s.tab, s.keyed)
	}

	click(m, "x remove")
	if s.picked != "x on alpha" || m.sheet == nil {
		t.Fatalf("a key: picked %q, keys %v", s.picked, s.keyed)
	}

	// The title is no row: the cursor stays put and nothing's picked.
	s.picked = ""
	click(m, "a sheet to click")
	if s.cur != 0 || s.picked != "" || m.sheet == nil {
		t.Fatalf("the title: cur %d picked %q, keys %v", s.cur, s.picked, s.keyed)
	}

	m.sheetView(base)
	m.Update(tea.MouseClickMsg{X: 0, Y: 0, Button: tea.MouseLeft})
	if m.sheet != nil {
		t.Fatalf("outside: sheet %T, keys %v", m.sheet, s.keyed)
	}
}

// ctrl+x's question takes a click on any of its keys.
func TestConfirmClick(t *testing.T) {
	m := clickModel(t)
	var did string
	m.confirm = &confirmation{question: "Hide it?", onYes: func() tea.Cmd { did = "yes"; return nil },
		more: []confirmChoice{{"r", "restart", func() tea.Cmd { did = "restart"; return nil }}}}
	base := strings.Repeat("\n", 39)
	x, y := clickAt(t, m.confirmModal(base), "r restart")
	m.Update(tea.MouseClickMsg{X: x + 2, Y: y, Button: tea.MouseLeft})
	if did != "restart" || m.confirm != nil {
		t.Fatalf("did %q, confirm %v", did, m.confirm != nil)
	}
}

// A message of several images opens on the one clicked; ← → go round them,
// the count says which, and a click on the strip shows that one.
func TestImageViewerMovesThroughImages(t *testing.T) {
	was := termimg.Current
	termimg.Current = termimg.Blocks
	defer func() { termimg.Current = was }()
	var paths []string
	for i := range 3 {
		paths = append(paths, writeTestPNG(t, 60+40*i, 40))
	}
	m := clickModel(t)
	s := &messageSheet{messages: []convo.UserMessage{{Ref: "t1", Text: "[Image #1] [Image #2] [Image #3]", Images: paths}}, tab: 1}
	m.sheet = s
	ready := func() string {
		if cmd := s.prepare(m.sheetWidth()-4, max(1, m.h-11)); cmd != nil {
			cmd()
		}
		return ansi.Strip(m.sheetView(strings.Repeat("\n", m.h-1)))
	}
	if screen := ready(); !strings.Contains(screen, "1 / 3") || s.picture == nil || len(s.stripAt) != 3 {
		t.Fatalf("first image: strip %v\n%s", s.stripAt, screen)
	}
	s.key(m, tea.KeyPressMsg{}, "right")
	if s.tab != 2 {
		t.Fatalf("→: %d", s.tab)
	}
	s.key(m, tea.KeyPressMsg{}, "left")
	s.key(m, tea.KeyPressMsg{}, "left")
	if s.tab != 3 {
		t.Fatalf("← round: %d", s.tab)
	}
	ready()
	m.Update(tea.MouseClickMsg{X: m.sheetAt[0] + s.stripAt[1][0] + 1, Y: m.sheetAt[1] + s.stripY + 1, Button: tea.MouseLeft})
	if s.tab != 2 || m.sheet == nil {
		t.Fatalf("strip click: tab %d sheet %T", s.tab, m.sheet)
	}
	if screen := ready(); !strings.Contains(screen, "2 / 3") {
		t.Fatalf("count:\n%s", screen)
	}
}

func writeTestPNG(t *testing.T, w, h int) string {
	t.Helper()
	img := image.NewNRGBA(image.Rect(0, 0, w, h))
	for y := range h {
		for x := range w {
			img.SetNRGBA(x, y, color.NRGBA{R: uint8(3 * x), G: uint8(5 * y), B: 90, A: 255})
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
