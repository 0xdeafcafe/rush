package fleet

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
)

// What a finished agent made in a temp folder can go; what was there
// before it, what's changed since, and anything nested are kept or folded.
func TestLookLeft(t *testing.T) {
	t.Setenv("TMPDIR", "/tmp") // a system temp folder, whatever runs the test
	t.Setenv("RUSH_HOME", t.TempDir())
	before := filepath.Join(t.TempDir(), "before")
	_ = os.Mkdir(before, 0o700)
	time.Sleep(10 * time.Millisecond)
	_ = os.MkdirAll(filepath.Join(host.Root(), "s1"), 0o700) // the session begins
	time.Sleep(10 * time.Millisecond)
	made := filepath.Join(t.TempDir(), "clone")
	_ = os.MkdirAll(filepath.Join(made, "sub"), 0o700)
	a := &Agent{ID: "s1", DisplayName: "a", UpdatedAt: time.Now(), Left: []host.Left{
		{Path: made}, {Path: filepath.Join(made, "sub")}, {Path: before}, {Path: "/nope/gone"},
	}}
	items := LookLeft([]*Agent{a})
	got := map[string]LeftItem{}
	for _, it := range items {
		got[it.Path] = it
	}
	if len(items) != 3 {
		t.Fatalf("nested not folded: %+v", items)
	}
	if !got[made].Safe() || got[before].Safe() || got[before].Why() == "" || !got["/nope/gone"].Gone {
		t.Fatalf("classes wrong: %+v", items)
	}
	if _, err := RemoveLeft(got[before]); err == nil {
		t.Fatal("removed what was there before the agent")
	}
	if _, err := RemoveLeft(got[made]); err != nil || exists(made) {
		t.Fatalf("didn't remove what it made: %v", err)
	}
	a.UpdatedAt = time.Now().Add(-time.Hour) // a later write is someone else's
	_ = os.Mkdir(made, 0o700)
	if it := LookLeft([]*Agent{a}); it[0].Path == made && it[0].Safe() {
		t.Fatal("touched since, yet safe")
	}
	if !shallow("/opt") || !shallow(os.Getenv("HOME")+"/Documents") || shallow("/tmp/x") {
		t.Fatal("shallow misjudged")
	}
}

func exists(p string) bool { _, err := os.Stat(p); return err == nil }
