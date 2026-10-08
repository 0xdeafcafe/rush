package ui

import (
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// The picker finds folders on disk: a typed path's, or those beside the
// projects it knows; known projects list apart from other folders.
func TestPickerDiskDirs(t *testing.T) {
	root := t.TempDir()
	for _, d := range []string{"app", "api", "other", ".hidden"} {
		_ = os.Mkdir(filepath.Join(root, d), 0o700)
	}
	if got := diskDirs(root+"/a", nil); !slices.Equal(got, []string{filepath.Join(root, "api"), filepath.Join(root, "app")}) {
		t.Fatalf("typed path: %v", got)
	}
	if got := diskDirs("oth", []string{root}); !slices.Equal(got, []string{filepath.Join(root, "other")}) {
		t.Fatalf("beside projects: %v", got)
	}
	app := filepath.Join(root, "app")
	p := &picker{dirs: []string{"/x/folder", app}, known: map[string]bool{app: true}, disk: []string{app, filepath.Join(root, "other")}}
	if got := p.shownDirs(); !slices.Equal(got, []string{app, "/x/folder", filepath.Join(root, "other")}) || p.dirSection(got[2]) != "On disk" {
		t.Fatalf("sections: %v", got)
	}
}
