package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// A board stands its columns side by side only when every card fits a
// column on one line; in a narrow panel with longer cards it stacks
// them, each card whole.
func TestBoardStacksWhenCardsDontFit(t *testing.T) {
	draw := func(b host.Block, w int) string {
		var ls []string
		for _, l := range blockBoard(&b, w, blockCtx{st: &viewsState{}, key: "v/b0"}) {
			ls = append(ls, ansi.Strip(l.s))
		}
		return strings.Join(ls, "\n")
	}
	short := host.Block{Kind: "board", Items: []host.Item{
		{Text: "Doing", Items: []host.Item{{Text: "API ref"}}},
		{Text: "Done", Items: []host.Item{{Text: "404 page"}}},
	}}
	if out := draw(short, 48); !strings.Contains(strings.Split(out, "\n")[0], "Done") {
		t.Fatalf("short cards should stand side by side:\n%s", out)
	}
	long := host.Block{Kind: "board", Items: []host.Item{
		{Text: "Doing", Items: []host.Item{{Text: "API ref polish", Meta: "cramped, spelling, nested lists"}}},
		{Text: "On branch, not pushed", Items: []host.Item{{Text: "Sidebar ellipsis, bottom space, preload"}}},
	}}
	out := draw(long, 48)
	if strings.Contains(strings.Split(out, "\n")[0], "On branch") {
		t.Fatalf("long cards should stack:\n%s", out)
	}
	for _, want := range []string{"cramped, spelling, nested lists", "Sidebar ellipsis, bottom space, preload"} {
		if !strings.Contains(out, want) {
			t.Fatalf("no %q whole in:\n%s", want, out)
		}
	}
}
