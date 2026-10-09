package ui

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	_ "github.com/0xdeafcafe/rush/internal/adapters/acp" // Kimi CLI, OpenCode
	"github.com/0xdeafcafe/rush/internal/agent"
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

func TestComposerTicksMakeRoutes(t *testing.T) {
	m, _ := benchModel(160, 50)
	m.openComposer()
	c, ok := m.sheet.(*matchSheet)
	if !ok {
		t.Skip("no harness runs here")
	}
	before := len(c.routes(m))
	if before == 0 {
		t.Fatal("ticked tiles make no routes")
	}
	// Untick every harness: nothing runs, and START won't go.
	c.row = compHarnesses
	for i, hk := range c.harns {
		if c.onH[hk] {
			c.col = i
			c.key(m, tea.KeyPressMsg{}, "space")
		}
	}
	if n := len(c.routes(m)); n != 0 {
		t.Fatalf("%d routes with no harness ticked", n)
	}
	c.row = compStart
	c.key(m, tea.KeyPressMsg{}, "enter")
	if m.sheet == nil {
		t.Fatal("START closed with nothing to run")
	}
	// Tick them back: the routes return, and START takes the chosen one.
	c.row = compHarnesses
	for i := range c.harns {
		c.col = i
		c.key(m, tea.KeyPressMsg{}, "space")
	}
	if len(c.routes(m)) < before {
		t.Fatalf("routes = %d after ticking every harness, want at least %d", len(c.routes(m)), before)
	}
	c.row = compStart
	c.key(m, tea.KeyPressMsg{}, "enter")
	if m.sheet != nil {
		t.Fatal("START didn't close the composer")
	}
	body := c.body(m, 120, 60)
	if len(body) == 0 {
		t.Fatal("no body")
	}
}

// Ticking a tile on moves START's choice to what it turns on: kimi ticked
// and STARTed starts kimi, not the Claude Code the composer opened on.
func TestComposerTickMovesChoice(t *testing.T) {
	if !agent.Runs("kimi") {
		t.Skip("kimi isn't installed here")
	}
	m, _ := benchModel(160, 50)
	m.host, m.preview, m.paneFocus = nil, false, false
	m.openComposer()
	c, ok := m.sheet.(*matchSheet)
	if !ok {
		t.Skip("no harness runs here")
	}
	tick := func(row, i int) {
		c.row, c.col = row, i
		c.key(m, tea.KeyPressMsg{}, "space")
	}
	for i, id := range c.provs {
		if id == "kimi" && !c.onP[id] {
			tick(compProviders, i)
		}
	}
	for i, hk := range c.harns {
		if hk == "kimi" && !c.onH[hk] {
			tick(compHarnesses, i)
		}
	}
	if c.o.kind != "kimi" {
		t.Fatalf("kimi ticked, START runs %q", c.o.kind)
	}
	c.row = compStart
	c.key(m, tea.KeyPressMsg{}, "enter")
	if m.startOver == nil || m.startOver.kind != "kimi" {
		t.Fatalf("START took %+v, want kimi", m.startOver)
	}
}
