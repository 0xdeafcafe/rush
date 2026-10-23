package host

import (
	"os"
	"strings"
	"testing"
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
