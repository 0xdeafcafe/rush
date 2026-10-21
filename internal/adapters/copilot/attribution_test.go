package copilot

import (
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

func TestAttribution(t *testing.T) {
	p := agent.Profile{Dir: t.TempDir()}
	if _, set := (Adapter{}).Attribution(p); set {
		t.Fatal("no settings.json: its default")
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
	if _, set := (Adapter{}).Attribution(p); set {
		t.Fatal("on is its default")
	}
}
