package ui

import (
	"slices"
	"testing"

	tea "charm.land/bubbletea/v2"

	_ "github.com/0xdeafcafe/rush/internal/adapters/acp" // Kimi CLI, OpenCode
)

func TestShelfPicksAndReturns(t *testing.T) {
	m := footModel(t, 3)
	m.snap.Agents[1].Kind, m.snap.Agents[1].Spend.Model = "claude", "claude-haiku-4-5"
	m.host = nil
	tiles := m.shelf(m.startDir())
	if len(tiles) < 2 {
		t.Fatalf("shelf = %+v, want the profile's setup and a recent one", tiles)
	}
	m.shelfKey("alt+2")
	if m.startOver == nil || m.setupKey(*m.startOver) != m.setupKey(tiles[1]) {
		t.Fatalf("⌥2 didn't take the second setup: %+v", m.startOver)
	}
	if got := m.shelf(m.startDir()); m.setupKey(got[1]) != m.setupKey(tiles[1]) {
		t.Fatal("picking a setup moved it on the shelf")
	}
	m.shelfKey("alt+1")
	if m.startOver != nil {
		t.Fatalf("⌥1 kept a one-off instead of the profile's own: %+v", m.startOver)
	}
	if m.shelfKey("alt+x") || !m.shelfKey("alt+9") {
		t.Fatal("only ⌥1…9 are the shelf's")
	}
}

func TestShelfClickOnBorder(t *testing.T) {
	m, _ := benchModel(160, 50)
	m.snap.Agents[1].Kind, m.snap.Agents[1].Spend.Model = "claude", "claude-haiku-4-5"
	m.host, m.preview = nil, false // the Prompt full width
	m.rebuild()
	m.View()
	if len(m.shelfHits) < 2 {
		t.Fatalf("shelf hits = %+v, box %d wide", m.shelfHits, m.promptBox.w)
	}
	h := m.shelfHits[1]
	if !m.shelfClick(h.x0, m.promptBoxY) || m.startOver == nil {
		t.Fatal("a click on a setup didn't take it")
	}
	if m.shelfClick(h.x0, m.promptBoxY+1) {
		t.Fatal("a click off the border took a setup")
	}
}

// The New session sheet starts on the next session's model, and enter
// keeps what it shows for the next session.
func TestNewSheetStarts(t *testing.T) {
	m, _ := benchModel(160, 50)
	m.host, m.preview, m.paneFocus = nil, false, false
	m.openComposer()
	s, ok := m.sheet.(*newSheet)
	if !ok {
		t.Skip("no harness runs here")
	}
	want := s.start(m)
	if body := s.body(m, 90, 40); len(body) == 0 {
		t.Fatal("no body")
	}
	// ←→ on the model opens its list; typing narrows it; esc closes it.
	s.key(m, tea.KeyPressMsg{}, "right")
	if !s.open {
		t.Fatal("→ on the model didn't open the list")
	}
	s.key(m, tea.KeyPressMsg{Text: "zzzz-none"}, "z")
	if len(s.shown(m)) != 0 {
		t.Fatalf("a filter matching nothing shows %v", s.shown(m))
	}
	s.key(m, tea.KeyPressMsg{}, "esc")
	if s.open || m.sheet == nil {
		t.Fatal("esc in the list closed the sheet, or not the list")
	}
	s.key(m, tea.KeyPressMsg{}, "enter")
	if m.sheet != nil {
		t.Fatal("enter didn't close the sheet")
	}
	if got := m.nextStart(m.startDir()); m.setupKey(got) != m.setupKey(want) {
		t.Fatalf("the next session starts as %+v, want %+v", got, want)
	}
}

// A harness switched off in Config is never auto's, nor on the sheet.
func TestNewSheetSkipsHarnessOff(t *testing.T) {
	m, _ := benchModel(160, 50)
	m.host, m.preview, m.paneFocus = nil, false, false
	m.openComposer()
	s, ok := m.sheet.(*newSheet)
	if !ok {
		t.Skip("no harness runs here")
	}
	h := s.auto(m)
	m.store.Config.SetHarnessOn(h, false)
	for _, p := range m.newPicks() {
		if slices.Contains(m.newCells(p.id), h) {
			t.Fatalf("%s is off, but %s offers it", h, p.id)
		}
	}
}
