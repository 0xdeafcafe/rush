package ui

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// The panel is a box that fills its column exactly, says each row with
// its mark, its views as tabs along its top edge, and keeps to the
// height it's given, scrolled.
func TestViewsPanel(t *testing.T) {
	now := time.Now()
	half, four, nine := 0.45, 4.0, 9.0
	v := host.View{Name: "prs", Title: "Pull requests", Summary: "8 ready · 2 failing", Updated: now.Add(-2 * time.Minute),
		Progress: &host.Progress{Done: 8, Total: 11, Label: "merged"},
		Stats:    []host.Stat{{Value: "8", Label: "ready", Tone: "good", Trend: []float64{2, 3, 5, 8}}, {Value: "2", Label: "failing", Tone: "bad"}, {Value: "1", Label: "waiting"}},
		Groups: []host.Group{
			{Title: "Failing", Tone: "bad", Text: "Both fail on lint.", Rows: []host.Row{
				{Text: "#8947 fix the flaky ops test", Meta: "checks cancelled", Tone: "bad", Link: "https://github.com/o/r/pull/8947"},
				{Text: "#8945 lint", Meta: "another session pushed", Tone: "warn"},
				{Text: "#8950 rerun", Meta: "checks running", Tone: "busy", Progress: &half}}},
			{Title: "Time per check", Rows: []host.Row{
				{Text: "lint", Value: &four, Meta: "4m"},
				{Text: "e2e", Value: &nine, Meta: "9m", Tone: "warn"}}},
			{Title: "Ready for your approval", Folded: true, Rows: []host.Row{
				{Text: "#8965 trace filters", Meta: "all green", Tone: "good"}}},
		},
		Note: "Updated after each push."}
	tests := host.View{Name: "tests", Title: "Tests", Updated: now.Add(-time.Hour)}
	st := &viewsState{}
	rows := viewsPanel([]host.View{v, tests}, nil, 48, 40, now, st)
	if p := os.Getenv("RUSH_VIEWS_DUMP"); p != "" {
		_ = os.WriteFile(p, []byte(strings.Join(rows, "\n")+"\n"), 0o644)
	}
	if len(rows) != 40 {
		t.Fatalf("%d rows, want 40", len(rows))
	}
	for _, r := range rows {
		if cellw.String(r) != 48 {
			t.Fatalf("row %d wide: %q", cellw.String(r), ansi.Strip(r))
		}
	}
	text := ansi.Strip(strings.Join(rows, "\n"))
	for _, want := range []string{"╭─ ✦ Pull requests · Tests", "› × ─╮", "2m ago", "ready", "▾ Failing  3", "Both fail on lint.", "✗ #8947 fix the flaky", "checks cancelled",
		"Updated after each push.", "8 of 11 merged", " 73%", "▁▂▄█", "45%", "└ checks running", "▸ Ready for your approval  1", "e2e", "9m", "╰─ made by the agent · 2m ago"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	if strings.Contains(text, "#8965") {
		t.Error("a group the agent folded shows its rows")
	}
	if len(st.tabs) != 2 || st.tabs[1].name != "tests" {
		t.Fatalf("tabs: %+v", st.tabs)
	}
	// Short, it scrolls: the bottom edge says how much more.
	short := viewsPanel([]host.View{v}, nil, 48, 10, now, &viewsState{scroll: 3})
	if len(short) != 10 || !strings.Contains(ansi.Strip(short[9]), "↑ 3 · ↓") {
		t.Fatalf("scrolled: %q", ansi.Strip(strings.Join(short, "\n")))
	}
	// Folded, it's a slim box with the mark that opens it again.
	shut := viewsPanel([]host.View{v}, nil, viewsStrip, 10, now, &viewsState{shut: true})
	if len(shut) != 10 || !strings.Contains(ansi.Strip(shut[1]), "‹") {
		t.Fatalf("folded: %q", ansi.Strip(strings.Join(shut, "\n")))
	}
	for _, r := range shut {
		if cellw.String(r) != viewsStrip {
			t.Fatalf("folded row %d wide", cellw.String(r))
		}
	}
	if viewsWidth(100) != 0 || viewsWidth(150) == 0 {
		t.Fatal("the sidebar takes room only when the conversation keeps 72")
	}
}

// The mouse works the panel: the wheel scrolls it, a tab shows its view,
// a group's title folds it, the pointer lights what a click does, and ×
// dismisses it.
func TestViewsMouse(t *testing.T) {
	now := time.Now()
	var rows []host.Row
	for i := range 30 {
		rows = append(rows, host.Row{Text: "row " + strconv.Itoa(i)})
	}
	vs := []host.View{{Name: "a", Title: "Alpha", Updated: now, Groups: []host.Group{{Title: "Rows", Rows: rows}}}, {Name: "b", Title: "Beta", Updated: now}}
	c := &hostConn{}
	m := &Model{host: c}
	draw := func() string {
		c.views.x, c.views.y = 10, 5
		return ansi.Strip(strings.Join(viewsPanel(vs, nil, 40, 12, now, &c.views), "\n"))
	}
	draw()
	x0 := m.paneX() + 10
	if !m.wheelViews(c, x0+5, 8, false) || c.views.scroll != 3 {
		t.Fatalf("the wheel didn't scroll it: %d", c.views.scroll)
	}
	if m.wheelViews(c, x0-1, 8, false) {
		t.Fatal("the wheel beside it scrolled it")
	}
	c.views.scroll = 0
	draw()
	// The group's title is the body's second row, under a blank.
	if !m.hoverViews(x0+5, 5+1+1) || !c.views.hot {
		t.Fatal("the group's title isn't lit under the pointer")
	}
	m.clickViews(c, x0+5, 5+1+1)
	if !strings.Contains(draw(), "▸ Rows") {
		t.Fatalf("a click on its title didn't fold it:\n%s", draw())
	}
	tab := c.views.tabs[1]
	m.clickViews(c, x0+tab.from, 5)
	if c.views.tab != "b" || !strings.Contains(draw(), "Beta") {
		t.Fatal("a click on a tab didn't show its view")
	}
}

// × puts the shown view away among the past ones; ⟲ lists them, and a
// click on one brings it back, pinned. With none showing, a slim strip
// opens the past ones.
func TestViewsPutAwayAndBack(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := os.MkdirAll(filepath.Dir(host.SockPath("s1")), 0o700); err != nil {
		t.Fatal(err)
	}
	_ = host.SaveView("s1", host.View{Name: "prs", Title: "Pull requests", Summary: "3 ready"})
	c := &hostConn{sess: &convo.Session{Info: host.Info{ID: "s1"}}}
	m := &Model{host: c}
	draw := func() string {
		vs := m.viewsOf("s1")
		c.views.x, c.views.y = 0, 0
		return ansi.Strip(strings.Join(viewsPanel(vs, m.pastViewsOf("s1"), 40, 12, time.Now(), &c.views), "\n"))
	}
	draw()
	x0 := m.paneX()
	m.clickViews(c, x0+40-4, 0) // ×
	if vs, _ := host.Views("s1"); len(vs) != 0 || len(host.PastViews("s1")) != 1 {
		t.Fatalf("× didn't put it away: %+v", vs)
	}
	if out := draw(); !strings.Contains(out, "⟲") {
		t.Fatalf("no strip to open the past ones:\n%s", out)
	}
	m.clickViews(c, x0+1, 3) // the strip
	if out := draw(); !c.views.past || !strings.Contains(out, "Past views") || !strings.Contains(out, "Pull requests") {
		t.Fatalf("the past ones aren't listed:\n%s", out)
	}
	m.clickViews(c, x0+5, 2) // the past view's row, under a blank
	vs, _ := host.Views("s1")
	if len(vs) != 1 || !vs[0].Pinned || c.views.past {
		t.Fatalf("not brought back pinned: %+v", vs)
	}
}

// A plugin's view shows only while the plugin runs allowed to draw it,
// and names the plugin; the agent's own gives the session its line.
func TestViewsOfPlugins(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := os.MkdirAll(filepath.Dir(host.SockPath("s1")), 0o700); err != nil {
		t.Fatal(err)
	}
	_ = host.SaveView("s1", host.View{Name: "mine", Title: "Mine", Summary: "the agent's"})
	_ = host.SaveView("s1", host.View{Name: "p-ci--ci", Title: "Checks", Summary: "the plugin's", By: "ci"})
	m := &Model{}
	if vs := m.viewsOf("s1"); len(vs) != 1 || vs[0].Name != "mine" {
		t.Fatalf("a plugin that isn't running drew: %+v", vs)
	}
	if s := ansi.Strip(viewsBottom(48, "ci", "now", 0, 0)); !strings.Contains(s, "made by plugin ci") {
		t.Fatalf("the edge doesn't name the plugin: %q", s)
	}
}

// Numbers share a bar only as parts of one whole: each with a tone.
func TestShareBar(t *testing.T) {
	parts := []host.Stat{{Value: "3", Tone: "good"}, {Value: "1", Tone: "bad"}, {Value: "2", Tone: "quiet"}}
	if shareBar(parts, 20) == "" {
		t.Fatal("parts of a whole drew no bar")
	}
	if shareBar([]host.Stat{{Value: "41", Label: "uses"}, {Value: "2", Label: "unsure", Tone: "warn"}}, 20) != "" {
		t.Fatal("a number with no tone shared a bar")
	}
}
