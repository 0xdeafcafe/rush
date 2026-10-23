package claude

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestAttributionOffAndOnKeepsTheRest(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "settings.json")
	if err := os.WriteFile(p, []byte(`{"model":"opus[1m]","attribution":{"sessionUrl":false}}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, set := AttributionIn(dir); set {
		t.Fatal("nothing said yet")
	}
	if err := SetAttributionIn(dir, false); err != nil {
		t.Fatal(err)
	}
	if on, set := AttributionIn(dir); on || !set {
		t.Fatalf("off: on=%v set=%v", on, set)
	}
	b, _ := os.ReadFile(p)
	if !strings.Contains(string(b), `"sessionUrl": false`) || !strings.Contains(string(b), `"opus[1m]"`) {
		t.Fatalf("the rest is kept:\n%s", b)
	}
	if err := SetAttributionIn(dir, true); err != nil {
		t.Fatal(err)
	}
	if _, set := AttributionIn(dir); set {
		b, _ := os.ReadFile(p)
		t.Fatalf("on leaves Claude Code to its default:\n%s", b)
	}
}
