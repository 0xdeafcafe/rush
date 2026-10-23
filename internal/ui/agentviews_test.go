package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/charmbracelet/x/ansi"
)

// The panel fills its column exactly, says each row with its mark, and
// keeps to the height it's given.
func TestViewsPanel(t *testing.T) {
	now := time.Now()
	v := host.View{Title: "Pull requests", Updated: now.Add(-2 * time.Minute),
		Stats: []host.Stat{{Value: "8", Label: "ready", Tone: "good"}, {Value: "2", Label: "failing", Tone: "bad"}, {Value: "1", Label: "waiting"}},
		Groups: []host.Group{
			{Title: "Failing", Rows: []host.Row{
				{Text: "#8947 fix the flaky ops test", Meta: "checks cancelled", Tone: "bad", Link: "https://github.com/o/r/pull/8947"},
				{Text: "#8945 lint", Meta: "another session pushed", Tone: "warn"}}},
			{Title: "Ready for your approval", Rows: []host.Row{
				{Text: "#8965 trace filters", Meta: "all green", Tone: "good", Link: "https://github.com/o/r/pull/8965"},
				{Text: "#8942 dataset export", Meta: "all green", Tone: "good"}}},
		},
		Note: "Updated after each push."}
	rows := viewsPanel([]host.View{v}, 44, 30, now, false)
	if p := os.Getenv("RUSH_VIEWS_DUMP"); p != "" {
		_ = os.WriteFile(p, []byte(strings.Join(rows, "\n")+"\n"), 0o644)
	}
	if len(rows) != 30 {
		t.Fatalf("%d rows, want 30", len(rows))
	}
	for _, r := range rows {
		if cellw.String(r) != 44 {
			t.Fatalf("row %d wide: %q", cellw.String(r), ansi.Strip(r))
		}
	}
	text := ansi.Strip(strings.Join(rows, "\n"))
	for _, want := range []string{"Pull requests", "2m ago", "ready", "Failing  2", "✗ #8947 fix the flaky", "checks cancelled", "✓ #8965", "Updated after each push."} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	if short := viewsPanel([]host.View{v}, 44, 8, now, false); len(short) != 8 || !strings.Contains(ansi.Strip(short[7]), "more rows") {
		t.Fatalf("cut short: %q", ansi.Strip(strings.Join(short, "\n")))
	}
	// It says the agent made it, and folds to a strip with the mark that
	// opens it again.
	if !strings.Contains(ansi.Strip(rows[0]), "generative UI, made by the agent") || !strings.Contains(ansi.Strip(rows[0]), "›") {
		t.Errorf("not marked as the agent's: %q", ansi.Strip(rows[0]))
	}
	shut := viewsPanel([]host.View{v}, viewsStrip, 10, now, true)
	if len(shut) != 10 || !strings.Contains(ansi.Strip(shut[0]), "‹") {
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
