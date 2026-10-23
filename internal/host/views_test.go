package host

import (
	"os"
	"strings"
	"testing"
	"time"
)

// A view is kept, read back newest first, cut to size, and cleared.
func TestViews(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := os.MkdirAll(dir("s1"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := SaveView("s1", View{Name: "Bad Name", Title: "x"}); err == nil {
		t.Fatal("a bad name was taken")
	}
	if err := SaveView("s1", View{Name: "prs"}); err == nil {
		t.Fatal("a view with no title was taken")
	}
	if err := SaveView("nope", View{Name: "prs", Title: "x"}); err == nil {
		t.Fatal("a view was kept for no session")
	}
	v := View{Name: "prs", Title: "Pull\nrequests", Summary: "8 ready · 2 failing",
		Stats: []Stat{{Value: "8", Label: "ready", Tone: "good"}, {Value: "2", Label: "failing", Tone: "purple"}},
		Groups: []Group{{Title: "Failing", Rows: []Row{
			{Text: "#8947 " + strings.Repeat("long ", 100), Tone: "bad", Link: "https://github.com/o/r/pull/8947"},
			{Text: "#8945", Link: "file:///etc/passwd"},
		}}, {Title: "Time", Text: "per\ncheck", Folded: true, Rows: []Row{
			{Text: "e2e", Value: new(9.0)}, {Text: "lint", Value: new(-1.0)},
		}}}}
	if err := SaveView("s1", v); err != nil {
		t.Fatal(err)
	}
	if err := SaveView("s1", View{Name: "tests", Title: "Tests"}); err != nil {
		t.Fatal(err)
	}
	vs, _ := Views("s1")
	if len(vs) != 2 || vs[0].Name != "tests" || vs[1].Name != "prs" {
		t.Fatalf("views %+v", vs)
	}
	p := vs[1]
	if p.Title != "Pull requests" || p.Stats[1].Tone != "" || len([]rune(p.Groups[0].Rows[0].Text)) != viewTextMost {
		t.Fatalf("not cut to size: %+v", p)
	}
	if g := p.Groups[1]; g.Text != "per check" || !g.Folded || *g.Rows[0].Value != 9 || g.Rows[1].Value != nil {
		t.Fatalf("a chart's group: %+v", g)
	}
	if p.Groups[0].Rows[0].Link == "" || p.Groups[0].Rows[1].Link != "" {
		t.Fatalf("links: %+v", p.Groups[0].Rows)
	}
	if err := DropView("s1", "tests"); err != nil {
		t.Fatal(err)
	}
	if vs, _ := Views("s1"); len(vs) != 1 {
		t.Fatalf("after clearing: %d", len(vs))
	}
	for i := range viewsMost - 1 {
		if err := SaveView("s1", View{Name: "v" + string(rune('a'+i)), Title: "x"}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveView("s1", View{Name: "toomany", Title: "x"}); err == nil {
		t.Fatal("a seventh view was kept")
	}
	if err := SaveView("s1", View{Name: "prs", Title: "again"}); err != nil {
		t.Fatalf("updating one in place: %v", err)
	}
}

// Cleared, a view goes among the past ones; one untouched since a time
// is put away unless pinned; a past one comes back pinned.
func TestPastViews(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := os.MkdirAll(dir("s1"), 0o700); err != nil {
		t.Fatal(err)
	}
	_ = SaveView("s1", View{Name: "old", Title: "Old"})
	time.Sleep(5 * time.Millisecond)
	since := time.Now()
	_ = SaveView("s1", View{Name: "new", Title: "New", Blocks: []Block{{Kind: "hero", Value: "41"}, {Kind: "nope"}}})
	if vs, _ := Views("s1"); len(vs) != 1 {
		t.Fatalf("a block of no kind was taken: %+v", vs)
	}
	PutAwayStale("s1", since)
	vs, _ := Views("s1")
	past := PastViews("s1")
	if len(vs) != 0 || len(past) != 1 || past[0].Name != "old" || past[0].Gone.IsZero() {
		t.Fatalf("stale: now %+v, past %+v", vs, past)
	}
	if err := BringBack("s1", past[0].File); err != nil {
		t.Fatal(err)
	}
	PutAwayStale("s1", time.Now())
	if vs, _ := Views("s1"); len(vs) != 1 || !vs[0].Pinned {
		t.Fatalf("brought back: %+v", vs)
	}
	if len(PastViews("s1")) != 0 {
		t.Fatal("still among the past ones")
	}
	if err := BringBack("s1", "../../etc/passwd"); err == nil {
		t.Fatal("brought back a file outside")
	}
	_ = DropView("s1", "old")
	if vs, _ := Views("s1"); len(vs) != 0 || len(PastViews("s1")) != 1 {
		t.Fatal("clearing didn't put it away")
	}
}
