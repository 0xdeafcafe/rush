package plugin

import (
	"path/filepath"
	"strings"
	"testing"
)

func TestCategory(t *testing.T) {
	m := Manifest{Name: "x", Command: []string{"x"}, Category: "voice"}
	if err := m.Validate("/p/x"); err != nil || m.CategoryOf().Title != "Voice" {
		t.Fatalf("voice: %v, %+v", err, m.CategoryOf())
	}
	m.Category = "telepathy"
	if err := m.Validate("/p/x"); err == nil || !strings.Contains(err.Error(), "use one of") {
		t.Fatalf("a category rush doesn't know should be refused: %v", err)
	}
	m.Category = ""
	if err := m.Validate("/p/x"); err != nil || m.CategoryOf().ID != CategoryOther {
		t.Fatalf("none is Other: %v, %+v", err, m.CategoryOf())
	}
}

// The plugins in the repository load, each under a category.
func TestRepoPluginsLoad(t *testing.T) {
	dirs, _ := filepath.Glob("../../plugins/examples/*/plugin.json")
	for _, f := range dirs {
		p, err := Load(filepath.Dir(f))
		if err != nil {
			t.Errorf("%s: %v", f, err)
			continue
		}
		if p.Category == "" {
			t.Errorf("%s names no category", p.Name)
		}
	}
}
