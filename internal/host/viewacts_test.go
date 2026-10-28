package host

import (
	"os"
	"testing"
)

// A change you make to a row shows over the agent's, stays when it saves
// the view again, and goes once the agent changes the same itself.
func TestChangeRow(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := os.MkdirAll(dir("s1"), 0o700); err != nil {
		t.Fatal(err)
	}
	no, yes := false, true
	good := "good"
	view := func(done bool, tone string) View {
		return View{Name: "todo", Title: "To do", Blocks: []Block{{Kind: "checklist", Items: []Item{
			{Text: "write it", Done: &done, Tone: tone,
				Actions: []Action{{Label: "approve", Set: &Change{Tone: &good}}, {Label: "nothing"}, {Label: "bad", Open: "file:///x"}}},
			{Text: "test it"},
		}}}}
	}
	if err := SaveView("s1", view(false, "")); err != nil {
		t.Fatal(err)
	}
	vs, _ := Views("s1")
	if as := vs[0].Blocks[0].Items[0].Actions; len(as) != 1 || as[0].Label != "approve" {
		t.Fatalf("buttons that do nothing were kept: %+v", as)
	}
	key := ItemKey(BlockKey(0), "", "write it", nil)
	if _, err := ChangeRow("s1", "todo", key, Change{Done: &yes, Tone: &good}); err != nil {
		t.Fatal(err)
	}
	it := func() Item { vs, _ := Views("s1"); return vs[0].Blocks[0].Items[0] }
	if got := it(); got.Done == nil || !*got.Done || got.Tone != "good" {
		t.Fatalf("the change doesn't show: %+v", got)
	}
	// The agent saves it again as it had it: yours stays.
	if err := SaveView("s1", view(false, "")); err != nil {
		t.Fatal(err)
	}
	if got := it(); got.Done == nil || !*got.Done || got.Tone != "good" {
		t.Fatalf("the agent's save undid yours: %+v", got)
	}
	// It changes the tone itself: its word goes, your tick stays.
	if err := SaveView("s1", view(false, "warn")); err != nil {
		t.Fatal(err)
	}
	if got := it(); got.Done == nil || !*got.Done || got.Tone != "warn" {
		t.Fatalf("the agent's own change didn't win: %+v", got)
	}
	if _, err := ChangeRow("s1", "todo", key, Change{Done: &no}); err != nil {
		t.Fatal(err)
	}
	if got := it(); got.Done == nil || *got.Done {
		t.Fatalf("unticking didn't show: %+v", got)
	}
	if _, err := ChangeRow("s1", "todo", "b0/gone", Change{Done: &yes}); err == nil {
		t.Fatal("a row not there was changed")
	}
}
