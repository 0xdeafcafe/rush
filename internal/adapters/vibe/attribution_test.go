package vibe

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

func TestAttributionChangesOnlyItsLine(t *testing.T) {
	p := agent.Profile{Dir: t.TempDir()}
	path := filepath.Join(p.Dir, "config.toml")
	orig := "# mine\nactive_model = \"devstral\"\n\n[[providers]]\nname = \"mistral\"\ninclude_commit_signature = true\n"
	if err := os.WriteFile(path, []byte(orig), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, set := (Adapter{}).Attribution(p); set {
		t.Fatal("a key inside a table isn't Vibe's top-level one")
	}
	if err := (Adapter{}).SetAttribution(p, false); err != nil {
		t.Fatal(err)
	}
	if on, set := (Adapter{}).Attribution(p); on || !set {
		t.Fatalf("off: on=%v set=%v", on, set)
	}
	if err := (Adapter{}).SetAttribution(p, true); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(path)
	if want := "include_commit_signature = true\n" + orig; string(b) != want {
		t.Fatalf("got:\n%s\nwant:\n%s", b, want)
	}
	if strings.Count(string(b), "include_commit_signature") != 2 {
		t.Fatalf("one line of its own:\n%s", b)
	}
}
