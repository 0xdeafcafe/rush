package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

func TestPanelsScrollAwayAndKeepComposer(t *testing.T) {
	m, _ := benchModel(200, 55)
	c := m.host
	c.input = nil
	c.sess.Info.Queue = []string{"queued alpha", "queued beta", "queued gamma"}
	c.sess.Apply(*askReq(), time.Now())
	render := func() string { return ansi.Strip(strings.Join(m.rushPane(120, 50), "\n")) }
	tail := render()
	if !strings.Contains(tail, "Which rules first?") || !strings.Contains(tail, "queued alpha") {
		t.Fatalf("tail missing its panels:\n%s", tail)
	}
	box := c.boxY
	panelRows := c.auxRows
	// Move far enough to put every panel below the visible content.
	c.scroll = panelRows + 20
	c.scrollOnly = true
	up := render()
	if strings.Contains(up, "queued alpha") || strings.Contains(up, "Which rules first?") {
		t.Fatalf("panels remained pinned:\n%s", up)
	}
	if c.boxY != box {
		t.Fatal("composer moved", box, c.boxY)
	}
	if len(c.qAt) != 0 {
		t.Fatal("hidden queue remains clickable", c.qAt)
	}
	if c.bodyRows < 25 {
		t.Fatal("panels still reserve conversation space", c.bodyRows)
	}
	// A timer redraw must not bring a focused question back over history.
	c.cardFocus = true
	c.panelFocus = true
	c.scrollOnly = false
	before := c.scroll
	render()
	if c.scroll != before {
		t.Fatal("focused card snapped back on redraw")
	}
	m.paneKey(tea.KeyPressMsg{}, "down")
	render()
	if c.scroll != 0 {
		t.Fatal("question navigation did not reveal the question")
	}
	// Return to the tail; clicking a visible queue row keeps its old behavior.
	c.cardFocus = false
	c.scroll = 0
	c.scrollOnly = true
	render()
	for row, qi := range c.qAt {
		y := c.dockY + c.qTop + row
		m.clickRow(c, 0, y)
		if c.sel != "q:"+string(rune('0'+qi)) {
			t.Fatal("queue geometry did not follow scroll", c.sel)
		}
	}
	if os.Getenv("RUSH_PANELS_PREVIEW") != "" {
		c.sel = ""
		c.scroll = 0
		os.WriteFile("/tmp/rush-panels-tail.ansi", []byte(strings.Join(m.rushPane(120, 50), "\n")), 0600)
		c.scroll = panelRows + 20
		c.scrollOnly = true
		os.WriteFile("/tmp/rush-panels-up.ansi", []byte(strings.Join(m.rushPane(120, 50), "\n")), 0600)
	}
}

func TestProjectsIncludesLargeOldAndRunningStorage(t *testing.T) {
	m := &Model{store: &state.Store{}, snap: &fleet.Snapshot{At: time.Now()}}
	m.snap.Agents = []*fleet.Agent{
		{Key: "old", DisplayName: "old session", Temp: 40 << 30, UpdatedAt: time.Now().Add(-48 * time.Hour)},
		{Key: "live", DisplayName: "live session", Temp: 2 << 30, PID: 12},
	}
	rows := m.tempRows(120)
	text := ""
	found := map[string]bool{}
	for _, r := range rows {
		text += ansi.Strip(r.line) + "\n"
		if r.temp != nil {
			found[r.temp.Key] = true
		}
	}
	if !found["old"] || !found["live"] || !strings.Contains(text, "stop before cleaning") {
		t.Fatal(text)
	}
	m.measuring = true
	m.snap.Agents = nil
	rows = m.tempRows(120)
	text = ""
	for _, r := range rows {
		text += ansi.Strip(r.line)
	}
	if !strings.Contains(text, "measuring session storage") {
		t.Fatal("measurement hidden", text)
	}
}

func TestTempMeasurementsPublishBeforeNextSession(t *testing.T) {
	m, _ := benchModel(180, 40)
	m.loader = &fleet.Loader{Temp: fleet.NewTempSizes()}
	a, b := m.snap.Agents[0], m.snap.Agents[1]
	m.snap.Agents = []*fleet.Agent{a, b}
	cmd := m.onTempMeasured(tempMeasuredMsg{remaining: []*fleet.Agent{b}, key: a.Key, size: fleet.TempSize{Bytes: 40 << 30, At: time.Now()}})
	if a.Temp != 40<<30 || m.loader.Temp.Bytes(a.Key) != 40<<30 {
		t.Fatal("first measurement not published")
	}
	if cmd == nil || !m.measuring {
		t.Fatal("did not continue the measurement queue")
	}
	cmd = m.onTempMeasured(tempMeasuredMsg{key: b.Key, size: fleet.TempSize{Bytes: 1, At: time.Now()}})
	if cmd != nil || m.measuring {
		t.Fatal("finished measurements still running")
	}
}
