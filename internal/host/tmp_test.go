package host

import (
	"os"
	"path/filepath"
	"testing"
)

// A session's scratch sits under its project, and goes with it, contents
// and all, from the old layout and from one project to the next.
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

	got = placeTemp("s1", b)
	if _, err := os.Stat(filepath.Join(got, "left")); err != nil || got != filepath.Join(TempRoot(), slug(b), "s1") {
		t.Fatalf("didn't follow to the next project: %s", got)
	}
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
