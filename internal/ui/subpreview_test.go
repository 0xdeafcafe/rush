package ui

import (
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

func subPreviewFixture(n int) *Model {
	m, _ := benchModel(240, 55)
	c := m.host
	c.input = nil
	c.subTails = map[string]*convo.Tail{}
	for i := range n {
		id := fmt.Sprintf("lane-%02d", i)
		c.subs = append(c.subs, convo.Subagent{ID: id, Type: id, Description: "Review the implementation", Mod: time.Now().UnixNano()})
	}
	c.sel = "sub:" + c.subs[n-1].ID
	c.subPeekID = c.subs[n-1].ID
	c.subPeek = &convo.Tail{Sess: benchConvo(30)}
	for i, v := range m.views(c) {
		if v == "subagents" {
			c.view = i
		}
	}
	return m
}

func TestSubagentSiblingScroll(t *testing.T) {
	m := subPreviewFixture(43)
	c := m.host
	draw := func() []string { return m.rushPane(170, 45) }
	lines := draw()
	if len(lines) != 45 {
		t.Fatalf("height = %d", len(lines))
	}
	if len(c.bodyRefs) != 43 {
		t.Fatalf("only %d of 43 runs are navigable", len(c.bodyRefs))
	}
	p := &c.subPreview
	if p.h != c.dockY-m.paneTop-c.bodyTop {
		t.Fatal("preview does not use actual available height")
	}
	before := c.scroll
	m.update(tea.MouseWheelMsg{X: p.x + 4, Y: p.y + 5, Button: tea.MouseWheelUp})
	draw()
	if c.scroll != before || p.scroll != 3 {
		t.Fatalf("preview wheel moved list: list %d -> %d, preview %d", before, c.scroll, p.scroll)
	}
	m.update(tea.MouseWheelMsg{X: m.paneX() + 4, Y: p.y + 5, Button: tea.MouseWheelDown})
	draw()
	if c.scroll != before-3 || p.scroll != 3 {
		t.Fatalf("list wheel moved preview: list %d, preview %d", c.scroll, p.scroll)
	}
	// Timer redraws do not snap either viewport back to its selection/tail.
	c.scrollOnly = false
	before = c.scroll
	draw()
	if c.scroll != before || p.scroll != 3 {
		t.Fatal("redraw moved the reading position")
	}
	if os.Getenv("RUSH_SUB_PREVIEW") != "" {
		os.WriteFile("/tmp/rush-subagents-preview.ansi", []byte(strings.Join(draw(), "\n")), 0600)
	}
	// Every run is reachable, including ones outside the original screen.
	c.scrollOnly = false
	for range 42 {
		m.moveSel(c, 1)
		draw()
	}
	if c.sel != "sub:lane-00" {
		t.Fatalf("navigation stopped at %q", c.sel)
	}
	found := false
	for _, ref := range c.rowRefs {
		if ref == c.sel {
			found = true
		}
	}
	if !found {
		t.Fatal("last selected run is off screen")
	}
	// The pointer over the preview hovers nothing in the list behind it.
	if r := m.subHoverAt(p.x+4, p.y+5); r != "" {
		t.Fatalf("pointer on the preview hovers %q behind it", r)
	}
	// The preview opens its own run, regardless of the list row at this y.
	id := p.id
	m.update(tea.MouseClickMsg{X: p.x + 4, Y: p.y + 5, Button: tea.MouseLeft})
	if c.subOpen != id {
		t.Fatalf("opened %q instead of preview %q", c.subOpen, id)
	}
	draw()
	if p.visible {
		t.Fatal("preview hit area remains after opening conversation")
	}
}

func TestSubagentShortListLongPreview(t *testing.T) {
	m := subPreviewFixture(2)
	c := m.host
	lines := m.rushPane(170, 45)
	if c.scroll != 0 {
		t.Fatal("preview made the short list scroll", c.scroll)
	}
	p := &c.subPreview
	if p.total <= p.rows {
		t.Fatal("fixture needs a long preview")
	}
	if !strings.Contains(ansi.Strip(lines[c.bodyTop+p.h-1]), "latest") {
		t.Fatal("preview footer clipped")
	}
	m.update(tea.MouseWheelMsg{X: p.x + 1, Y: p.y + 2, Button: tea.MouseWheelUp})
	m.rushPane(170, 45)
	top := p.total - p.scroll - p.rows
	c.subPeek.Sess.Apply(host.Sent{Text: "new output while reading above"}, time.Now())
	m.rushPane(170, 45)
	if got := p.total - p.scroll - p.rows; got != top {
		t.Fatalf("stream moved preview top %d -> %d", top, got)
	}
	// A narrow layout has one ordinary list viewport and no stale hit area.
	c.scrollOnly = false
	m.rushPane(110, 45)
	if p.visible || m.subPreviewAt(c, p.x+1, p.y+2) {
		t.Fatal("narrow layout kept preview input")
	}
}
