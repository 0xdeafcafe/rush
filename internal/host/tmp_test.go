package host

import (
	"os"
	"path/filepath"
	"testing"
)

// A session's scratch sits under its project, and goes there, contents
// and all, from the old layout. Once there it stays when the session moves
// to another project: its conversation names the files in it by path.
func TestPlaceTempFollowsProject(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	was := systemTemp
	systemTemp = func(string) bool { return false } // the test's own folders are in it
	t.Cleanup(func() { systemTemp = was })
	a, b := t.TempDir(), t.TempDir()
	old := filepath.Join(Root(), "s1", "tmp")
	if err := os.MkdirAll(old, 0o700); err != nil {
		t.Fatal(err)
	}
	_ = os.WriteFile(filepath.Join(old, "left"), []byte("x"), 0o600)

	got := placeTemp("s1", a)
	if want := filepath.Join(TempRoot(), slug(a), "s1"); got != want || TempDir("s1") != want {
		t.Fatalf("placed at %s (TempDir %s), want %s", got, TempDir("s1"), want)
	}
	if _, err := os.Stat(filepath.Join(got, "left")); err != nil {
		t.Fatal("the old folder's files didn't move with it")
	}

	if again := placeTemp("s1", b); again != got {
		t.Fatalf("moved to %s with the session, from %s", again, got)
	}
	if _, err := os.Stat(filepath.Join(got, "left")); err != nil || TempDir("s1") != got {
		t.Fatal("its files went with a move")
	}
	// One whose folder has gone gets a new one where it works now.
	_ = os.RemoveAll(got)
	if fresh := placeTemp("s1", b); fresh != filepath.Join(TempRoot(), slug(b), "s1") {
		t.Fatalf("a gone folder came back at %s", fresh)
	}
	got = TempDir("s1")
	if !IsTemp(filepath.Join(got, "x")) || !IsTemp(filepath.Join(old, "x")) || IsTemp(a) || !IsTempDir(got) {
		t.Fatal("IsTemp/IsTempDir misjudged a folder")
	}
	// Working in its own scratch doesn't move it.
	if placeTemp("s1", filepath.Join(got, "x")) != got {
		t.Fatal("moved while working inside it")
	}
}

func TestIsTemp(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	was := systemTemp
	systemTemp = func(string) bool { return false }
	t.Cleanup(func() { systemTemp = was })
	for p, want := range map[string]bool{
		"":                                      false,
		TempRoot():                              true,
		filepath.Join(TempRoot(), "a", "b"):     true,
		TempRoot() + "x":                        false,
		filepath.Join(Root(), "s1", "tmp"):      true,
		filepath.Join(Root(), "s1", "tmp", "x"): true,
		filepath.Join(Root(), "s1", "tmpx"):     false,
		filepath.Join(Root(), "s1"):             false,
		Root():                                  false,
		filepath.Join(Root(), "tmp"):            false,
		"/elsewhere/s1/tmp":                     false,
		"rel/tmp":                               false,
	} {
		if got := IsTemp(p); got != want {
			t.Errorf("IsTemp(%q) = %v, want %v", p, got, want)
		}
	}
}
