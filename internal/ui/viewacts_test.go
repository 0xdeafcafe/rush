package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// A click opens a table row: its cells by their columns, its detail and
// its buttons under it; a button changes its row; a checklist's mark
// ticks; another click shuts the row.
func TestViewRowsWork(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := os.MkdirAll(filepath.Dir(host.SockPath("s1")), 0o700); err != nil {
		t.Fatal(err)
	}
	good := "good"
	_ = host.SaveView("s1", host.View{Name: "prs", Title: "PRs", Blocks: []host.Block{
		{Kind: "table", Columns: []string{"PR", "What", "State"}, Items: []host.Item{
			{Cells: []string{"#3", "auto-compact, session archive and resume across hosts", "reviewing"}, Tone: "busy",
				Link: "https://github.com/o/r/pull/3", Detail: "Conflicts with main.",
				Actions: []host.Action{{Label: "approve", Set: &host.Change{Tone: &good}}}},
		}},
		{Kind: "checklist", Items: []host.Item{{Text: "run the tests"}}},
	}})
	c := &hostConn{sess: &convo.Session{Info: host.Info{ID: "s1"}}}
	m := &Model{host: c}
	draw := func() []string {
		c.views.x, c.views.y = 0, 0
		return strings.Split(ansi.Strip(strings.Join(viewsPanel(m.viewsOf("s1"), nil, 48, 30, time.Now(), &c.views), "\n")), "\n")
	}
	at := func(ls []string, s string) int {
		for i, l := range ls {
			if strings.Contains(l, s) {
				return i
			}
		}
		t.Fatalf("no %q in:\n%s", s, strings.Join(ls, "\n"))
		return -1
	}
	x0 := m.paneX()
	ls := draw()
	m.clickViews(c, x0+8, at(ls, "#3"))
	ls = draw()
	out := strings.Join(ls, "\n")
	for _, want := range []string{"What   auto-compact, session archive and", "Conflicts with main.", "approve", "open ↗", "copy"} {
		if !strings.Contains(out, want) {
			t.Fatalf("the open row has no %q:\n%s", want, out)
		}
	}
	y := at(ls, "approve")
	from := c.views.acts[y-1].btns[0].from
	m.clickViews(c, x0+from, y)
	if vs, _ := host.Views("s1"); vs[0].Blocks[0].Items[0].Tone != "good" {
		t.Fatalf("approve didn't change its row: %+v", vs[0].Blocks[0].Items[0])
	}
	m.clickViews(c, x0+2, at(draw(), "run the tests"))
	if vs, _ := host.Views("s1"); vs[0].Blocks[1].Items[0].Done == nil || !*vs[0].Blocks[1].Items[0].Done {
		t.Fatal("a click on its mark didn't tick it")
	}
	ls = draw()
	m.clickViews(c, x0+8, at(ls, "#3"))
	if strings.Contains(strings.Join(draw(), "\n"), "Conflicts") {
		t.Fatal("another click didn't shut it")
	}
}
